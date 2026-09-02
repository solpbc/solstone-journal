use crate::RgbFrame;

const HASH_WIDTH: u32 = 9;
const HASH_HEIGHT: u32 = 8;
const RESAMPLE_PRECISION_BITS: i32 = 22;

/// Compute the 64-bit difference hash used by Python's `VideoProcessor`.
pub fn dhash(frame: &RgbFrame) -> u64 {
    let gray = resize_and_grayscale(frame);
    if gray.len() != (HASH_WIDTH * HASH_HEIGHT) as usize {
        return 0;
    }

    let mut hash = 0_u64;
    for row in 0..8 {
        for col in 0..8 {
            let index = row * 9 + col;
            if gray[index] > gray[index + 1] {
                hash |= 1_u64 << (row * 8 + col);
            }
        }
    }
    hash
}

pub(crate) fn resize_and_grayscale(frame: &RgbFrame) -> Vec<u8> {
    let Some(resized) = pillow_bilinear_resize(frame, HASH_WIDTH, HASH_HEIGHT) else {
        return Vec::new();
    };
    resized
        .chunks_exact(3)
        .map(|rgb| {
            let luma = u32::from(rgb[0]) * 19_595
                + u32::from(rgb[1]) * 38_470
                + u32::from(rgb[2]) * 7_471
                + 0x8000;
            (luma >> 16) as u8
        })
        .collect()
}

#[derive(Debug)]
struct AxisWeights {
    start: usize,
    weights: Vec<i32>,
}

fn pillow_bilinear_resize(
    frame: &RgbFrame,
    output_width: u32,
    output_height: u32,
) -> Option<Vec<u8>> {
    let input_width = usize::try_from(frame.width).ok()?;
    let input_height = usize::try_from(frame.height).ok()?;
    let output_width = usize::try_from(output_width).ok()?;
    let output_height = usize::try_from(output_height).ok()?;
    if input_width == 0 || input_height == 0 || output_width == 0 || output_height == 0 {
        return None;
    }

    let horizontal = pillow_bilinear_weights(input_width, output_width);
    let vertical = pillow_bilinear_weights(input_height, output_height);
    let mut intermediate = vec![0_u8; output_width.checked_mul(input_height)?.checked_mul(3)?];
    for (source_y, source_row) in frame
        .pixels
        .chunks_exact(input_width.checked_mul(3)?)
        .enumerate()
    {
        for (output_x, coefficients) in horizontal.iter().enumerate() {
            for channel in 0..3 {
                let mut sum = 1_i32 << (RESAMPLE_PRECISION_BITS - 1);
                for (offset, weight) in coefficients.weights.iter().enumerate() {
                    sum +=
                        i32::from(source_row[(coefficients.start + offset) * 3 + channel]) * weight;
                }
                intermediate[(source_y * output_width + output_x) * 3 + channel] = clip_8(sum);
            }
        }
    }

    let mut output = vec![0_u8; output_width.checked_mul(output_height)?.checked_mul(3)?];
    for (output_y, coefficients) in vertical.iter().enumerate() {
        for output_x in 0..output_width {
            for channel in 0..3 {
                let mut sum = 1_i32 << (RESAMPLE_PRECISION_BITS - 1);
                for (offset, weight) in coefficients.weights.iter().enumerate() {
                    sum += i32::from(
                        intermediate[((coefficients.start + offset) * output_width + output_x) * 3
                            + channel],
                    ) * weight;
                }
                output[(output_y * output_width + output_x) * 3 + channel] = clip_8(sum);
            }
        }
    }
    Some(output)
}

fn pillow_bilinear_weights(input_size: usize, output_size: usize) -> Vec<AxisWeights> {
    let scale = input_size as f64 / output_size as f64;
    let filter_scale = scale.max(1.0);
    let support = filter_scale;
    (0..output_size)
        .map(|output_index| {
            let center = (output_index as f64 + 0.5) * scale;
            let start = ((center - support + 0.5) as usize).min(input_size);
            let end = ((center + support + 0.5) as usize).min(input_size);
            let mut weights: Vec<f64> = (start..end)
                .map(|input_index| {
                    let distance = ((input_index as f64 - center + 0.5) / filter_scale).abs();
                    (1.0 - distance).max(0.0)
                })
                .collect();
            let total: f64 = weights.iter().sum();
            for weight in &mut weights {
                *weight /= total;
            }
            AxisWeights {
                start,
                weights: weights
                    .into_iter()
                    .map(|weight| {
                        (weight * f64::from(1_i32 << RESAMPLE_PRECISION_BITS) + 0.5) as i32
                    })
                    .collect(),
            }
        })
        .collect()
}

fn clip_8(sum: i32) -> u8 {
    let value = sum >> RESAMPLE_PRECISION_BITS;
    value.clamp(0, 255) as u8
}

/// Format a dHash exactly as Python's `%016x` formatter does.
pub fn format_dhash(hash: u64) -> String {
    format!("{hash:016x}")
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use super::{dhash, format_dhash, resize_and_grayscale};
    use crate::RgbFrame;

    #[test]
    fn all_black_frame_hashes_to_zero() {
        let frame = RgbFrame::new(9, 8, vec![0; 9 * 8 * 3]).expect("valid RGB frame");
        assert_eq!(dhash(&frame), 0);
        assert_eq!(format_dhash(dhash(&frame)), "0000000000000000");
    }

    #[test]
    fn decreasing_row_sets_each_row_bit() {
        let mut pixels = Vec::with_capacity(9 * 8 * 3);
        for _ in 0..8 {
            for value in (0_u8..=8).rev() {
                pixels.extend_from_slice(&[value, value, value]);
            }
        }
        let frame = RgbFrame::new(9, 8, pixels).expect("valid RGB frame");
        assert_eq!(dhash(&frame), u64::MAX);
    }

    #[test]
    fn resize_and_grayscale_matches_pillow_bilinear_reference() {
        let mut pixels = Vec::with_capacity(16 * 12 * 3);
        for y in 0_u16..12 {
            for x in 0_u16..16 {
                pixels.extend_from_slice(&[
                    ((17 * x + 3 * y) % 256) as u8,
                    ((5 * x + 29 * y) % 256) as u8,
                    ((31 * x + 7 * y) % 256) as u8,
                ]);
            }
        }
        let frame = RgbFrame::new(16, 12, pixels).expect("valid RGB frame");
        let expected = [
            13, 32, 53, 74, 92, 89, 105, 127, 130, 40, 58, 79, 100, 110, 112, 132, 153, 132, 67,
            85, 106, 127, 133, 138, 159, 180, 159, 96, 114, 135, 156, 161, 167, 188, 208, 184, 123,
            141, 162, 179, 178, 193, 176, 173, 131, 135, 153, 156, 109, 106, 122, 105, 100, 46, 46,
            64, 82, 80, 82, 100, 121, 127, 73, 55, 73, 94, 97, 106, 126, 147, 153, 99,
        ];
        assert_eq!(resize_and_grayscale(&frame), expected);
    }
}
