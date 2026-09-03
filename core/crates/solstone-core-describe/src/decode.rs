// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Cursor;
use std::path::Path;

use ffmpeg_next as ffmpeg;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb, imageops::FilterType};

use crate::hash::dhash;
use crate::{HashedFrame, WinnowConfig, WinnowCounters, WinnowState, WinnowVerdict};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RgbFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
}

impl RgbFrame {
    pub(crate) fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?
            .checked_mul(3)?;
        (pixels.len() == expected).then_some(Self {
            width,
            height,
            pixels,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QualifiedFrame {
    pub frame_id: u64,
    pub timestamp: f64,
    /// Inline PNG bytes cross the generate boundary; never materialized as a file.
    pub png: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WinnowMetrics {
    pub raw: usize,
    pub dhash_qualified: usize,
    pub scene_cut: usize,
    pub stride_dropped: usize,
    pub kept: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DescribeResult {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub qualified_frames: Vec<QualifiedFrame>,
    pub first_hash: Option<u64>,
    pub last_hash: Option<u64>,
    pub qualified_count: usize,
    pub decode_failed: bool,
    /// `None` has the same meaning as Python's empty `{}` failure metrics.
    pub winnow: Option<WinnowMetrics>,
}

pub fn process_video(path: &Path, config: WinnowConfig) -> DescribeResult {
    process_video_inner(path, config, true)
}

/// Decode and winnow a video without materializing PNG payloads.
///
/// The returned frame metadata is complete, but every [`QualifiedFrame::png`]
/// is empty. Use this when the caller does not cross the generate boundary.
pub fn process_video_metadata(path: &Path, config: WinnowConfig) -> DescribeResult {
    process_video_inner(path, config, false)
}

fn process_video_inner(path: &Path, config: WinnowConfig, encode_payloads: bool) -> DescribeResult {
    let mut result = DescribeResult::default();
    if ffmpeg::init().is_err() {
        result.decode_failed = true;
        return result;
    }

    let mut input = match ffmpeg::format::input(path) {
        Ok(input) => input,
        Err(_) => {
            result.decode_failed = true;
            return result;
        }
    };

    let (stream_index, time_base, parameters) =
        match input.streams().best(ffmpeg::media::Type::Video) {
            Some(stream) => (stream.index(), stream.time_base(), stream.parameters()),
            None => {
                result.decode_failed = true;
                return result;
            }
        };
    let context = match ffmpeg::codec::context::Context::from_parameters(parameters) {
        Ok(context) => context,
        Err(_) => return decode_failed(result),
    };
    let mut decoder = match context.decoder().video() {
        Ok(decoder) => decoder,
        Err(_) => return decode_failed(result),
    };

    result.width = Some(decoder.width());
    result.height = Some(decoder.height());
    let mut scaler: Option<ffmpeg::software::scaling::context::Context> = None;

    let mut frame_id = 0_u64;
    let mut state = WinnowState::new(config);
    let mut raw = 0_usize;

    loop {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {
                if packet.stream() != stream_index {
                    continue;
                }
                if decoder.send_packet(&packet).is_err()
                    || receive_frames(
                        &mut decoder,
                        &mut scaler,
                        time_base,
                        encode_payloads,
                        &mut frame_id,
                        &mut raw,
                        &mut state,
                        &mut result,
                    )
                    .is_err()
                {
                    return decode_failed(result);
                }
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(_) => return decode_failed(result),
        }
    }

    if decoder.send_eof().is_err()
        || receive_frames(
            &mut decoder,
            &mut scaler,
            time_base,
            encode_payloads,
            &mut frame_id,
            &mut raw,
            &mut state,
            &mut result,
        )
        .is_err()
    {
        return decode_failed(result);
    }

    let counters = state.counters();
    result.qualified_count = result.qualified_frames.len();
    result.winnow = Some(metrics(raw, counters));
    result
}

fn rgb_scaler_for<'a>(
    slot: &'a mut Option<ffmpeg::software::scaling::context::Context>,
    decoded: &ffmpeg::frame::Video,
) -> Result<&'a mut ffmpeg::software::scaling::context::Context, ffmpeg::Error> {
    let matches_frame = slot.as_ref().is_some_and(|scaler| {
        let input = scaler.input();
        input.format == decoded.format()
            && input.width == decoded.width()
            && input.height == decoded.height()
    });
    if !matches_frame {
        *slot = Some(ffmpeg::software::scaling::context::Context::get(
            decoded.format(),
            decoded.width(),
            decoded.height(),
            ffmpeg::format::Pixel::RGB24,
            decoded.width(),
            decoded.height(),
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        )?);
    }
    slot.as_mut().ok_or(ffmpeg::Error::Bug)
}

#[allow(clippy::too_many_arguments)]
fn receive_frames(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut Option<ffmpeg::software::scaling::context::Context>,
    time_base: ffmpeg::Rational,
    encode_payloads: bool,
    frame_id: &mut u64,
    raw: &mut usize,
    state: &mut WinnowState,
    result: &mut DescribeResult,
) -> Result<(), ffmpeg::Error> {
    loop {
        let mut decoded = ffmpeg::frame::Video::empty();
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {
                *raw += 1;
                let Some(pts) = decoded.pts() else {
                    continue;
                };
                *frame_id += 1;
                let timestamp = pts as f64 * f64::from(time_base);
                let mut rgb = ffmpeg::frame::Video::empty();
                rgb_scaler_for(scaler, &decoded)?.run(&decoded, &mut rgb)?;
                let frame = rgb_frame(&rgb).ok_or(ffmpeg::Error::InvalidData)?;
                let hash = dhash(&frame);
                let verdict = state.decide(HashedFrame { timestamp, hash });
                if matches!(verdict, WinnowVerdict::Kept | WinnowVerdict::SceneCut) {
                    if result.first_hash.is_none() {
                        result.first_hash = Some(hash);
                    }
                    result.last_hash = Some(hash);
                    result.qualified_frames.push(QualifiedFrame {
                        frame_id: *frame_id,
                        timestamp,
                        png: if encode_payloads {
                            encode_png(&frame).ok_or(ffmpeg::Error::InvalidData)?
                        } else {
                            Vec::new()
                        },
                    });
                }
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn encode_png(frame: &RgbFrame) -> Option<Vec<u8>> {
    let image =
        ImageBuffer::<Rgb<u8>, _>::from_raw(frame.width, frame.height, frame.pixels.clone())?;
    encode_image_png(image)
}

pub fn resize_for_vlm_png(png: &[u8], max_image_tokens: Option<u64>) -> Option<Vec<u8>> {
    let image = image::load_from_memory_with_format(png, ImageFormat::Png)
        .ok()?
        .into_rgb8();
    encode_image_png(resize_for_vlm(image, max_image_tokens))
}

fn encode_image_png(image: ImageBuffer<Rgb<u8>, Vec<u8>>) -> Option<Vec<u8>> {
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut output, ImageFormat::Png)
        .ok()?;
    Some(output.into_inner())
}

pub fn resize_for_vlm(
    image: ImageBuffer<Rgb<u8>, Vec<u8>>,
    max_image_tokens: Option<u64>,
) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    const MAX_DIMENSION: f64 = 1920.0;
    let width = image.width();
    let height = image.height();
    let mut scale = (MAX_DIMENSION / f64::from(width.max(height))).min(1.0);
    if let Some(max_image_tokens) = max_image_tokens {
        let max_pixels = max_image_tokens as f64 * 32.0 * 32.0;
        scale = scale.min((max_pixels / (f64::from(width) * f64::from(height))).sqrt());
    }
    if scale >= 1.0 {
        return image;
    }
    let target_width = (f64::from(width) * scale).floor().max(1.0) as u32;
    let target_height = (f64::from(height) * scale).floor().max(1.0) as u32;
    image::imageops::resize(&image, target_width, target_height, FilterType::Lanczos3)
}

fn rgb_frame(frame: &ffmpeg::frame::Video) -> Option<RgbFrame> {
    let width = usize::try_from(frame.width()).ok()?;
    let height = usize::try_from(frame.height()).ok()?;
    let row_len = width.checked_mul(3)?;
    if frame.planes() == 0 || frame.stride(0) < row_len {
        return None;
    }
    let data = frame.data(0);
    let needed = frame.stride(0).checked_mul(height)?;
    if data.len() < needed {
        return None;
    }
    let mut pixels = Vec::with_capacity(row_len.checked_mul(height)?);
    for row in 0..height {
        let start = row.checked_mul(frame.stride(0))?;
        pixels.extend_from_slice(data.get(start..start.checked_add(row_len)?)?);
    }
    RgbFrame::new(frame.width(), frame.height(), pixels)
}

fn metrics(raw: usize, counters: WinnowCounters) -> WinnowMetrics {
    WinnowMetrics {
        raw,
        dhash_qualified: counters.dhash_qualified,
        scene_cut: counters.scene_cut,
        stride_dropped: counters.stride_dropped,
        kept: counters.kept,
    }
}

fn decode_failed(mut result: DescribeResult) -> DescribeResult {
    result.decode_failed = true;
    // Python retains frames that qualified before an FFmpegError. Its metrics
    // assignment sits after the loop, so `None` represents the same empty
    // metrics object on this terminal-error path.
    result.qualified_count = result.qualified_frames.len();
    result.winnow = None;
    result
}

#[cfg(all(test, not(feature = "full-tests")))]
mod image_tests {
    use image::{ImageBuffer, Rgb};

    use super::resize_for_vlm;

    #[test]
    fn categorization_images_obey_the_unconditional_pixel_budget() {
        let image = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(4096, 2160);
        let resized = resize_for_vlm(image, Some(1024));
        assert!(u64::from(resized.width()) * u64::from(resized.height()) <= 1024 * 32 * 32);
        assert!(resized.width() <= 1920 && resized.height() <= 1920);
    }

    #[test]
    fn extraction_resize_keeps_more_detail_than_categorization() {
        let large = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(2000, 1200);
        let categorization = resize_for_vlm(large.clone(), Some(1024));
        let extraction = resize_for_vlm(large, None);
        assert!(
            u64::from(extraction.width()) * u64::from(extraction.height())
                > u64::from(categorization.width()) * u64::from(categorization.height())
        );
        let small = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(800, 600);
        let categorization = resize_for_vlm(small.clone(), Some(1024));
        let extraction = resize_for_vlm(small, None);
        assert_eq!(categorization.dimensions(), extraction.dimensions());
    }
}
