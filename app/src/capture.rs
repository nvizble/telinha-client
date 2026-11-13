use anyhow::{Context, bail};
use livekit::webrtc::{
    desktop_capturer::{
        CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions,
    },
    prelude::{VideoFrame, VideoResolution, VideoRotation},
    video_frame::VideoBuffer,
    video_source::native::NativeVideoSource,
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use telinha_shared::Quality;

mod convert;
#[cfg(target_os = "windows")]
mod window;

#[derive(Clone, Debug)]
pub struct Source {
    pub id: Option<u64>,
    pub name: String,
    pub kind: DesktopCaptureSourceType,
}
impl Source {
    pub fn supports_audio(&self) -> bool {
        cfg!(target_os = "windows") && self.kind == DesktopCaptureSourceType::Window
    }
}

pub fn sources() -> anyhow::Result<Vec<Source>> {
    #[cfg(target_os = "macos")]
    {
        // ScreenCaptureKit's own picker lists both windows and displays. It also
        // handles permission without relying on a stale macOS display list.
        Ok(vec![Source {
            id: None,
            name: "Escolher uma janela ou tela no macOS".into(),
            kind: DesktopCaptureSourceType::Generic,
        }])
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut result = Vec::new();
        for kind in [
            DesktopCaptureSourceType::Window,
            DesktopCaptureSourceType::Screen,
        ] {
            let capturer = DesktopCapturer::new(DesktopCapturerOptions::new(kind))
                .context("Não deu pra listar as janelas. Confira a permissão de captura.")?;
            for (index, source) in capturer.get_source_list().into_iter().enumerate() {
                let name = if source.title().is_empty() {
                    format!("Tela {}", index + 1)
                } else {
                    source.title()
                };
                result.push(Source {
                    id: Some(source.id()),
                    name,
                    kind,
                });
            }
        }
        Ok(result)
    }
}

pub struct Capture {
    pub source: NativeVideoSource,
    pub frames: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    output: Arc<parking_lot::Mutex<Option<NativeVideoSource>>>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct StopOnDrop(Option<Arc<AtomicBool>>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if let Some(stop) = &self.0 {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

// The native callback runs outside Tokio, including on threads owned by WebRTC.
// Keep the frame pipeline separate so it can also be exercised without screen access.
struct FramePublisher {
    runtime: tokio::runtime::Handle,
    quality: Quality,
    source: Option<NativeVideoSource>,
    frames: Arc<AtomicU64>,
    ready: Option<tokio::sync::oneshot::Sender<NativeVideoSource>>,
    converter: convert::Converter,
    output: Arc<parking_lot::Mutex<Option<NativeVideoSource>>>,
}

impl FramePublisher {
    fn publish_bgra(&mut self, w: i32, h: i32, stride: u32, data: &[u8]) -> anyhow::Result<()> {
        let buffer = self.converter.convert(w, h, stride, data, self.quality)?;
        // NativeVideoSource starts a Tokio keepalive task on its first frame.
        // Enter on the actual callback thread, not only on the spawning thread.
        let _runtime = self.runtime.enter();
        let (width, height) = (buffer.width(), buffer.height());
        let native = self
            .source
            .get_or_insert_with(|| NativeVideoSource::new(VideoResolution { width, height }, true));
        // A zero timestamp lets LiveKit use its clock, also used by its keepalive.
        let output = self.output.lock().clone().unwrap_or_else(|| native.clone());
        output.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, buffer));
        self.frames.fetch_add(1, Ordering::Relaxed);
        if let Some(tx) = self.ready.take() {
            let _ = tx.send(native.clone());
        }
        Ok(())
    }
}

fn capture_callback(
    stop: &AtomicBool,
    faults: &tokio::sync::mpsc::UnboundedSender<String>,
    callback: impl FnOnce() -> anyhow::Result<()>,
) {
    // Rust must never unwind through the SDK's C++ callback: cxx aborts the app.
    let error = match catch_unwind(AssertUnwindSafe(callback)) {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error.to_string(),
        Err(_) => "A captura falhou ao processar a imagem. Tente compartilhar de novo.".into(),
    };
    stop.store(true, Ordering::Relaxed);
    tracing::error!("{error}");
    let _ = faults.send(error);
}

impl Capture {
    /// Commit a prepared capture to the existing published track. Preparation
    /// uses a separate source, so canceling never changes what viewers receive.
    pub fn replace(&mut self, previous: &Self) {
        previous.stop.store(true, Ordering::Relaxed);
        self.source = previous.source.clone();
        *self.output.lock() = Some(self.source.clone());
    }

    pub async fn start(
        choice: Source,
        quality: Quality,
        faults: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> anyhow::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut guard = StopOnDrop(Some(stop.clone()));
        let frames = Arc::new(AtomicU64::new(0));
        let output = Arc::new(parking_lot::Mutex::new(None));
        let thread_output = output.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let thread_stop = stop.clone();
        let thread_frames = frames.clone();
        let runtime = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("telinha-capture".into())
            .spawn(move || {
                let result = (|| -> anyhow::Result<()> {
                    #[cfg(target_os = "windows")]
                    let window = window::Target::for_source(&choice)?;
                    let mut options = DesktopCapturerOptions::new(choice.kind);
                    options.set_include_cursor(true);
                    let mut capturer = DesktopCapturer::new(options).context(
                        "Não deu pra iniciar a captura. Confira a permissão de gravação de tela.",
                    )?;
                    let source = if let Some(id) = choice.id {
                        Some(
                            capturer
                                .get_source_list()
                                .into_iter()
                                .find(|s| s.id() == id)
                                .context("A janela foi fechada. Escolha outra janela.")?,
                        )
                    } else {
                        None
                    };
                    let callback_stop = thread_stop.clone();
                    let mut publisher = FramePublisher {
                        runtime,
                        quality,
                        source: None,
                        frames: thread_frames,
                        ready: Some(ready_tx),
                        converter: convert::Converter::default(),
                        output: thread_output,
                    };
                    let callback_faults = faults.clone();
                    capturer.start_capture(source, move |result| {
                        if callback_stop.load(Ordering::Relaxed) {
                            return;
                        }
                        capture_callback(&callback_stop, &callback_faults, || {
                            let frame = match result {
                                Ok(frame) => frame,
                                Err(CaptureError::Temporary) => return Ok(()),
                                Err(CaptureError::Permanent) => bail!(
                                    "A captura terminou. Escolha a janela de novo pra continuar."
                                ),
                            };
                            publisher.publish_bgra(
                                frame.width(),
                                frame.height(),
                                frame.stride(),
                                frame.data(),
                            )
                        });
                    });
                    let period = Duration::from_secs_f64(1.0 / quality.fps() as f64);
                    #[cfg(target_os = "windows")]
                    let mut checked_window = Instant::now() - Duration::from_secs(1);
                    while !thread_stop.load(Ordering::Relaxed) {
                        // WGC may return temporary errors or its last frame after
                        // a game window closes. Verify the original HWND/owner;
                        // a static image or a minimized window is still valid.
                        #[cfg(target_os = "windows")]
                        if checked_window.elapsed() >= Duration::from_millis(250) {
                            if let Some(window) = &window {
                                window.check()?;
                            }
                            checked_window = Instant::now();
                        }
                        let before = Instant::now();
                        capturer.capture_frame();
                        if let Some(wait) = period.checked_sub(before.elapsed()) {
                            std::thread::sleep(wait);
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    thread_stop.store(true, Ordering::Relaxed);
                    let _ = faults.send(error.to_string());
                }
            })?;
        let source = match tokio::time::timeout(Duration::from_secs(45), ready_rx).await {
            Ok(Ok(source)) => source,
            Ok(Err(_)) => bail!(
                "Não deu pra capturar. Confira a permissão de gravação de tela e escolha uma janela."
            ),
            Err(_) => bail!(
                "Nenhuma imagem chegou. Escolha uma fonte e confira a permissão de gravação de tela."
            ),
        };
        // Disarm cancellation only after the owner has been constructed.
        let capture = Self {
            source,
            frames,
            stop,
            output,
        };
        guard.0.take();
        Ok(capture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prepared_capture_does_not_replace_video_until_committed() {
        use futures_util::StreamExt;
        use livekit::{
            track::LocalVideoTrack,
            webrtc::{
                video_frame::native::VideoFrameBufferExt, video_source::RtcVideoSource,
                video_stream::native::NativeVideoStream,
            },
        };
        fn prepared() -> (FramePublisher, Capture) {
            let output = Arc::new(parking_lot::Mutex::new(None));
            let frames = Arc::new(AtomicU64::new(0));
            let mut publisher = FramePublisher {
                runtime: tokio::runtime::Handle::current(),
                quality: Quality::FullHd30,
                source: None,
                frames: frames.clone(),
                ready: None,
                converter: convert::Converter::default(),
                output: output.clone(),
            };
            publisher.publish_bgra(32, 32, 128, &[255; 4096]).unwrap();
            let capture = Capture {
                source: publisher.source.clone().unwrap(),
                frames,
                stop: Arc::new(AtomicBool::new(false)),
                output,
            };
            (publisher, capture)
        }
        let (mut original, current) = prepared();
        let track = LocalVideoTrack::create_video_track(
            "Synthetic switch",
            RtcVideoSource::Native(current.source.clone()),
        );
        let mut frames = NativeVideoStream::new(track.rtc_track());
        let (_, canceled) = prepared();
        drop(canceled);
        assert!(!current.stop.load(Ordering::Relaxed));
        original.publish_bgra(32, 32, 128, &[255; 4096]).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), frames.next())
                .await
                .unwrap()
                .is_some()
        );
        let (mut next, mut replacement) = prepared();
        replacement.replace(&current);
        assert!(current.stop.load(Ordering::Relaxed));
        drop(current);
        let red: Vec<u8> = [0, 0, 255, 255].into_iter().cycle().take(4096).collect();
        next.publish_bgra(32, 32, 128, &red).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(frame) = frames.next().await {
                let buffer = frame.buffer.to_i420();
                let (w, h) = (buffer.width(), buffer.height());
                let mut bgra = vec![0; (w * h * 4) as usize];
                buffer.to_argb(
                    livekit::webrtc::prelude::VideoFormatType::ARGB,
                    &mut bgra,
                    w * 4,
                    w as i32,
                    h as i32,
                );
                if bgra[2] > 180 && bgra[0] < 80 && bgra[1] < 80 {
                    return;
                }
            }
            panic!("Existing video track ended during source replacement");
        })
        .await
        .expect("Existing published source did not receive the replacement pixels");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn first_capture_frame_on_native_thread_initializes_livekit() {
        let frames = Arc::new(AtomicU64::new(0));
        let (ready, received) = tokio::sync::oneshot::channel();
        let mut publisher = FramePublisher {
            runtime: tokio::runtime::Handle::current(),
            quality: Quality::FullHd30,
            source: None,
            frames: frames.clone(),
            ready: Some(ready),
            converter: convert::Converter::default(),
            output: Arc::default(),
        };
        // ScreenCaptureKit and the Windows capturer call from a plain native thread.
        // Odd dimensions and row padding also exercise conversion and resizing.
        std::thread::spawn(move || {
            assert!(tokio::runtime::Handle::try_current().is_err());
            publisher
                .publish_bgra(321, 181, 1312, &vec![255; 1312 * 181])
                .unwrap();
            publisher
                .publish_bgra(320, 180, 1280, &vec![255; 1280 * 180])
                .unwrap();
            assert!(tokio::runtime::Handle::try_current().is_err());
        })
        .join()
        .expect("capturing must not panic outside Tokio");
        assert!(received.await.is_ok());
        assert_eq!(frames.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn callback_panics_become_a_recoverable_capture_error() {
        let stop = AtomicBool::new(false);
        let (faults, mut errors) = tokio::sync::mpsc::unbounded_channel();
        capture_callback(&stop, &faults, || panic!("injected frame failure"));
        assert!(stop.load(Ordering::Relaxed));
        assert!(
            errors
                .try_recv()
                .unwrap()
                .contains("Tente compartilhar de novo")
        );
    }
}
