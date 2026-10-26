// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, VecDeque};
use std::io::Cursor;
use std::path::Path;

use ffmpeg_next as ffmpeg;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb, imageops::FilterType};

use crate::hash::dhash;
use crate::{HashedFrame, WinnowConfig, WinnowCounters, WinnowState, WinnowVerdict};

/// Demuxers this call site reads; every other demuxer compiled into FFmpeg is refused.
const INPUT_FORMATS: &str = "matroska,mov";

/// Opens a local media file, restricted to [`INPUT_FORMATS`] and the `file` protocol.
fn open_input(path: &Path) -> Result<ffmpeg::format::context::Input, ffmpeg::Error> {
    let mut options = ffmpeg::Dictionary::new();
    options.set("protocol_whitelist", "file");
    options.set("format_whitelist", INPUT_FORMATS);
    ffmpeg::format::input_with_dictionary(path, options)
}

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
    collect_video(path, config, true, &|| false)
}

/// Decode and winnow a video without materializing PNG payloads.
///
/// The returned frame metadata is complete, but every [`QualifiedFrame::png`]
/// is empty. Use this when the caller does not cross the generate boundary.
pub fn process_video_metadata(path: &Path, config: WinnowConfig) -> DescribeResult {
    collect_video(path, config, false, &|| false)
}

fn collect_video(
    path: &Path,
    config: WinnowConfig,
    encode_payloads: bool,
    checkpoint: &dyn Fn() -> bool,
) -> DescribeResult {
    let mut pull = match FramePull::open(path, config, encode_payloads) {
        Ok(pull) => pull,
        Err(result) => return result,
    };
    let mut frames = Vec::new();
    loop {
        match pull.next(checkpoint) {
            PullNext::Frame(frame) => frames.push(frame),
            PullNext::Done => break,
            PullNext::Cancelled => break,
        }
    }
    pull.finish(frames)
}

pub(crate) enum PullNext {
    Frame(QualifiedFrame),
    Done,
    Cancelled,
}

pub(crate) enum RedecodeFail {
    Cancelled,
    Missing,
}

/// One PNG from a fresh open of the same demuxer and winnow. Earlier frames are dropped.
pub(crate) fn redecode_frame(
    path: &Path,
    config: WinnowConfig,
    frame_id: u64,
    timestamp: f64,
    checkpoint: &dyn Fn() -> bool,
) -> Result<Vec<u8>, RedecodeFail> {
    let mut pull = FramePull::open(path, config, true).map_err(|_| RedecodeFail::Missing)?;
    loop {
        match pull.next(checkpoint) {
            PullNext::Cancelled => return Err(RedecodeFail::Cancelled),
            PullNext::Done => return Err(RedecodeFail::Missing),
            PullNext::Frame(frame) => {
                if frame.frame_id == frame_id {
                    if frame.timestamp.to_bits() != timestamp.to_bits() {
                        return Err(RedecodeFail::Missing);
                    }
                    return Ok(frame.png);
                }
                if checkpoint() {
                    return Err(RedecodeFail::Cancelled);
                }
            }
        }
    }
}

pub(crate) struct FramePull {
    encode: bool,
    retain_pngs: Option<BTreeSet<u64>>,
    state: Option<PullState>,
    result: DescribeResult,
    pending: VecDeque<QualifiedFrame>,
    failed: bool,
    finished: bool,
    raw: usize,
    winnow: Option<WinnowState>,
}

struct PngPlan<'a> {
    encode: bool,
    retain: Option<&'a BTreeSet<u64>>,
}

struct PullState {
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    scaler: Option<ffmpeg::software::scaling::context::Context>,
    stream_index: usize,
    time_base: ffmpeg::Rational,
    frame_id: u64,
    input_done: bool,
    eof_sent: bool,
}

impl FramePull {
    #[allow(clippy::result_large_err)]
    pub(crate) fn open(
        path: &Path,
        config: WinnowConfig,
        encode: bool,
    ) -> Result<Self, DescribeResult> {
        let mut result = DescribeResult::default();
        if ffmpeg::init().is_err() {
            result.decode_failed = true;
            return Err(result);
        }
        let input = match open_input(path) {
            Ok(input) => input,
            Err(_) => {
                result.decode_failed = true;
                return Err(result);
            }
        };
        let (stream_index, time_base, parameters) =
            match input.streams().best(ffmpeg::media::Type::Video) {
                Some(stream) => (stream.index(), stream.time_base(), stream.parameters()),
                None => {
                    result.decode_failed = true;
                    return Err(result);
                }
            };
        let context = match ffmpeg::codec::context::Context::from_parameters(parameters) {
            Ok(context) => context,
            Err(_) => return Err(decode_failed(result)),
        };
        let decoder = match context.decoder().video() {
            Ok(decoder) => decoder,
            Err(_) => return Err(decode_failed(result)),
        };
        result.width = Some(decoder.width());
        result.height = Some(decoder.height());
        Ok(Self {
            encode,
            state: Some(PullState {
                input,
                decoder,
                scaler: None,
                stream_index,
                time_base,
                frame_id: 0,
                input_done: false,
                eof_sent: false,
            }),
            result,
            pending: VecDeque::new(),
            failed: false,
            finished: false,
            raw: 0,
            winnow: Some(WinnowState::new(config)),
            retain_pngs: None,
        })
    }

    /// Encode and enqueue PNGs only for these qualified ids. Other kept frames stay metadata.
    pub(crate) fn retain_pngs(&mut self, ids: BTreeSet<u64>) {
        self.retain_pngs = Some(ids);
    }

    pub(crate) fn next(&mut self, checkpoint: &dyn Fn() -> bool) -> PullNext {
        if self.finished && self.pending.is_empty() {
            return PullNext::Done;
        }
        if let Some(frame) = self.pending.pop_front() {
            return PullNext::Frame(frame);
        }
        if self.finished {
            return PullNext::Done;
        }
        let Some(mut state) = self.state.take() else {
            self.finished = true;
            return PullNext::Done;
        };
        let status = pump(
            &mut state,
            &mut self.result,
            &mut self.pending,
            &mut self.raw,
            self.winnow.as_mut().expect("winnow lives until finish"),
            PngPlan {
                encode: self.encode,
                retain: self.retain_pngs.as_ref(),
            },
            checkpoint,
        );
        match status {
            PumpStatus::Ready => {
                self.state = Some(state);
                self.pending
                    .pop_front()
                    .map(PullNext::Frame)
                    .unwrap_or(PullNext::Done)
            }
            PumpStatus::Eof => {
                let counters = self
                    .winnow
                    .as_ref()
                    .expect("winnow lives until finish")
                    .counters();
                self.result.qualified_count = self.result.qualified_frames.len();
                self.result.winnow = Some(metrics(self.raw, counters));
                self.finished = true;
                self.pending
                    .pop_front()
                    .map(PullNext::Frame)
                    .unwrap_or(PullNext::Done)
            }
            PumpStatus::Failed => {
                self.failed = true;
                self.result = decode_failed(std::mem::take(&mut self.result));
                self.finished = true;
                self.state = None;
                self.pending
                    .pop_front()
                    .map(PullNext::Frame)
                    .unwrap_or(PullNext::Done)
            }
            PumpStatus::Cancelled => {
                self.finished = true;
                self.state = None;
                self.pending.clear();
                PullNext::Cancelled
            }
        }
    }

    fn finish(mut self, frames: Vec<QualifiedFrame>) -> DescribeResult {
        if self.failed || self.result.decode_failed {
            self.result.decode_failed = true;
            self.result.qualified_count = frames.len();
            self.result.winnow = None;
        } else if !self.finished {
            if let Some(winnow) = self.winnow.as_ref() {
                self.result.qualified_count = frames.len();
                self.result.winnow = Some(metrics(self.raw, winnow.counters()));
            }
        } else {
            self.result.qualified_count = frames.len();
        }
        self.result.qualified_frames = frames;
        self.result
    }

    pub(crate) fn metadata(&self) -> DescribeResult {
        let mut result = self.result.clone();
        for frame in &mut result.qualified_frames {
            frame.png.clear();
        }
        result
    }
}

enum PumpStatus {
    Ready,
    Eof,
    Failed,
    Cancelled,
}

fn pump(
    state: &mut PullState,
    result: &mut DescribeResult,
    pending: &mut VecDeque<QualifiedFrame>,
    raw: &mut usize,
    winnow: &mut WinnowState,
    png: PngPlan<'_>,
    checkpoint: &dyn Fn() -> bool,
) -> PumpStatus {
    loop {
        if checkpoint() {
            return PumpStatus::Cancelled;
        }
        if !pending.is_empty() {
            return PumpStatus::Ready;
        }
        if !state.input_done {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut state.input) {
                Ok(()) => {
                    if packet.stream() != state.stream_index {
                        continue;
                    }
                    if state.decoder.send_packet(&packet).is_err()
                        || receive_frames(
                            &mut state.decoder,
                            &mut state.scaler,
                            state.time_base,
                            &png,
                            &mut state.frame_id,
                            raw,
                            winnow,
                            result,
                            pending,
                        )
                        .is_err()
                    {
                        return PumpStatus::Failed;
                    }
                    if !pending.is_empty() {
                        return PumpStatus::Ready;
                    }
                }
                Err(ffmpeg::Error::Eof) => state.input_done = true,
                Err(_) => return PumpStatus::Failed,
            }
        } else if !state.eof_sent {
            state.eof_sent = true;
            if state.decoder.send_eof().is_err()
                || receive_frames(
                    &mut state.decoder,
                    &mut state.scaler,
                    state.time_base,
                    &png,
                    &mut state.frame_id,
                    raw,
                    winnow,
                    result,
                    pending,
                )
                .is_err()
            {
                return PumpStatus::Failed;
            }
            if !pending.is_empty() {
                return PumpStatus::Ready;
            }
            return PumpStatus::Eof;
        } else {
            return PumpStatus::Eof;
        }
    }
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
    png: &PngPlan<'_>,
    frame_id: &mut u64,
    raw: &mut usize,
    state: &mut WinnowState,
    result: &mut DescribeResult,
    pending: &mut VecDeque<QualifiedFrame>,
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
                    let keep_png = png.retain.is_none_or(|ids| ids.contains(frame_id));
                    result.qualified_frames.push(QualifiedFrame {
                        frame_id: *frame_id,
                        timestamp,
                        png: Vec::new(),
                    });
                    if keep_png {
                        let encoded = if png.encode {
                            encode_png(&frame).ok_or(ffmpeg::Error::InvalidData)?
                        } else {
                            Vec::new()
                        };
                        pending.push_back(QualifiedFrame {
                            frame_id: *frame_id,
                            timestamp,
                            png: encoded,
                        });
                    }
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
    match vlm_target_dimensions(image.width(), image.height(), max_image_tokens) {
        Some((width, height)) => {
            image::imageops::resize(&image, width, height, FilterType::Lanczos3)
        }
        None => image,
    }
}

fn vlm_target_dimensions(
    width: u32,
    height: u32,
    max_image_tokens: Option<u64>,
) -> Option<(u32, u32)> {
    const MAX_DIMENSION: f64 = 1920.0;
    let mut scale = (MAX_DIMENSION / f64::from(width.max(height))).min(1.0);
    if let Some(max_image_tokens) = max_image_tokens {
        let max_pixels = max_image_tokens as f64 * 32.0 * 32.0;
        scale = scale.min((max_pixels / (f64::from(width) * f64::from(height))).sqrt());
    }
    if scale >= 1.0 {
        return None;
    }
    let target_width = (f64::from(width) * scale).floor().max(1.0) as u32;
    let target_height = (f64::from(height) * scale).floor().max(1.0) as u32;
    Some((target_width, target_height))
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

    use super::{resize_for_vlm, vlm_target_dimensions};

    fn pixels(dimensions: Option<(u32, u32)>, original: (u32, u32)) -> u64 {
        let (width, height) = dimensions.unwrap_or(original);
        u64::from(width) * u64::from(height)
    }

    #[test]
    fn categorization_images_obey_the_unconditional_pixel_budget() {
        let (width, height) =
            vlm_target_dimensions(4096, 2160, Some(1024)).expect("large image is resized");
        assert!(u64::from(width) * u64::from(height) <= 1024 * 32 * 32);
        assert!(width <= 1920 && height <= 1920);

        let resized = resize_for_vlm(ImageBuffer::<Rgb<u8>, Vec<u8>>::new(64, 32), Some(1));
        assert_eq!(
            Some(resized.dimensions()),
            vlm_target_dimensions(64, 32, Some(1))
        );
        assert!(u64::from(resized.width()) * u64::from(resized.height()) <= 32 * 32);
        let unchanged = resize_for_vlm(ImageBuffer::<Rgb<u8>, Vec<u8>>::new(8, 4), Some(1));
        assert_eq!(unchanged.dimensions(), (8, 4));
    }

    #[test]
    fn extraction_resize_keeps_more_detail_than_categorization() {
        let large = (2000, 1200);
        let categorization = vlm_target_dimensions(large.0, large.1, Some(1024));
        let extraction = vlm_target_dimensions(large.0, large.1, None);
        assert!(pixels(extraction, large) > pixels(categorization, large));
        let small = (800, 600);
        assert_eq!(
            vlm_target_dimensions(small.0, small.1, Some(1024)),
            vlm_target_dimensions(small.0, small.1, None)
        );
    }
}
