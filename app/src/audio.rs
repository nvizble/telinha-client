use anyhow::Context;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use futures_util::StreamExt;
use livekit::{track::RemoteAudioTrack, webrtc::audio_stream::native::NativeAudioStream};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

#[cfg(any(target_os = "windows", test))]
mod policy;
#[cfg(target_os = "windows")]
mod target;

/// A window-backed application selected independently from the video source.
/// PID and creation time bind a selection to one process even if a window ID is reused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Application {
    pub window_id: u64,
    pub process_id: u32,
    pub created_at: u64,
    pub name: String,
}

/// Called alongside window enumeration on a background worker, never while rendering.
pub fn applications(sources: &[crate::capture::Source]) -> Vec<Application> {
    #[cfg(target_os = "windows")]
    {
        target::applications(sources)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = sources;
        Vec::new()
    }
}

/// One bounded playback buffer per remote participant; never capture the mic.
pub async fn play(
    track: RemoteAudioTrack,
    volume: Arc<AtomicU32>,
    selected: Arc<Mutex<Option<String>>>,
    participant: String,
) -> anyhow::Result<()> {
    let queue = Arc::new(Mutex::new(VecDeque::<i16>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    struct Stop(Arc<AtomicBool>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    let _guard = Stop(stop.clone());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let queue_for_output = queue.clone();
    std::thread::Builder::new()
        .name("telinha-playback".into())
        .spawn(move || {
            let start = || -> anyhow::Result<(cpal::Stream, cpal::StreamConfig)> {
                let device = cpal::default_host()
                    .default_output_device()
                    .context("Nenhuma saída de áudio encontrada.")?;
                let supported = device.default_output_config()?;
                let format = supported.sample_format();
                let config: cpal::StreamConfig = supported.into();
                fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
                    device: &cpal::Device,
                    config: &cpal::StreamConfig,
                    queue: Arc<Mutex<VecDeque<i16>>>,
                    volume: Arc<AtomicU32>,
                ) -> Result<cpal::Stream, cpal::BuildStreamError> {
                    device.build_output_stream(
                        config,
                        move |out: &mut [T], _| {
                            let gain = f32::from_bits(volume.load(Ordering::Relaxed));
                            // The audio callback must not wait for a producer holding the lock.
                            if let Some(mut queue) = queue.try_lock() {
                                for sample in out {
                                    *sample = T::from_sample(
                                        queue.pop_front().unwrap_or(0) as f32 / 32768.0 * gain,
                                    );
                                }
                            } else {
                                out.fill(T::from_sample(0.0));
                            }
                        },
                        |error| tracing::warn!("Saída de áudio interrompida: {error}"),
                        None,
                    )
                }
                let stream = match format {
                    cpal::SampleFormat::F32 => {
                        stream::<f32>(&device, &config, queue_for_output, volume)?
                    }
                    cpal::SampleFormat::I16 => {
                        stream::<i16>(&device, &config, queue_for_output, volume)?
                    }
                    cpal::SampleFormat::U16 => {
                        stream::<u16>(&device, &config, queue_for_output, volume)?
                    }
                    _ => anyhow::bail!("Formato da saída de áudio não suportado."),
                };
                stream.play()?;
                Ok((stream, config))
            };
            match start() {
                Ok((stream, config)) => {
                    let _ = tx.send(Ok((config.sample_rate.0, config.channels)));
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    drop(stream);
                }
                Err(error) => {
                    let _ = tx.send(Err(error.to_string()));
                }
            }
        })?;
    let (rate, channels) = rx.await?.map_err(anyhow::Error::msg)?;
    let mut audio = NativeAudioStream::new(track.rtc_track(), rate as i32, channels as i32);
    let capacity = rate as usize * channels as usize / 5; // 200 ms maximum latency.
    while let Some(frame) = audio.next().await {
        let mut queue = queue.lock();
        if selected.lock().as_deref() != Some(&participant) {
            queue.clear();
            continue;
        }
        queue.extend(frame.data.iter().copied());
        let overflow = queue.len().saturating_sub(capacity);
        if overflow > 0 {
            queue.drain(..overflow);
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub mod windows {
    use super::*;
    use livekit::webrtc::{
        audio_frame::AudioFrame,
        audio_source::{AudioSourceOptions, native::NativeAudioSource},
    };
    use std::{borrow::Cow, time::Duration};
    use wasapi::{AudioClient, Direction, SampleType, StreamMode, WaveFormat};

    pub struct ProcessAudio {
        pub source: NativeAudioSource,
        stop: Arc<AtomicBool>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for ProcessAudio {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.task.abort();
        }
    }

    impl ProcessAudio {
        pub async fn start(
            application: Application,
            faults: tokio::sync::mpsc::UnboundedSender<String>,
        ) -> anyhow::Result<Self> {
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let delivery_stop = stop.clone();
            let delivery_faults = faults.clone();
            let (pcm_tx, mut pcm_rx) = tokio::sync::mpsc::channel::<Vec<i16>>(5);
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
            let source = NativeAudioSource::new(
                AudioSourceOptions {
                    echo_cancellation: false,
                    noise_suppression: false,
                    auto_gain_control: false,
                },
                48000,
                2,
                100,
            );
            std::thread::Builder::new()
                .name("telinha-process-audio".into())
                .spawn(move || {
                    let mut ready_tx = Some(ready_tx);
                    let result = (|| -> anyhow::Result<()> {
                        // Open and keep the original process on this worker. Never resolve a
                        // stale window into a different application's PID or use system loopback.
                        let target = target::Target::open(&application)?;
                        if worker_stop.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        wasapi::initialize_mta().ok()?;
                        let mut client = AudioClient::new_application_loopback_client(
                            target.process_id(),
                            true,
                        )?;
                        let format = WaveFormat::new(32, 32, SampleType::Float, 48000, 2, None);
                        client.initialize_client(
                            &format,
                            Direction::Capture,
                            StreamMode::EventsShared {
                                autoconvert: true,
                                buffer_duration_hns: 0,
                            },
                            None,
                        )?;
                        let event = client.set_get_eventhandle()?;
                        let capture = client.get_audiocaptureclient()?;
                        let mut queue = VecDeque::new();
                        // Activation can finish after the async start timed out or was canceled.
                        // In that case do not start capturing, even briefly.
                        if worker_stop.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        target.check_alive()?;
                        client.start_stream()?;
                        let result = (|| -> anyhow::Result<()> {
                            target.check_alive()?;
                            if let Some(tx) = ready_tx.take() {
                                let _ = tx.send(Ok(()));
                            }
                            while !worker_stop.load(Ordering::Relaxed) && !pcm_tx.is_closed() {
                                // A minimized or quiet window is still a live application. Only
                                // the retained process handle decides when its audio must end.
                                target.check_alive()?;
                                if capture.get_next_packet_size()?.unwrap_or(0) > 0 {
                                    capture.read_from_device_to_deque(&mut queue)?;
                                }
                                // 10 ms, stereo float32 -> signed PCM16.
                                while queue.len() >= 480 * 2 * 4 {
                                    let bytes: Vec<u8> = queue.drain(..480 * 2 * 4).collect();
                                    let pcm = bytes
                                        .chunks_exact(4)
                                        .map(|b| {
                                            (f32::from_le_bytes(b.try_into().unwrap())
                                                .clamp(-1.0, 1.0)
                                                * i16::MAX as f32)
                                                as i16
                                        })
                                        .collect();
                                    let _ = pcm_tx.try_send(pcm); // Drop rather than build audio latency.
                                }
                                let _ = event.wait_for_event(20);
                            }
                            Ok(())
                        })();
                        // Stop WASAPI on both normal shutdown and a process-lifetime failure.
                        let stopped = client.stop_stream();
                        result?;
                        stopped?;
                        Ok(())
                    })();
                    if let Err(error) = result {
                        if worker_stop.load(Ordering::Relaxed) {
                            return;
                        }
                        let message = format!("Não deu pra capturar o som deste programa: {error}");
                        if let Some(tx) = ready_tx.take() {
                            let _ = tx.send(Err(message));
                        } else {
                            let _ = faults.send(message);
                        }
                    }
                })?;
            let audio_source = source.clone();
            let task = tokio::spawn(async move {
                while let Some(data) = pcm_rx.recv().await {
                    let frame = AudioFrame {
                        data: Cow::Owned(data),
                        sample_rate: 48000,
                        num_channels: 2,
                        samples_per_channel: 480,
                    };
                    if let Err(error) = audio_source.capture_frame(&frame).await {
                        if !delivery_stop.swap(true, Ordering::Relaxed) {
                            let _ = delivery_faults.send(format!(
                                "O envio de áudio foi interrompido. Escolha o aplicativo novamente: {error}"
                            ));
                        }
                        break;
                    }
                }
            });
            let capture = Self { source, stop, task };
            tokio::time::timeout(Duration::from_secs(10), ready_rx)
                .await
                .context("A captura de áudio demorou demais.")?
                .context("Não deu pra iniciar o áudio deste programa.")?
                .map_err(anyhow::Error::msg)?;
            Ok(capture)
        }
    }
}
