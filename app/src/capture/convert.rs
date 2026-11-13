use anyhow::ensure;
use livekit::webrtc::{native::yuv_helper, prelude::I420Buffer};
use telinha_shared::Quality;

// libyuv is already linked by the pinned webrtc-sys dependency. Its C API
// operates on byte-order BGRA (called ARGB by libyuv on little-endian hosts).
// https://chromium.googlesource.com/libyuv/libyuv/+/refs/heads/main/include/libyuv/scale_argb.h
unsafe extern "C" {
    fn ARGBScale(
        src: *const u8,
        src_stride: i32,
        src_width: i32,
        src_height: i32,
        dst: *mut u8,
        dst_stride: i32,
        dst_width: i32,
        dst_height: i32,
        filter: i32,
    ) -> i32;
}

#[derive(Default)]
pub(super) struct Converter {
    scaled_bgra: Vec<u8>,
}

impl Converter {
    pub(super) fn convert(
        &mut self,
        w: i32,
        h: i32,
        stride: u32,
        data: &[u8],
        quality: Quality,
    ) -> anyhow::Result<I420Buffer> {
        ensure!(
            (1..=16384).contains(&w) && (1..=16384).contains(&h),
            "Dimensões de captura inválidas."
        );
        ensure!(
            stride >= w as u32 * 4
                && stride <= i32::MAX as u32
                && (stride as usize)
                    .checked_mul(h as usize)
                    .is_some_and(|len| data.len() >= len),
            "O quadro de captura chegou incompleto."
        );
        let (width, height) = quality.fit(w as u32, h as u32);
        // Small resizes benefit from libyuv's specialized I420 paths. For a
        // large reduction, avoid allocating/converting a full-resolution I420
        // frame: shrink BGRA first, reusing scratch storage. Odd source sizes
        // retain the established path, including all edge pixels.
        if w % 2 == 0 && h % 2 == 0 && w as u64 * h as u64 >= width as u64 * height as u64 * 3 {
            // ARGBScale only has an exact box prefilter for certain ratios.
            // Above 2:1, first average each 2x2 block, then let I420Scale finish
            // the reduction. A single arbitrary-ratio ARGB resize would alias
            // fine text/patterns (notably 5120x1440 -> 1920x540).
            let (work_width, work_height) = if w as u32 >= width * 2 && h as u32 >= height * 2 {
                (w as u32 / 2, h as u32 / 2)
            } else {
                (width, height)
            };
            let packed_stride = work_width * 4;
            self.scaled_bgra
                .resize(packed_stride as usize * work_height as usize, 0);
            // SAFETY: dimensions/strides are positive and fit i32; both slices
            // cover every row, are disjoint, and outlive this synchronous call.
            // 3 is libyuv::kFilterBox; no nearest-neighbor quality shortcut.
            let status = unsafe {
                ARGBScale(
                    data.as_ptr(),
                    stride as i32,
                    w,
                    h,
                    self.scaled_bgra.as_mut_ptr(),
                    packed_stride as i32,
                    work_width as i32,
                    work_height as i32,
                    3,
                )
            };
            ensure!(status == 0, "Não deu para reduzir a imagem da captura.");
            let mut buffer = to_i420(work_width, work_height, packed_stride, &self.scaled_bgra);
            return Ok(if (work_width, work_height) == (width, height) {
                buffer
            } else {
                buffer.scale(width as i32, height as i32)
            });
        }
        let mut buffer = to_i420(w as u32, h as u32, stride, data);
        Ok(if (width, height) == (w as u32, h as u32) {
            buffer
        } else {
            buffer.scale(width as i32, height as i32)
        })
    }
}

fn to_i420(w: u32, h: u32, stride: u32, data: &[u8]) -> I420Buffer {
    let mut buffer = I420Buffer::new(w, h);
    let (sy, su, sv) = buffer.strides();
    let (y, u, v) = buffer.data_mut();
    yuv_helper::argb_to_i420(data, stride, y, sy, u, su, v, sv, w as i32, h as i32);
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use livekit::webrtc::video_frame::VideoBuffer;
    use std::{hint::black_box, time::Instant};

    fn bars(w: usize, h: usize, stride: usize) -> Vec<u8> {
        let mut pixels = vec![99; stride * h];
        for row in pixels.chunks_exact_mut(stride) {
            for (x, pixel) in row[..w * 4].chunks_exact_mut(4).enumerate() {
                pixel.copy_from_slice(if x < w / 3 {
                    &[0, 0, 255, 255]
                } else if x < 2 * w / 3 {
                    &[0, 255, 0, 255]
                } else {
                    &[255, 0, 0, 255]
                });
            }
        }
        pixels
    }

    #[test]
    fn ultrawide_downscale_preserves_colors_edges_and_reuses_scratch() {
        let mut converter = Converter::default();
        for (w, h, width, height, scratch_len) in [
            (3440, 1440, 1920, 802, 1920 * 802 * 4),
            (5120, 1440, 1920, 540, 2560 * 720 * 4),
        ] {
            let stride = w * 4 + 32;
            let data = bars(w as usize, h as usize, stride as usize);
            for pass in 0..2 {
                let address = converter.scaled_bgra.as_ptr();
                let buffer = converter
                    .convert(w, h, stride as u32, &data, Quality::FullHd30)
                    .unwrap();
                assert_eq!((buffer.width(), buffer.height()), (width, height));
                assert_eq!(converter.scaled_bgra.len(), scratch_len);
                if pass > 0 {
                    assert_eq!(address, converter.scaled_bgra.as_ptr());
                }
                let (y, u, v) = buffer.data();
                let (sy, su, sv) = buffer.strides();
                for row in [0, height / 2, height - 1] {
                    for (x, expected) in [
                        (2, (82, 90, 240)),
                        (width / 2, (144, 54, 34)),
                        (width - 3, (41, 240, 110)),
                    ] {
                        let actual = [
                            y[(row * sy + x) as usize],
                            u[((row / 2) * su + x / 2) as usize],
                            v[((row / 2) * sv + x / 2) as usize],
                        ];
                        for (actual, expected) in
                            actual.into_iter().zip([expected.0, expected.1, expected.2])
                        {
                            assert!(
                                actual.abs_diff(expected) <= 3,
                                "color mismatch: {actual} vs {expected}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn large_reductions_average_fine_detail_instead_of_aliasing() {
        let (w, h) = (5120usize, 1440usize);
        let mut data = vec![255; w * h * 4];
        for (y, row) in data.chunks_exact_mut(w * 4).enumerate() {
            for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
                pixel[..3].fill(if (x + y) % 2 == 0 { 0 } else { 255 });
            }
        }
        let mut converter = Converter::default();
        let buffer = converter
            .convert(w as i32, h as i32, w as u32 * 4, &data, Quality::FullHd30)
            .unwrap();
        assert_eq!((buffer.width(), buffer.height()), (1920, 540));
        // A one-pixel checkerboard should become uniform mid-gray. Direct
        // arbitrary-ratio ARGBScale leaves a strong moire pattern here.
        assert!(
            buffer
                .data()
                .0
                .iter()
                .all(|luma| (125..=127).contains(luma))
        );
    }

    #[test]
    fn conversion_rejects_invalid_frames_and_keeps_small_sources_on_direct_path() {
        let mut converter = Converter::default();
        for (w, h, stride, data) in [
            (0, 4, 16, vec![0; 64]),
            (16385, 2, 65540, vec![]),
            (4, 4, 15, vec![0; 60]),
            (4, 4, 16, vec![0; 63]),
            (4, 4, u32::MAX, vec![]),
        ] {
            assert!(
                converter
                    .convert(w, h, stride, &data, Quality::FullHd30)
                    .is_err()
            );
        }
        let data = bars(321, 181, 1312);
        let buffer = converter
            .convert(321, 181, 1312, &data, Quality::FullHd30)
            .unwrap();
        assert_eq!((buffer.width(), buffer.height()), (320, 180));
        assert!(converter.scaled_bgra.is_empty());
    }

    // Run explicitly in CI. Timing is reported, never asserted: runner load
    // varies, and this measures preparation only, not capture/encoding/network.
    #[test]
    #[ignore = "synthetic capture processing benchmark"]
    fn benchmark_ultrawide_processing() {
        println!(
            "capture benchmark: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        for (w, h) in [
            (1920, 1080),
            (2560, 1080),
            (3440, 1440),
            (5120, 1440),
            (3840, 2160),
        ] {
            let data = bars(w as usize, h as usize, w as usize * 4);
            let (width, height) = Quality::FullHd30.fit(w, h);
            let mut converter = Converter::default();
            let mut previous = Vec::new();
            let mut current = Vec::new();
            for round in 0..7 {
                // Alternate ordering so thermal/load changes do not always
                // favor one implementation. Discard the first warm-up round.
                for optimized in if round % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let start = Instant::now();
                    for _ in 0..30 {
                        let buffer = if optimized {
                            converter
                                .convert(
                                    w as i32,
                                    h as i32,
                                    w * 4,
                                    black_box(&data),
                                    Quality::FullHd30,
                                )
                                .unwrap()
                        } else {
                            let mut buffer = to_i420(w, h, w * 4, black_box(&data));
                            if (w, h) == (width, height) {
                                buffer
                            } else {
                                buffer.scale(width as i32, height as i32)
                            }
                        };
                        black_box(buffer);
                    }
                    if round > 0 {
                        let samples = if optimized {
                            &mut current
                        } else {
                            &mut previous
                        };
                        samples.push(start.elapsed().as_secs_f64() * 1000. / 30.);
                    }
                }
            }
            previous.sort_by(f64::total_cmp);
            current.sort_by(f64::total_cmp);
            let before = previous[previous.len() / 2];
            let after = current[current.len() / 2];
            println!(
                "{w}x{h} -> {width}x{height}: before={before:.3}ms after={after:.3}ms reduction={:.1}% scratch={} bytes",
                (1. - after / before) * 100.,
                converter.scaled_bgra.len()
            );
        }
    }
}
