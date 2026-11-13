//! Video-only webcam capture. The native driver lives in a child of this bundle:
//! cancelling a blocked driver or losing a device cannot take down screen sharing.
use anyhow::{Context, ensure};
use livekit::webrtc::{
    native::yuv_helper,
    prelude::{I420Buffer, VideoFormatType, VideoFrame, VideoResolution, VideoRotation},
    video_frame::native::VideoFrameBufferExt,
    video_source::native::NativeVideoSource,
};
use nokhwa::{
    Camera,
    buffer::Buffer,
    pixel_format::RgbAFormat,
    utils::{ApiBackend, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

mod formats;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Device {
    pub name: String,
    // Keep numeric and opaque device IDs distinct, including numeric-looking IDs.
    index: Option<u32>,
    id: Option<String>,
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

struct Worker {
    child: Child,
    stopped: Arc<AtomicBool>,
}
impl Worker {
    fn spawn(mode: &str, device: Option<&Device>) -> anyhow::Result<Self> {
        let executable = std::env::current_exe()?;
        let mut command = Command::new(&executable);
        #[cfg(target_os = "macos")]
        let native_helper = executable.with_file_name("telinha-camera");
        #[cfg(target_os = "macos")]
        if mode != "probe" || native_helper.exists() {
            ensure!(
                native_helper.exists(),
                "Abra o pacote telinha.app para usar a webcam no Mac."
            );
            command = Command::new(native_helper);
            command.arg(mode);
        } else {
            command.args(["--camera-worker", mode]);
        }
        #[cfg(not(target_os = "macos"))]
        command.args(["--camera-worker", mode]);
        if let Some(device) = device {
            command.arg(serde_json::to_string(device)?);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        Ok(Self {
            child: command.spawn().context("Não deu pra preparar a webcam.")?,
            stopped: Arc::new(AtomicBool::new(false)),
        })
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Called only when the person opens the webcam picker, never at app startup.
pub async fn devices() -> anyhow::Result<Vec<Device>> {
    let mut worker = Worker::spawn("list", None)?;
    let output = worker.child.stdout.take().context("Webcam sem resposta.")?;
    let read = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Device>> {
        let mut bytes = Vec::new();
        output.take(65_537).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 65_536, "Lista de webcams inválida.");
        let response: Result<Vec<Device>, String> = serde_json::from_slice(&bytes)
            .context("Não deu pra listar as webcams. Confira a permissão de câmera.")?;
        response.map_err(anyhow::Error::msg)
    });
    tokio::time::timeout(Duration::from_secs(60), read)
        .await
        .context(
            "A webcam demorou para responder. Confira a permissão de câmera e tente novamente.",
        )??
}

pub struct Capture {
    pub source: NativeVideoSource,
    pub preview: Arc<Mutex<Option<Frame>>>,
    _worker: Worker,
}
impl Capture {
    pub async fn start(
        device: Device,
        request: u64,
        faults: mpsc::UnboundedSender<(u64, String)>,
    ) -> anyhow::Result<Self> {
        let mut worker = Worker::spawn("capture", Some(&device))?;
        let mut output = worker.child.stdout.take().context("Webcam sem resposta.")?;
        let stopped = worker.stopped.clone();
        let preview = Arc::new(Mutex::new(None));
        let thread_preview = preview.clone();
        let runtime = tokio::runtime::Handle::current();
        let (ready, first_frame) = oneshot::channel();
        std::thread::Builder::new()
            .name("telinha-camera-frames".into())
            .spawn(move || {
                let mut ready = Some(ready);
                let result = (|| -> anyhow::Result<()> {
                    // LiveKit creates a Tokio keepalive task on its first frame.
                    let _runtime = runtime.enter();
                    let mut source: Option<NativeVideoSource> = None;
                    let mut last_preview = Instant::now() - Duration::from_secs(1);
                    loop {
                        let frame = read_frame(&mut output)?;
                        if stopped.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        let mut buffer = to_i420(&frame)?;
                        let (width, height) = fit(frame.width, frame.height, 1280, 720);
                        let mut buffer = if (width, height) == (frame.width, frame.height) {
                            buffer
                        } else {
                            buffer.scale(width as i32, height as i32)
                        };
                        let source = source.get_or_insert_with(|| {
                            NativeVideoSource::new(VideoResolution { width, height }, false)
                        });
                        if last_preview.elapsed() >= Duration::from_millis(66) {
                            let (w, h) = fit(width, height, 640, 480);
                            let small = buffer.scale(w as i32, h as i32);
                            let mut bgra = vec![0; w as usize * h as usize * 4];
                            small.to_argb(
                                VideoFormatType::ARGB,
                                &mut bgra,
                                w * 4,
                                w as i32,
                                h as i32,
                            );
                            *thread_preview.lock() = Some(Frame {
                                width: w,
                                height: h,
                                bgra,
                            });
                            last_preview = Instant::now();
                        }
                        source
                            .capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, buffer));
                        if let Some(ready) = ready.take() {
                            let _ = ready.send(Ok(source.clone()));
                        }
                    }
                })();
                if let Err(error) = result
                    && !stopped.load(Ordering::Acquire)
                {
                    let message = error.to_string();
                    if let Some(ready) = ready {
                        let _ = ready.send(Err(message));
                    } else {
                        let _ = faults.send((request, message));
                    }
                }
            })?;
        let source = tokio::time::timeout(Duration::from_secs(20), first_frame).await
            .context("A webcam não enviou vídeo. Confira se está conectada e se outro app está usando a câmera.")?
            .context("A captura da webcam terminou antes de começar.")?
            .map_err(anyhow::Error::msg)?;
        Ok(Self {
            source,
            preview,
            _worker: worker,
        })
    }
}

pub fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let scale = (max_width as f64 / width.max(1) as f64)
        .min(max_height as f64 / height.max(1) as f64)
        .min(1.);
    (
        ((width as f64 * scale) as u32 / 2 * 2).max(2),
        ((height as f64 * scale) as u32 / 2 * 2).max(2),
    )
}

fn frame_len(width: u32, height: u32) -> anyhow::Result<usize> {
    ensure!(
        (2..=4096).contains(&width) && (2..=2160).contains(&height),
        "A webcam enviou dimensões inválidas."
    );
    Ok(width as usize * height as usize * 4)
}

fn read_frame(input: &mut impl Read) -> anyhow::Result<Frame> {
    let mut header = [0; 8];
    input
        .read_exact(&mut header)
        .context("A webcam foi desconectada ou parou de responder.")?;
    let width = u32::from_le_bytes(header[..4].try_into()?);
    let height = u32::from_le_bytes(header[4..].try_into()?);
    if width == 0 {
        ensure!(height <= 8192, "Resposta inválida da webcam.");
        let mut message = vec![0; height as usize];
        input.read_exact(&mut message)?;
        anyhow::bail!(String::from_utf8_lossy(&message).into_owned());
    }
    let mut bgra = vec![0; frame_len(width, height)?];
    input
        .read_exact(&mut bgra)
        .context("O vídeo da webcam foi interrompido.")?;
    Ok(Frame {
        width,
        height,
        bgra,
    })
}

fn to_i420(frame: &Frame) -> anyhow::Result<I420Buffer> {
    ensure!(
        frame.bgra.len() == frame_len(frame.width, frame.height)?,
        "Quadro incompleto da webcam."
    );
    let mut buffer = I420Buffer::new(frame.width, frame.height);
    let (sy, su, sv) = buffer.strides();
    let (y, u, v) = buffer.data_mut();
    yuv_helper::argb_to_i420(
        &frame.bgra,
        frame.width * 4,
        y,
        sy,
        u,
        su,
        v,
        sv,
        frame.width as i32,
        frame.height as i32,
    );
    Ok(buffer)
}

fn permission() -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    nokhwa::nokhwa_initialize(move |granted| {
        let _ = tx.send(granted);
    });
    ensure!(
        rx.recv_timeout(Duration::from_secs(55)).unwrap_or(false),
        "Permita o acesso à câmera nas configurações de privacidade do sistema e tente novamente."
    );
    Ok(())
}

fn list_native() -> anyhow::Result<Vec<Device>> {
    permission()?;
    Ok(nokhwa::query(ApiBackend::Auto)
        .context("Não deu pra listar as webcams.")?
        .into_iter()
        .take(64)
        .map(|camera| {
            let (index, id) = match camera.index() {
                CameraIndex::Index(index) => (Some(*index), None),
                CameraIndex::String(id) => (None, Some(id.clone())),
            };
            Device {
                name: camera.human_name(),
                index,
                id,
            }
        })
        .collect())
}

fn open_native(device: Device) -> anyhow::Result<(Camera, Buffer)> {
    let index = match (device.index, device.id) {
        (Some(index), None) => CameraIndex::Index(index),
        (None, Some(id)) => CameraIndex::String(id),
        _ => anyhow::bail!("Webcam inválida. Atualize a lista de câmeras."),
    };
    let mut camera = Camera::new(
        index,
        RequestedFormat::new::<RgbAFormat>(RequestedFormatType::None),
    )
    .context("Não deu pra abrir a webcam. Confira a conexão e a permissão de câmera.")?;
    let available = formats::enumerate(|pixel| Ok(camera.compatible_list_by_resolution(pixel)?))?;
    let choices = formats::candidates(&available);
    eprintln!(
        "telinha webcam: {} modos consultados, {} candidatos até 4K",
        available.len(),
        choices.len()
    );
    let first = formats::try_candidates(&choices, |format| {
        let _ = camera.stop_stream();
        // set_camera_requset re-enumerates the truncated two-format list.
        // This mode came from the complete per-pixel-format native enumeration.
        #[allow(deprecated)]
        camera.set_camera_format(format)?;
        camera.open_stream()?;
        let frame = camera.frame()?;
        // A successful format setter alone does not prove the driver produces
        // usable frames. Try the next advertised mode if reading/decoding fails.
        decode_frame(&frame)?;
        eprintln!(
            "telinha webcam: captura negociada em {}",
            camera.camera_format()
        );
        Ok(frame)
    })?;
    Ok((camera, first))
}

fn decode_frame(frame: &Buffer) -> anyhow::Result<Frame> {
    frame_len(frame.resolution().width(), frame.resolution().height())?;
    let pixels = if frame.source_frame_format() == FrameFormat::MJPEG {
        let mut reader = image::ImageReader::with_format(
            std::io::Cursor::new(frame.buffer()),
            image::ImageFormat::Jpeg,
        );
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(2160);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        reader
            .decode()
            .context("Não deu pra ler a imagem JPEG desta webcam.")?
            .into_rgba8()
    } else {
        frame
            .decode_image::<RgbAFormat>()
            .context("Não deu pra ler a imagem desta webcam.")?
    };
    let (width, height) = pixels.dimensions();
    frame_len(width, height)?;
    let mut bgra = pixels.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    normalize_frame(Frame {
        width,
        height,
        bgra,
    })
}

fn normalize_frame(frame: Frame) -> anyhow::Result<Frame> {
    ensure!(
        frame.bgra.len() == frame_len(frame.width, frame.height)?,
        "Quadro incompleto da webcam."
    );
    // Capture may need a native 4K mode, but never send 4K frames through the
    // child pipe or raise the webcam's existing 720p/30 transmission budget.
    let (width, height) = fit(frame.width, frame.height, 1280, 720);
    if (width, height) == (frame.width, frame.height) {
        return Ok(frame);
    }
    let scaled = to_i420(&frame)?.scale(width as i32, height as i32);
    let mut bgra = vec![0; frame_len(width, height)?];
    scaled.to_argb(
        VideoFormatType::ARGB,
        &mut bgra,
        width * 4,
        width as i32,
        height as i32,
    );
    Ok(Frame {
        width,
        height,
        bgra,
    })
}

fn capture_native(device: Device, output: &mut impl Write) -> anyhow::Result<()> {
    permission()?;
    // Drain the driver's callback queue even if encoding or the pipe is slower.
    // Keep at most one waiting frame, so a slow receiver cannot build a backlog.
    let (send, frames) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("telinha-camera-driver".into())
        .spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let (mut camera, first) = open_native(device)?;
                let mut first = Some(first);
                loop {
                    let frame = if let Some(first) = first.take() {
                        first
                    } else {
                        camera
                            .frame()
                            .context("A webcam foi desconectada ou parou de responder.")?
                    };
                    match send.try_send(Ok(frame)) {
                        Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                let _ = send.send(Err(error));
            }
        })?;
    let mut previous = Instant::now() - Duration::from_secs(1);
    loop {
        let frame = frames
            .recv_timeout(Duration::from_secs(10))
            .context("A webcam parou de enviar vídeo. Tente ligá-la novamente.")??;
        if previous.elapsed() < Duration::from_millis(32) {
            continue;
        }
        let frame = decode_frame(&frame)?;
        output.write_all(&frame.width.to_le_bytes())?;
        output.write_all(&frame.height.to_le_bytes())?;
        output.write_all(&frame.bgra)?;
        output.flush()?;
        previous = Instant::now();
    }
}

/// Internal entry point, before GPUI, account restoration or logging to stdout.
pub fn worker_main() -> anyhow::Result<()> {
    // An app crash/force quit closes this pipe too: never orphan an active webcam.
    std::thread::spawn(|| {
        let mut byte = [0];
        let mut input = std::io::stdin().lock();
        while matches!(input.read(&mut byte), Ok(1)) {}
        exit_worker();
    });
    let mut output = std::io::stdout().lock();
    match std::env::args().nth(2).as_deref() {
        Some("probe") => {
            // Test child pipes and parent-death cleanup without opening a device.
            output.write_all(&2u32.to_le_bytes())?;
            output.write_all(&2u32.to_le_bytes())?;
            output.write_all(&[0, 0, 255, 255].repeat(4))?;
            output.flush()?;
            loop {
                std::thread::park();
            }
        }
        Some("list") => {
            let result = list_native().map_err(|error| error.to_string());
            serde_json::to_writer(&mut output, &result)?;
        }
        Some("capture") => {
            let result = (|| {
                let device = serde_json::from_str(
                    &std::env::args().nth(3).context("Webcam não escolhida.")?,
                )?;
                capture_native(device, &mut output)
            })();
            if let Err(error) = result {
                let message = error.to_string();
                let bytes = message.as_bytes();
                output.write_all(&0u32.to_le_bytes())?;
                output.write_all(&(bytes.len() as u32).to_le_bytes())?;
                output.write_all(bytes)?;
            }
        }
        _ => anyhow::bail!("Modo de webcam inválido."),
    }
    Ok(())
}

fn exit_worker() -> ! {
    // std::process::exit flushes stdout and can wait for the capture thread's
    // locked pipe. OS process exit releases the driver without waiting for it.
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn _exit(status: i32) -> !;
        }
        unsafe { _exit(0) }
    }
    #[cfg(target_os = "windows")]
    unsafe {
        windows::Win32::System::Threading::ExitProcess(0)
    }
}

pub fn probe() -> anyhow::Result<()> {
    tokio::runtime::Runtime::new()?.block_on(async {
        let mut worker = Worker::spawn("probe", None)?;
        let mut output = worker.child.stdout.take().context("Webcam worker sem pipe de saída.")?;
        let read = tokio::task::spawn_blocking(move || read_frame(&mut output));
        let frame = tokio::time::timeout(Duration::from_secs(5), read).await
            .context("O processo de webcam não respondeu.")???;
        ensure!(frame.width == 2 && frame.height == 2 && frame.bgra == [0, 0, 255, 255].repeat(4), "Protocolo do processo de webcam inválido.");
        // Simulate the OS closing the parent's pipes on quit or crash.
        drop(worker.child.stdin.take());
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(status) = worker.child.try_wait()? {
                    ensure!(status.success(), "O processo de webcam não encerrou corretamente.");
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.context("O processo de webcam sobreviveu ao fechamento do app.")??;
        println!("PASS: processo isolado, quadro sintético e encerramento ao perder o app; nenhuma câmera acessada.");
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nokhwa::utils::Resolution;

    #[test]
    fn camera_packets_reject_invalid_dimensions_and_truncated_frames() {
        let mut bad = [0xff; 8].as_slice();
        assert!(read_frame(&mut bad).is_err());
        let mut bytes = 640u32.to_le_bytes().to_vec();
        bytes.extend(480u32.to_le_bytes());
        bytes.extend([0; 16]);
        assert!(read_frame(&mut bytes.as_slice()).is_err());
        assert!(
            to_i420(&Frame {
                width: 2,
                height: 2,
                bgra: vec![]
            })
            .is_err()
        );
    }

    #[test]
    fn camera_bgra_to_yuv_preserves_red_and_aspect_ratio() {
        let frame = Frame {
            width: 4,
            height: 2,
            bgra: [0, 0, 255, 255].repeat(8),
        };
        let buffer = to_i420(&frame).unwrap();
        let mut pixels = vec![0; 32];
        buffer.to_argb(VideoFormatType::ARGB, &mut pixels, 16, 4, 2);
        assert!(pixels[2] > 240 && pixels[0] < 10 && pixels[1] < 10);
        assert_eq!(fit(1920, 1080, 640, 480), (640, 360));
        assert_eq!(fit(640, 480, 1280, 720), (640, 480));
    }

    #[test]
    fn native_4k_is_reduced_before_the_child_pipe_and_invalid_frames_are_rejected() {
        let frame = normalize_frame(Frame {
            width: 3840,
            height: 2160,
            bgra: [0, 0, 255, 255].repeat(3840 * 2160),
        })
        .unwrap();
        assert_eq!(
            (frame.width, frame.height, frame.bgra.len()),
            (1280, 720, 1280 * 720 * 4)
        );
        assert!(frame.bgra[2] > 240 && frame.bgra[0] < 10);
        assert!(
            normalize_frame(Frame {
                width: 3840,
                height: 2160,
                bgra: vec![]
            })
            .is_err()
        );
        let low = normalize_frame(Frame {
            width: 640,
            height: 480,
            bgra: vec![0; 640 * 480 * 4],
        })
        .unwrap();
        assert_eq!((low.width, low.height), (640, 480));
        let oversized = Buffer::new(Resolution::new(7680, 4320), &[], FrameFormat::MJPEG);
        assert!(decode_frame(&oversized).is_err());
        let truncated = Buffer::new(Resolution::new(1280, 720), &[0; 16], FrameFormat::MJPEG);
        assert!(decode_frame(&truncated).is_err());
    }

    #[test]
    fn jpeg_and_nv12_decode_and_jpeg_header_limits_apply_before_allocation() {
        let nv12 = Buffer::new(
            Resolution::new(2, 2),
            &[82, 82, 82, 82, 90, 240],
            FrameFormat::NV12,
        );
        let red = decode_frame(&nv12).unwrap();
        assert!(red.bgra[2] > 200 && red.bgra[0] < 60 && red.bgra[1] < 60);
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(
                &[255, 0, 0].repeat(16),
                4,
                4,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        let frame = decode_frame(&Buffer::new(
            Resolution::new(4, 4),
            &jpeg,
            FrameFormat::MJPEG,
        ))
        .unwrap();
        assert_eq!((frame.width, frame.height), (4, 4));
        assert!(frame.bgra[2] > 200 && frame.bgra[0] < 60);
        // The compressed header, not just the driver's declared resolution,
        // must be bounded before a JPEG decoder allocates its output.
        let sof = jpeg
            .windows(2)
            .position(|bytes| bytes == [0xff, 0xc0])
            .unwrap();
        jpeg[sof + 5..sof + 7].copy_from_slice(&4320u16.to_be_bytes());
        jpeg[sof + 7..sof + 9].copy_from_slice(&7680u16.to_be_bytes());
        let error = decode_frame(&Buffer::new(
            Resolution::new(4, 4),
            &jpeg,
            FrameFormat::MJPEG,
        ))
        .err()
        .unwrap();
        assert!(matches!(
            error.downcast_ref::<image::ImageError>(),
            Some(image::ImageError::Limits(_))
        ));
    }
}
