//! Optional local video preview. It never receives or creates an audio track.
use crate::session::Pixels;
use futures_util::StreamExt;
use livekit::{
    prelude::*,
    webrtc::{
        prelude::VideoFormatType, video_frame::native::VideoFrameBufferExt,
        video_stream::native::NativeVideoStream,
    },
};
use parking_lot::Mutex;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct State {
    enabled: Arc<AtomicBool>,
    frame: Arc<Mutex<Option<Pixels>>>,
}

impl State {
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.clear();
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn take(&self) -> Option<Pixels> {
        self.frame.lock().take()
    }
    fn clear(&self) {
        self.frame.lock().take();
    }
    fn offer(&self, frame: Pixels) {
        let mut slot = self.frame.lock();
        if self.enabled() {
            *slot = Some(frame);
        }
    }
}

pub struct Worker {
    task: tokio::task::JoinHandle<()>,
    state: State,
}

impl Worker {
    pub fn start(track: LocalVideoTrack, state: State) -> Self {
        let output = state.clone();
        let task = tokio::spawn(async move {
            loop {
                if !output.enabled() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                let mut stream = NativeVideoStream::new(track.rtc_track());
                let mut last = Instant::now() - Duration::from_secs(1);
                while output.enabled() {
                    let frame =
                        match tokio::time::timeout(Duration::from_millis(200), stream.next()).await
                        {
                            Ok(Some(frame)) => frame,
                            Ok(None) => break,
                            Err(_) => continue,
                        };
                    if last.elapsed() < Duration::from_millis(100) {
                        continue;
                    }
                    last = Instant::now();
                    let mut buffer = frame.buffer.to_i420();
                    let (width, height) = telinha_shared::Quality::Hd60
                        .fit(frame.buffer.width(), frame.buffer.height());
                    if (width, height) != (frame.buffer.width(), frame.buffer.height()) {
                        buffer = buffer.scale(width as i32, height as i32);
                    }
                    let mut bgra = vec![0; width as usize * height as usize * 4];
                    buffer.to_argb(
                        VideoFormatType::ARGB,
                        &mut bgra,
                        width * 4,
                        width as i32,
                        height as i32,
                    );
                    output.offer(Pixels {
                        track: "local-preview".into(),
                        width,
                        height,
                        bgra,
                    });
                }
                output.clear();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        Self { task, state }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.task.abort();
        self.state.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use livekit::webrtc::{prelude::*, video_source::native::NativeVideoSource};

    #[tokio::test]
    async fn preview_is_opt_in_bounded_and_clears_pixels_when_disabled() {
        let source = NativeVideoSource::new(
            VideoResolution {
                width: 3440,
                height: 1440,
            },
            true,
        );
        let track = LocalVideoTrack::create_video_track(
            "synthetic local preview",
            RtcVideoSource::Native(source.clone()),
        );
        let state = State::default();
        let worker = Worker::start(track, state.clone());
        let feed = || {
            let mut buffer = I420Buffer::new(3440, 1440);
            let (y, u, v) = buffer.data_mut();
            y.fill(82);
            u.fill(90);
            v.fill(240);
            source.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, buffer));
        };
        feed();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(state.take().is_none());
        state.set_enabled(true);
        let frame = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                feed();
                tokio::time::sleep(Duration::from_millis(40)).await;
                if let Some(frame) = state.take() {
                    break frame;
                }
            }
        })
        .await
        .unwrap();
        assert!(frame.width <= 1280 && frame.height <= 720);
        assert!((frame.width as f32 / frame.height as f32 - 3440. / 1440.).abs() < 0.02);
        assert!(frame.bgra[2] > 180);
        state.set_enabled(false);
        feed();
        tokio::time::sleep(Duration::from_millis(220)).await;
        assert!(state.take().is_none());
        drop(worker);
        assert!(state.take().is_none());
    }
}
