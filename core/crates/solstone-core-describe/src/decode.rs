// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, VecDeque};
use std::io::Cursor;
use std::path::Path;

use ffmpeg_next as ffmpeg;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb, imageops::FilterType};

use crate::hash::dhash;
use crate::text_gate::TextGate;
use crate::{HashedFrame, WinnowConfig, WinnowCounters, WinnowState, WinnowVerdict};

/// Demuxers this call site reads; every other demuxer compiled into FFmpeg is refused.
const INPUT_FORMATS: &str = "matroska,mov";

/// Still images describe reads as one frame: a screenshot.
const STILL_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

fn is_still(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            STILL_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// Opens a local media file, restricted to [`INPUT_FORMATS`] and the `file` protocol.
fn open_input(path: &Path) -> Result<ffmpeg::format::context::Input, ffmpeg::Error> {
    let mut options = ffmpeg::Dictionary::new();
    options.set("protocol_whitelist", "file");
    options.set("format_whitelist", INPUT_FORMATS);
    ffmpeg::format::input_with_dictionary(path, options)
}

pub(crate) fn recording_supported(path: &Path) -> Result<bool, ffmpeg::Error> {
    if is_still(path) {
        return Ok(true);
    }
    ffmpeg::init()?;
    let input = open_input(path)?;
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(ffmpeg::Error::StreamNotFound)?;
    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
    Ok(ffmpeg::decoder::find(context.id()).is_some())
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
    pub text_changed: usize,
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
    /// Frames kept because their text changed under the hash threshold. A later open of
    /// the same video replays these instead of reading text again, so frame ids agree.
    pub text_changed: BTreeSet<u64>,
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
    text_changed: &BTreeSet<u64>,
    frame_id: u64,
    timestamp: f64,
    checkpoint: &dyn Fn() -> bool,
) -> Result<Vec<u8>, RedecodeFail> {
    let mut pull = FramePull::open(path, config, true).map_err(|_| RedecodeFail::Missing)?;
    pull.replay_text_changes(text_changed.clone());
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
    replay_text_changes: BTreeSet<u64>,
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

/// Who decides on a frame below the hash threshold: the live gate, or an earlier pass.
struct Gate<'a, 'g> {
    live: Option<&'a mut TextGate<'g>>,
    replay: &'a BTreeSet<u64>,
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
        if is_still(path) {
            return Self::open_still(path, config, encode);
        }
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
            replay_text_changes: BTreeSet::new(),
        })
    }

    /// One screenshot is one frame: decoded whole, kept, and done.
    #[allow(clippy::result_large_err)]
    fn open_still(path: &Path, config: WinnowConfig, encode: bool) -> Result<Self, DescribeResult> {
        let mut result = DescribeResult::default();
        let Some(image) = std::fs::read(path)
            .ok()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
        else {
            return Err(decode_failed(result));
        };
        let image = image.into_rgb8();
        let (width, height) = image.dimensions();
        let Some(frame) = RgbFrame::new(width, height, image.into_raw()) else {
            return Err(decode_failed(result));
        };
        let hash = dhash(&frame);
        let mut winnow = WinnowState::new(config);
        winnow.decide(HashedFrame {
            timestamp: 0.0,
            hash,
        });
        let png = if encode {
            match encode_png(&frame) {
                Some(png) => png,
                None => return Err(decode_failed(result)),
            }
        } else {
            Vec::new()
        };
        result.width = Some(width);
        result.height = Some(height);
        result.first_hash = Some(hash);
        result.last_hash = Some(hash);
        result.qualified_frames.push(QualifiedFrame {
            frame_id: 1,
            timestamp: 0.0,
            png: Vec::new(),
        });
        result.qualified_count = 1;
        result.winnow = Some(metrics(1, winnow.counters()));
        Ok(Self {
            encode,
            state: None,
            result,
            pending: VecDeque::from([QualifiedFrame {
                frame_id: 1,
                timestamp: 0.0,
                png,
            }]),
            failed: false,
            finished: true,
            raw: 1,
            winnow: Some(winnow),
            retain_pngs: None,
            replay_text_changes: BTreeSet::new(),
        })
    }

    /// Encode and enqueue PNGs only for these qualified ids. Other kept frames stay metadata.
    pub(crate) fn retain_pngs(&mut self, ids: BTreeSet<u64>) {
        self.retain_pngs = Some(ids);
    }

    /// Keep these frames as the text gate kept them on an earlier pass over this video.
    pub(crate) fn replay_text_changes(&mut self, ids: BTreeSet<u64>) {
        self.replay_text_changes = ids;
    }

    pub(crate) fn next(&mut self, checkpoint: &dyn Fn() -> bool) -> PullNext {
        self.next_gated(checkpoint, None)
    }

    /// Like [`Self::next`], with the text gate deciding on frames below the hash threshold.
    pub(crate) fn next_gated(
        &mut self,
        checkpoint: &dyn Fn() -> bool,
        gate: Option<&mut TextGate<'_>>,
    ) -> PullNext {
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
            Gate {
                live: gate,
                replay: &self.replay_text_changes,
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

#[allow(clippy::too_many_arguments)]
fn pump(
    state: &mut PullState,
    result: &mut DescribeResult,
    pending: &mut VecDeque<QualifiedFrame>,
    raw: &mut usize,
    winnow: &mut WinnowState,
    png: PngPlan<'_>,
    mut gate: Gate<'_, '_>,
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
                            &mut gate,
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
                    &mut gate,
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
    gate: &mut Gate<'_, '_>,
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
                let hashed = HashedFrame { timestamp, hash };
                let mut verdict = state.decide(hashed);
                let promote = match gate.live.as_deref_mut() {
                    Some(live) => live.frame(timestamp, &frame, verdict),
                    None => gate.replay.contains(frame_id),
                };
                if promote && verdict == WinnowVerdict::BelowThreshold {
                    verdict = state.promote(hashed);
                    result.text_changed.insert(*frame_id);
                }
                if matches!(
                    verdict,
                    WinnowVerdict::Kept | WinnowVerdict::SceneCut | WinnowVerdict::TextChanged
                ) {
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

pub(crate) fn encode_png(frame: &RgbFrame) -> Option<Vec<u8>> {
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
    // A screen at least twice as wide as it is tall (21:9 or 32:9) would lose too much text height to the
    // longest-side cap: a 3440x1440 frame at 1920x803 roughly doubles misread text. It is sized by area
    // instead, up to 1920 x 1920 px, the largest image every thinking lane admits.
    const WIDE_MAX_PIXELS: f64 = MAX_DIMENSION * MAX_DIMENSION;
    let (w, h) = (f64::from(width), f64::from(height));
    let mut scale = (MAX_DIMENSION / w.max(h)).min(1.0);
    if w >= 2.0 * h {
        scale = scale.max((WIDE_MAX_PIXELS / (w * h)).sqrt().min(1.0));
    }
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
        text_changed: counters.text_changed,
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
    fn a_screenshot_is_one_kept_frame_at_its_own_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("screenshot_screen.png");
        ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(9, 19, Rgb([10, 20, 30]))
            .save(&path)
            .expect("write png");
        let result = super::process_video(&path, crate::WinnowConfig::default());
        assert!(!result.decode_failed);
        assert_eq!((result.width, result.height), (Some(9), Some(19)));
        assert_eq!(result.qualified_count, 1);
        assert_eq!(result.qualified_frames[0].frame_id, 1);
        let decoded = image::load_from_memory(&result.qualified_frames[0].png).expect("png");
        assert_eq!((decoded.width(), decoded.height()), (9, 19));
        let png = std::fs::read(&path).expect("read");
        std::fs::write(&path, &png[..png.len() / 2]).expect("truncate");
        assert!(super::process_video(&path, crate::WinnowConfig::default()).decode_failed);
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

    #[test]
    fn wide_screens_keep_their_text_height_and_other_screens_keep_the_longest_side_cap() {
        // Ultrawide and super-ultrawide extraction images grow to the area every lane admits.
        for wide in [(3440, 1440), (5120, 1440), (2560, 1080)] {
            let (width, height) = vlm_target_dimensions(wide.0, wide.1, None).unwrap_or(wide);
            assert!(u64::from(width) * u64::from(height) <= 1920 * 1920);
            assert!(
                height > wide.1 * 1920 / wide.0,
                "{wide:?} kept only {height} px of height"
            );
        }
        assert_eq!(vlm_target_dimensions(3440, 1440, None), Some((2967, 1242)));
        assert_eq!(vlm_target_dimensions(2560, 1080, None), None);
        // Desktops, Retina and 4K panels, phones and a near-16:9 capture are unchanged.
        for (screen, expected) in [
            ((3840, 2160), (1920, 1080)),
            ((2880, 1800), (1920, 1200)),
            ((2560, 1439), (1920, 1079)),
            ((1179, 2556), (885, 1920)),
            ((1080, 2400), (864, 1920)),
        ] {
            assert_eq!(
                vlm_target_dimensions(screen.0, screen.1, None),
                Some(expected)
            );
        }
        // Categorization stays inside its own token budget.
        let (width, height) = vlm_target_dimensions(3440, 1440, Some(1024)).expect("resized");
        assert!(u64::from(width) * u64::from(height) <= 1024 * 32 * 32);
    }
}

#[cfg(test)]
mod text_gate_tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use super::{FramePull, PullNext, redecode_frame};
    use crate::WinnowConfig;
    use crate::screen_ocr::{OcrFailure, OcrWorker, OcrWorkerFactory, RawOccurrence};
    use crate::text_gate::TextGate;

    /// Every read finds four lines no earlier read had.
    #[derive(Default)]
    struct FreshText {
        seed: u64,
    }

    impl OcrWorker for FreshText {
        fn recognize(&mut self, _png: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure> {
            Ok((0..4)
                .map(|order| {
                    let words = (0..5)
                        .map(|_| {
                            (0..6)
                                .map(|_| {
                                    self.seed = self
                                        .seed
                                        .wrapping_mul(6_364_136_223_846_793_005)
                                        .wrapping_add(1_442_695_040_888_963_407);
                                    char::from(b'a' + (self.seed >> 59) as u8 % 26)
                                })
                                .collect::<String>()
                        })
                        .collect::<Vec<_>>();
                    RawOccurrence {
                        line_order: order,
                        text: words.join(" "),
                        confidence: Some(0.99),
                        geometry: Vec::new(),
                        recurrence: 0,
                    }
                })
                .collect())
        }
    }

    impl OcrWorkerFactory for FreshText {
        fn recipe_id(&mut self) -> String {
            "fresh".to_owned()
        }

        fn create(&mut self) -> Result<Option<Box<dyn OcrWorker>>, OcrFailure> {
            Ok(Some(Box::new(FreshText::default())))
        }
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/describe_corpus")
            .join(name)
    }

    fn drain(pull: &mut FramePull, mut gate: Option<&mut TextGate<'_>>) -> Vec<(u64, f64)> {
        let mut frames = Vec::new();
        loop {
            match pull.next_gated(&|| false, gate.as_deref_mut()) {
                PullNext::Frame(frame) => frames.push((frame.frame_id, frame.timestamp)),
                PullNext::Done => return frames,
                PullNext::Cancelled => panic!("nothing cancels this pull"),
            }
        }
    }

    fn open(video: &Path, encode: bool) -> FramePull {
        FramePull::open(video, WinnowConfig::default(), encode)
            .unwrap_or_else(|_| panic!("fixture opens"))
    }

    #[test]
    #[cfg(feature = "full-tests")]
    fn selected_replay_preserves_native_png_bytes_and_text_changed_frames() {
        let video = fixture("stride_floor_vp8_screen.webm");
        let mut ocr = FreshText::default();
        let mut gate = TextGate::new(&mut ocr);
        let mut first = open(&video, false);
        let frames = drain(&mut first, Some(&mut gate));
        let text_changed = first.metadata().text_changed;
        let wanted = [frames[0], frames[2], frames[3]];
        let mut replay = open(&video, true);
        replay.replay_text_changes(text_changed.clone());
        replay.retain_pngs(wanted.iter().map(|(id, _)| *id).collect());
        for (id, timestamp) in wanted {
            let PullNext::Frame(frame) = replay.next(&|| false) else {
                panic!("requested native frame missing");
            };
            assert_eq!(frame.frame_id, id);
            assert_eq!(frame.timestamp.to_bits(), timestamp.to_bits());
            let single = redecode_frame(
                &video,
                WinnowConfig::default(),
                &text_changed,
                id,
                timestamp,
                &|| false,
            )
            .unwrap_or_else(|_| panic!("individual frame missing"));
            assert_eq!(frame.png, single);
        }
        assert!(matches!(replay.next(&|| false), PullNext::Done));
        let mut cancelled = open(&video, true);
        cancelled.retain_pngs(BTreeSet::from([wanted[2].0]));
        assert!(matches!(cancelled.next(&|| true), PullNext::Cancelled));
    }

    #[test]
    fn a_reopened_video_keeps_exactly_the_frames_the_text_gate_kept() {
        // Frame 2 is a scene cut; frames 3 to 12 move a few pixels each, under the hash threshold.
        let video = fixture("stride_floor_vp8_screen.webm");
        let mut ocr = FreshText::default();
        let mut gate = TextGate::new(&mut ocr);
        let mut pull = open(&video, false);
        let gated = drain(&mut pull, Some(&mut gate));
        let result = pull.metadata();
        let ids = gated.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        assert_eq!(ids, [1, 2, 7, 12]);
        assert_eq!(result.text_changed, BTreeSet::from([7, 12]));
        assert_eq!(result.winnow.map(|metrics| metrics.text_changed), Some(2));

        let mut plain = open(&video, false);
        assert_eq!(
            drain(&mut plain, None)
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            [1, 2]
        );

        let mut replay = open(&video, true);
        replay.replay_text_changes(result.text_changed.clone());
        assert_eq!(drain(&mut replay, None), gated);
        let (id, timestamp) = gated[3];
        let png = redecode_frame(
            &video,
            WinnowConfig::default(),
            &result.text_changed,
            id,
            timestamp,
            &|| false,
        );
        assert!(png.is_ok_and(|png| !png.is_empty()));
    }
}
