use anyhow::ensure;
use nokhwa::{
    pixel_format::{FormatDecoder, RgbAFormat},
    utils::{CameraFormat, FrameFormat, Resolution},
};
use std::collections::HashMap;

pub(super) fn enumerate(
    mut query: impl FnMut(FrameFormat) -> anyhow::Result<HashMap<Resolution, Vec<u32>>>,
) -> anyhow::Result<Vec<CameraFormat>> {
    let mut formats = Vec::new();
    let mut queried = false;
    // Nokhwa 0.10.11's Media Foundation compatible_fourcc() stops at two
    // pixel formats. Query each decoder format directly, including MJPEG/NV12.
    for &pixel in RgbAFormat::FORMATS {
        match query(pixel) {
            Ok(resolutions) => {
                queried = true;
                for (resolution, rates) in resolutions {
                    formats.extend(
                        rates
                            .into_iter()
                            .map(|fps| CameraFormat::new(resolution, pixel, fps)),
                    );
                }
            }
            Err(error) => eprintln!("telinha webcam: consulta {pixel} falhou: {error}"),
        }
    }
    ensure!(
        queried,
        "Não deu para consultar os formatos da webcam. Confira a conexão e o driver."
    );
    Ok(formats)
}

pub(super) fn candidates(formats: &[CameraFormat]) -> Vec<CameraFormat> {
    let mut choices: Vec<_> = formats
        .iter()
        .copied()
        .filter(|format| {
            RgbAFormat::FORMATS.contains(&format.format())
                && (2..=4096).contains(&format.width())
                && (2..=2160).contains(&format.height())
                && (1..=60).contains(&format.frame_rate())
        })
        .collect();
    choices.sort_by_key(|format| {
        let fps = format.frame_rate();
        let rate = if (24..=30).contains(&fps) {
            0
        } else if fps > 30 {
            1
        } else {
            2
        };
        // Prefer up to 1080p near 30 fps. Only use 1440p/4K if the smaller
        // advertised modes are absent or fail; the worker reduces the frames.
        let larger = u8::from(format.width() > 1920 || format.height() > 1080);
        let distance = format.width().abs_diff(1920) + format.height().abs_diff(1080);
        let pixel = match format.format() {
            FrameFormat::MJPEG => 0, // Less USB bandwidth, especially at 1080p/4K.
            FrameFormat::NV12 => 1,
            FrameFormat::YUYV => 2,
            FrameFormat::RAWRGB => 3,
            _ => 4,
        };
        (
            larger,
            rate,
            distance,
            fps.abs_diff(30),
            pixel,
            format.width(),
            format.height(),
        )
    });
    choices.dedup();
    choices
}

pub(super) fn try_candidates<T>(
    choices: &[CameraFormat],
    mut attempt: impl FnMut(CameraFormat) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    ensure!(
        !choices.is_empty(),
        "Não foi encontrado um formato de vídeo compatível. Atualize o driver da webcam e tente novamente."
    );
    for &format in choices {
        match attempt(format) {
            Ok(value) => return Ok(value),
            Err(error) => eprintln!("telinha webcam: modo {format} recusado: {error:#}"),
        }
    }
    anyhow::bail!(
        "A webcam não iniciou em nenhum dos modos disponíveis. Feche outros aplicativos que usam a câmera e tente novamente."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn format(w: u32, h: u32, pixel: FrameFormat, fps: u32) -> CameraFormat {
        CameraFormat::new(Resolution::new(w, h), pixel, fps)
    }

    #[test]
    fn finds_1080p_in_the_third_pixel_format_and_skips_query_failures() {
        let available = enumerate(|pixel| match pixel {
            FrameFormat::MJPEG => Ok(HashMap::from([(Resolution::new(3840, 2160), vec![30])])),
            FrameFormat::YUYV => anyhow::bail!("unavailable"),
            FrameFormat::NV12 => Ok(HashMap::from([(Resolution::new(1920, 1080), vec![30])])),
            _ => Ok(HashMap::new()),
        })
        .unwrap();
        assert_eq!(
            candidates(&available)[0],
            format(1920, 1080, FrameFormat::NV12, 30)
        );
        assert!(enumerate(|_| anyhow::bail!("disconnected")).is_err());
    }

    #[test]
    fn prefers_1080p30_then_smaller_modes_then_native_4k_without_inventing_modes() {
        let f = |w, h, fps| format(w, h, FrameFormat::MJPEG, fps);
        let available = [
            f(3840, 2160, 30),
            f(1280, 720, 60),
            f(640, 480, 30),
            f(1920, 1080, 30),
            f(1280, 720, 30),
            f(1920, 1080, 30),
        ];
        let choices = candidates(&available);
        assert_eq!(
            choices,
            vec![
                available[3],
                available[4],
                available[2],
                available[1],
                available[0]
            ]
        );
        assert_eq!(candidates(&available[..1]), vec![available[0]]);
        assert!(candidates(&[f(7680, 4320, 30), f(1920, 1080, 0), f(1920, 1080, 240)]).is_empty());
    }

    #[test]
    fn rejected_1080p_falls_back_and_an_accepted_mode_stops_retries() {
        let full = format(1920, 1080, FrameFormat::MJPEG, 30);
        let hd = format(1280, 720, FrameFormat::NV12, 30);
        let low = format(640, 480, FrameFormat::YUYV, 30);
        let mut attempts = Vec::new();
        let selected = try_candidates(&[full, hd, low], |candidate| {
            attempts.push(candidate);
            ensure!(candidate != full, "driver rejects this mode");
            Ok(candidate)
        })
        .unwrap();
        assert_eq!(selected, hd);
        assert_eq!(attempts, vec![full, hd]);
        assert!(try_candidates::<()>(&[], |_| panic!("no fabricated mode")).is_err());
    }
}
