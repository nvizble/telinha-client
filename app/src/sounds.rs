//! Short, local notification sounds; no media files or capture devices needed.
use anyhow::Context;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    f32::consts::TAU,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Clone, Copy)]
pub enum Cue {
    Started,
    Stopped,
}

const SOUND_SECONDS: f32 = 0.48;

// A deterministic noise bed keeps exported previews identical to app playback.
fn static_noise(time: f32) -> f32 {
    fn noise(mut frame: u32) -> f32 {
        frame = frame.wrapping_add(0x9e37_79b9);
        frame = (frame ^ (frame >> 16)).wrapping_mul(0x21f0_aaad);
        frame = (frame ^ (frame >> 15)).wrapping_mul(0x735a_2d97);
        frame ^= frame >> 15;
        (frame >> 8) as f32 / 8_388_607.5 - 1.0
    }
    let frame = (time * 48_000.0) as u32;
    (noise(frame) + noise(frame.wrapping_sub(1))) * 0.5
}

impl Cue {
    fn sample(self, time: f32) -> f32 {
        let duration = match self {
            Self::Started => SOUND_SECONDS,
            Self::Stopped => 0.34,
        };
        if !(0.0..duration).contains(&time) {
            return 0.0;
        }
        let attack = (time / 0.0015).min(1.0);
        let release = ((duration - time) / 0.04).min(1.0);
        let noise = static_noise(time);
        // Relay click and a low power-supply thump, softened at the boundary.
        let click = noise * (-150.0 * time).exp() * 0.20;
        let thump = (TAU * 72.0 * time).sin() * (-38.0 * time).exp() * 0.18;
        let texture = match self {
            Self::Started => {
                // A relay engages, then the power supply rises into a short,
                // rounded electrical sweep. Only a trace of static at ignition.
                let relay = (TAU * 1900.0 * time).sin() * (-170.0 * time).exp() * 0.06;
                let age = (time - 0.025).max(0.0);
                let envelope = (age / 0.012).min(1.0) * (-9.0 * age).exp();
                let phase = TAU * (1150.0 * age - 950.0 * 0.055 * (1.0 - (-age / 0.055).exp()));
                let power = (phase.sin() + 0.22 * (2.0 * phase).sin() + 0.18 * (0.5 * phase).sin())
                    * envelope
                    * 0.16;
                let ignition = noise * envelope * (-70.0 * age).exp() * 0.025;
                relay + power + ignition
            }
            Self::Stopped => {
                // A fast falling electrical sweep collapses into silence.
                let phase = TAU * 2600.0 * 0.055 * (1.0 - (-time / 0.055).exp());
                let discharge =
                    (phase.sin() + 0.15 * (2.0 * phase).sin()) * (-12.0 * time).exp() * 0.075;
                noise * (-22.0 * time).exp() * 0.10 + discharge
            }
        };
        (click + thump + texture) * attack * release * 0.75
    }
}

pub fn play(cue: Cue, volume: Arc<AtomicU32>) {
    if f32::from_bits(volume.load(Ordering::Relaxed)) <= 0.0 {
        return;
    }
    // Device setup and playback must never block the UI or the media runtime.
    if let Err(error) = std::thread::Builder::new()
        .name("telinha-notification".into())
        .spawn(move || {
            if let Err(error) = output(cue, volume) {
                tracing::warn!("Não deu pra reproduzir o aviso sonoro: {error}");
            }
        })
    {
        tracing::warn!("Não deu pra iniciar o aviso sonoro: {error}");
    }
}

fn output(cue: Cue, volume: Arc<AtomicU32>) -> anyhow::Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .context("Nenhuma saída de áudio encontrada.")?;
    let supported = device.default_output_config()?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let (done, finished) = mpsc::sync_channel(1);
    let stream = match format {
        cpal::SampleFormat::F32 => stream::<f32>(&device, &config, cue, volume, done)?,
        cpal::SampleFormat::I16 => stream::<i16>(&device, &config, cue, volume, done)?,
        cpal::SampleFormat::U16 => stream::<u16>(&device, &config, cue, volume, done)?,
        _ => anyhow::bail!("Formato da saída de áudio não suportado."),
    };
    stream.play()?;
    // Bounded lifetime even if the output device disappears during playback.
    finished.recv_timeout(Duration::from_secs(2))?;
    Ok(())
}

fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    cue: Cue,
    volume: Arc<AtomicU32>,
    done: mpsc::SyncSender<()>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let channels = config.channels as usize;
    let rate = config.sample_rate.0 as f32;
    // Render trailing silence so the last note can leave the device buffer.
    let frames = ((SOUND_SECONDS + 0.15) * rate) as usize;
    let mut position = 0;
    device.build_output_stream(
        config,
        move |out: &mut [T], _| {
            let gain = f32::from_bits(volume.load(Ordering::Relaxed)).clamp(0.0, 1.0);
            for frame in out.chunks_mut(channels) {
                let sample = cue.sample(position as f32 / rate) * gain;
                frame.fill(T::from_sample(sample));
                position += 1;
            }
            if position >= frames {
                let _ = done.try_send(());
            }
        },
        |error| tracing::warn!("Aviso sonoro interrompido: {error}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_are_distinct_bounded_and_fade_to_silence() {
        for rate in [44100, 48000, 96000] {
            let started: Vec<_> = (0..rate / 2)
                .map(|frame| Cue::Started.sample(frame as f32 / rate as f32))
                .collect();
            let stopped: Vec<_> = (0..rate / 2)
                .map(|frame| Cue::Stopped.sample(frame as f32 / rate as f32))
                .collect();
            assert_ne!(started, stopped);
            for samples in [&started, &stopped] {
                assert_eq!(samples[0], 0.0);
                assert!(
                    samples
                        .iter()
                        .all(|value| value.is_finite() && value.abs() < 0.4)
                );
                assert!(samples.iter().any(|value| value.abs() > 0.1));
                let end = (SOUND_SECONDS * rate as f32).ceil() as usize;
                assert!(samples[end..].iter().all(|value| *value == 0.0));
            }
        }
    }
}
