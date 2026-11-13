//! One local snapshot per source. Capture runs in disposable child processes;
//! closing the picker cancels them without touching the published screen.
use crate::capture::Source;
use anyhow::{Context, ensure};
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    time::Duration,
};

pub const MAX_PREVIEWS: usize = 96;
const MAX_PNG_BYTES: u64 = 512 * 1024;

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub async fn snapshot(source: Source) -> anyhow::Result<image::RgbaImage> {
    use livekit::webrtc::desktop_capturer::DesktopCaptureSourceType as Kind;
    let kind = match source.kind {
        Kind::Window => "window",
        Kind::Screen => "screen",
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        Kind::Generic => anyhow::bail!("A prévia é mostrada pelo sistema."),
    };
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--source-preview",
            kind,
            &source.id.context("Fonte sem identificação.")?.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut worker = Worker(command.spawn()?);
    let output = worker.0.stdout.take().context("Prévia sem resposta.")?;
    let read = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        output.take(MAX_PNG_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= MAX_PNG_BYTES, "Prévia grande demais.");
        decode(&bytes)
    });
    tokio::time::timeout(Duration::from_secs(3), read)
        .await
        .context("A janela não enviou uma prévia.")??
}

fn decode(bytes: &[u8]) -> anyhow::Result<image::RgbaImage> {
    let (width, height) =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png)
            .into_dimensions()?;
    ensure!(
        (1..=320).contains(&width) && (1..=180).contains(&height),
        "Dimensões de prévia inválidas."
    );
    let mut pixels =
        image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?.into_rgba8();
    // GPUI's RenderImage consumes BGRA, while PNG is encoded as RGBA.
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(pixels)
}

#[cfg(any(target_os = "windows", test))]
fn thumbnail(
    width: u32,
    height: u32,
    stride: usize,
    bytes: &[u8],
) -> anyhow::Result<image::RgbaImage> {
    ensure!(
        (1..=16384).contains(&width) && (1..=16384).contains(&height),
        "Dimensões inválidas."
    );
    let row = width as usize * 4;
    let required = stride
        .checked_mul(height as usize - 1)
        .and_then(|n| n.checked_add(row));
    ensure!(
        stride >= row && required.is_some_and(|n| bytes.len() >= n),
        "Quadro incompleto."
    );
    struct Bgra<'a> {
        width: u32,
        height: u32,
        stride: usize,
        bytes: &'a [u8],
    }
    impl image::GenericImageView for Bgra<'_> {
        type Pixel = image::Rgba<u8>;
        fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }
        fn get_pixel(&self, x: u32, y: u32) -> Self::Pixel {
            let n = y as usize * self.stride + x as usize * 4;
            image::Rgba([self.bytes[n + 2], self.bytes[n + 1], self.bytes[n], 255])
        }
    }
    let scale = (320. / width as f64).min(180. / height as f64).min(1.);
    Ok(image::imageops::thumbnail(
        &Bgra {
            width,
            height,
            stride,
            bytes,
        },
        (width as f64 * scale).round().max(1.) as u32,
        (height as f64 * scale).round().max(1.) as u32,
    ))
}

pub fn worker_main() -> anyhow::Result<()> {
    #[cfg(not(target_os = "windows"))]
    anyhow::bail!("No Mac, use as prévias do seletor do sistema.");
    #[cfg(target_os = "windows")]
    {
        std::thread::spawn(|| {
            let mut byte = [0];
            while matches!(std::io::stdin().read(&mut byte), Ok(1)) {}
            unsafe {
                windows::Win32::System::Threading::ExitProcess(0);
            }
        });
        use image::ImageEncoder;
        use livekit::webrtc::desktop_capturer::{
            CaptureError, DesktopCaptureSourceType as Kind, DesktopCapturer, DesktopCapturerOptions,
        };
        use std::{
            panic::{AssertUnwindSafe, catch_unwind},
            time::Instant,
        };
        let kind = match std::env::args().nth(2).as_deref() {
            Some("window") => Kind::Window,
            Some("screen") => Kind::Screen,
            _ => anyhow::bail!("Fonte inválida."),
        };
        let id: u64 = std::env::args()
            .nth(3)
            .context("Fonte não escolhida.")?
            .parse()?;
        let mut capturer = DesktopCapturer::new(DesktopCapturerOptions::new(kind))
            .context("Prévia indisponível.")?;
        let source = capturer
            .get_source_list()
            .into_iter()
            .find(|source| source.id() == id)
            .context("A janela foi fechada.")?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        capturer.start_capture(Some(source), move |frame| {
            let result = match frame {
                Ok(frame) => catch_unwind(AssertUnwindSafe(|| {
                    thumbnail(
                        frame.width() as u32,
                        frame.height() as u32,
                        frame.stride() as usize,
                        frame.data(),
                    )
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("A prévia falhou."))),
                Err(CaptureError::Temporary) => return,
                Err(CaptureError::Permanent) => Err(anyhow::anyhow!("Prévia indisponível.")),
            };
            let _ = sender.try_send(result);
        });
        let start = Instant::now();
        loop {
            capturer.capture_frame();
            if let Ok(result) = receiver.recv_timeout(Duration::from_millis(50)) {
                let pixels = result?;
                image::codecs::png::PngEncoder::new(std::io::stdout().lock()).write_image(
                    pixels.as_raw(),
                    pixels.width(),
                    pixels.height(),
                    image::ExtendedColorType::Rgba8,
                )?;
                return Ok(());
            }
            ensure!(
                start.elapsed() < Duration::from_millis(1500),
                "Prévia indisponível."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    #[test]
    fn preview_handles_padded_bgra_and_preserves_color_and_aspect() {
        let bytes = [
            0, 0, 255, 0, 0, 0, 255, 0, 99, 99, 99, 99, 0, 0, 255, 0, 0, 0, 255, 0,
        ];
        let small = thumbnail(2, 2, 12, &bytes).unwrap();
        assert!(small.pixels().all(|p| p.0 == [255, 0, 0, 255]));
        let large = [0, 0, 255, 0].repeat(640 * 480);
        let small = thumbnail(640, 480, 640 * 4, &large).unwrap();
        assert_eq!(small.dimensions(), (240, 180));
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(small.as_raw(), 240, 180, image::ExtendedColorType::Rgba8)
            .unwrap();
        assert!(
            decode(&png)
                .unwrap()
                .pixels()
                .all(|p| p.0 == [0, 0, 255, 255])
        );
    }

    #[test]
    fn preview_rejects_truncated_frames_and_oversized_pngs() {
        assert!(thumbnail(2, 2, 7, &[0; 32]).is_err());
        assert!(thumbnail(2, 2, 12, &[0; 19]).is_err());
        assert!(thumbnail(0, 2, 12, &[0; 24]).is_err());
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&vec![0; 321 * 4], 321, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        assert!(decode(&png).is_err());
    }
}
