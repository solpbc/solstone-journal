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

pub(crate) fn is_still(path: &Path) -> bool {
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
    /// The first frame's id and timestamp when it matched the winnow's start reference and
    /// was not kept: the screen was unchanged since the previous segment.
    pub carried_frame: Option<(u64, f64)>,
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
    selected_native: bool,
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
    selected_native: bool,
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
            selected_native: false,
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
            selected_native: false,
            replay_text_changes: BTreeSet::new(),
        })
    }

    /// Encode and enqueue PNGs only for these qualified ids. Other kept frames stay metadata.
    pub(crate) fn retain_pngs(&mut self, ids: BTreeSet<u64>) {
        self.retain_pngs = Some(ids);
    }

    /// Recover exact raw IDs already selected by an earlier pass. Unrequested
    /// frames need decoding for codec state, but no RGB conversion or hashing.
    /// Metadata and winnow metrics are not populated by this recovery mode.
    #[allow(dead_code)] // Face recovery has no caller in mobile builds.
    pub(crate) fn select_native_frames(&mut self, ids: BTreeSet<u64>) {
        self.retain_pngs(ids);
        self.selected_native = true;
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
                selected_native: self.selected_native,
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
                if png.selected_native && png.retain.is_some_and(|ids| !ids.contains(frame_id)) {
                    continue;
                }
                let timestamp = pts as f64 * f64::from(time_base);
                let mut rgb = ffmpeg::frame::Video::empty();
                rgb_scaler_for(scaler, &decoded)?.run(&decoded, &mut rgb)?;
                let frame = rgb_frame(&rgb).ok_or(ffmpeg::Error::InvalidData)?;
                if png.selected_native {
                    pending.push_back(QualifiedFrame {
                        frame_id: *frame_id,
                        timestamp,
                        png: encode_png(&frame).ok_or(ffmpeg::Error::InvalidData)?,
                    });
                    continue;
                }
                let hash = dhash(&frame);
                let hashed = HashedFrame { timestamp, hash };
                let first = result.first_hash.is_none();
                let carried = first && state.has_reference();
                let mut verdict = state.decide(hashed);
                let promote = match gate.live.as_deref_mut() {
                    Some(live) => live.frame(timestamp, &frame, verdict),
                    None => gate.replay.contains(frame_id),
                };
                if promote && verdict == WinnowVerdict::BelowThreshold {
                    verdict = state.promote(hashed);
                    result.text_changed.insert(*frame_id);
                }
                let kept = matches!(
                    verdict,
                    WinnowVerdict::Kept | WinnowVerdict::SceneCut | WinnowVerdict::TextChanged
                );
                if first {
                    // The video's own first frame, kept or not: what the next segment and
                    // change detection compare across the boundary.
                    result.first_hash = Some(hash);
                    if carried && !kept {
                        result.carried_frame = Some((*frame_id, timestamp));
                    }
                }
                if kept {
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
        replay.select_native_frames(wanted.iter().map(|(id, _)| *id).collect());
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
        cancelled.select_native_frames(BTreeSet::from([wanted[2].0]));
        assert!(matches!(cancelled.next(&|| true), PullNext::Cancelled));

        // Raw ID 3 is below the ordinary winnow threshold. Recovery obeys the
        // earlier caller's explicit IDs rather than making that choice again.
        let all = super::process_video(
            &video,
            WinnowConfig {
                dhash_threshold: 0,
                scene_cut_threshold: 0,
                min_stride_seconds: 0.,
                start_reference: None,
            },
        );
        let expected = all
            .qualified_frames
            .iter()
            .find(|f| f.frame_id == 3)
            .unwrap();
        let mut sparse = open(&video, true);
        sparse.select_native_frames(BTreeSet::from([3]));
        let PullNext::Frame(actual) = sparse.next(&|| false) else {
            panic!("requested raw frame missing");
        };
        assert_eq!(actual, *expected);

        let checkpoints = std::cell::Cell::new(0);
        let mut cancelled = open(&video, true);
        cancelled.select_native_frames(BTreeSet::from([wanted[2].0]));
        assert!(matches!(
            cancelled.next(&|| {
                checkpoints.set(checkpoints.get() + 1);
                checkpoints.get() == 3
            }),
            PullNext::Cancelled
        ));
        assert_eq!(checkpoints.get(), 3);
        assert!(cancelled.raw > 0);
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

    #[test]
    fn a_video_carried_from_its_predecessor_drops_a_matching_first_frame_on_every_open() {
        let video = fixture("stride_floor_vp8_screen.webm");
        let plain = super::process_video_metadata(&video, WinnowConfig::default());
        let first = plain.first_hash.expect("fixture has a first frame");
        let carried = WinnowConfig {
            start_reference: Some(first),
            ..WinnowConfig::default()
        };
        let mut pull =
            FramePull::open(&video, carried, false).unwrap_or_else(|_| panic!("fixture opens"));
        let kept = drain(&mut pull, None);
        let result = pull.metadata();
        // The unchanged first frame is not kept, but stays the video's first hash.
        assert_eq!(kept.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [2]);
        assert_eq!(result.first_hash, Some(first));
        assert_eq!(
            result.carried_frame,
            Some((1, plain.qualified_frames[0].timestamp))
        );
        assert_eq!(result.last_hash, plain.last_hash);
        let png = redecode_frame(
            &video,
            carried,
            &BTreeSet::new(),
            kept[0].0,
            kept[0].1,
            &|| false,
        );
        assert!(png.is_ok_and(|png| !png.is_empty()));

        // A screen that changed across the boundary keeps exactly today's frames.
        let changed = WinnowConfig {
            start_reference: Some(!first),
            ..WinnowConfig::default()
        };
        let result = super::process_video_metadata(&video, changed);
        assert_eq!(result.carried_frame, None);
        assert_eq!(result.qualified_frames, plain.qualified_frames);
    }

    const VP9_WEBM: &[u8] = &[
        0x1a, 0x45, 0xdf, 0xa3, 0x9f, 0x42, 0x86, 0x81, 0x01, 0x42, 0xf7, 0x81, 0x01, 0x42, 0xf2,
        0x81, 0x04, 0x42, 0xf3, 0x81, 0x08, 0x42, 0x82, 0x84, 0x77, 0x65, 0x62, 0x6d, 0x42, 0x87,
        0x81, 0x02, 0x42, 0x85, 0x81, 0x02, 0x18, 0x53, 0x80, 0x67, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x02, 0x15, 0x11, 0x4d, 0x9b, 0x74, 0xba, 0x4d, 0xbb, 0x8b, 0x53, 0xab, 0x84, 0x15,
        0x49, 0xa9, 0x66, 0x53, 0xac, 0x81, 0xa1, 0x4d, 0xbb, 0x8b, 0x53, 0xab, 0x84, 0x16, 0x54,
        0xae, 0x6b, 0x53, 0xac, 0x81, 0xd6, 0x4d, 0xbb, 0x8c, 0x53, 0xab, 0x84, 0x12, 0x54, 0xc3,
        0x67, 0x53, 0xac, 0x82, 0x01, 0x23, 0x4d, 0xbb, 0x8c, 0x53, 0xab, 0x84, 0x1c, 0x53, 0xbb,
        0x6b, 0x53, 0xac, 0x82, 0x01, 0xff, 0xec, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x59,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x15, 0x49, 0xa9, 0x66, 0xb0, 0x2a, 0xd7, 0xb1, 0x83,
        0x0f, 0x42, 0x40, 0x4d, 0x80, 0x8c, 0x4c, 0x61, 0x76, 0x66, 0x36, 0x32, 0x2e, 0x33, 0x2e,
        0x31, 0x30, 0x30, 0x57, 0x41, 0x8c, 0x4c, 0x61, 0x76, 0x66, 0x36, 0x32, 0x2e, 0x33, 0x2e,
        0x31, 0x30, 0x30, 0x44, 0x89, 0x88, 0x40, 0x9f, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x16,
        0x54, 0xae, 0x6b, 0xc8, 0xae, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3f, 0xd7, 0x81,
        0x01, 0x73, 0xc5, 0x88, 0xee, 0x17, 0x69, 0x63, 0xd7, 0xa6, 0xef, 0x46, 0x9c, 0x81, 0x00,
        0x22, 0xb5, 0x9c, 0x83, 0x75, 0x6e, 0x64, 0x88, 0x81, 0x00, 0x86, 0x85, 0x56, 0x5f, 0x56,
        0x50, 0x39, 0x83, 0x81, 0x01, 0x23, 0xe3, 0x83, 0x84, 0x3b, 0x9a, 0xca, 0x00, 0xe0, 0x90,
        0xb0, 0x81, 0x10, 0xba, 0x81, 0x10, 0x9a, 0x81, 0x02, 0x55, 0xb0, 0x84, 0x55, 0xb9, 0x81,
        0x01, 0x12, 0x54, 0xc3, 0x67, 0x40, 0x7f, 0x73, 0x73, 0x9f, 0x63, 0xc0, 0x80, 0x67, 0xc8,
        0x99, 0x45, 0xa3, 0x87, 0x45, 0x4e, 0x43, 0x4f, 0x44, 0x45, 0x52, 0x44, 0x87, 0x8c, 0x4c,
        0x61, 0x76, 0x66, 0x36, 0x32, 0x2e, 0x33, 0x2e, 0x31, 0x30, 0x30, 0x73, 0x73, 0xda, 0x63,
        0xc0, 0x8b, 0x63, 0xc5, 0x88, 0xee, 0x17, 0x69, 0x63, 0xd7, 0xa6, 0xef, 0x46, 0x67, 0xc8,
        0xa5, 0x45, 0xa3, 0x87, 0x45, 0x4e, 0x43, 0x4f, 0x44, 0x45, 0x52, 0x44, 0x87, 0x98, 0x4c,
        0x61, 0x76, 0x63, 0x36, 0x32, 0x2e, 0x31, 0x31, 0x2e, 0x31, 0x30, 0x30, 0x20, 0x6c, 0x69,
        0x62, 0x76, 0x70, 0x78, 0x2d, 0x76, 0x70, 0x39, 0x67, 0xc8, 0xa1, 0x45, 0xa3, 0x88, 0x44,
        0x55, 0x52, 0x41, 0x54, 0x49, 0x4f, 0x4e, 0x44, 0x87, 0x93, 0x30, 0x30, 0x3a, 0x30, 0x30,
        0x3a, 0x30, 0x32, 0x2e, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x00, 0x1f,
        0x43, 0xb6, 0x75, 0xd2, 0xe7, 0x81, 0x00, 0xa3, 0xab, 0x81, 0x00, 0x00, 0x80, 0x82, 0x49,
        0x83, 0x42, 0x00, 0x00, 0xf0, 0x00, 0xf6, 0x00, 0x38, 0x24, 0x1c, 0x18, 0x4a, 0x00, 0x00,
        0x30, 0x60, 0x00, 0x00, 0x18, 0x7f, 0xff, 0xfe, 0x08, 0x58, 0xff, 0xff, 0xff, 0xff, 0x90,
        0xa0, 0x7f, 0xff, 0xff, 0x5f, 0xf0, 0x00, 0xa3, 0xa0, 0x81, 0x03, 0xe8, 0x00, 0x86, 0x00,
        0x40, 0x92, 0xf0, 0x01, 0x40, 0x00, 0x00, 0x0c, 0x60, 0x00, 0x00, 0x00, 0xc9, 0xff, 0xfe,
        0x46, 0x03, 0xff, 0xff, 0xfe, 0xd5, 0xaf, 0xff, 0xb8, 0x80, 0x00, 0x1c, 0x53, 0xbb, 0x6b,
        0x91, 0xbb, 0x8f, 0xb3, 0x81, 0x00, 0xb7, 0x8a, 0xf7, 0x81, 0x01, 0xf1, 0x82, 0x01, 0xa8,
        0xf0, 0x81, 0x03,
    ];

    const HEVC_MP4: &[u8] = &[
        0x00, 0x00, 0x00, 0x1c, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d, 0x00, 0x00, 0x02,
        0x00, 0x69, 0x73, 0x6f, 0x6d, 0x69, 0x73, 0x6f, 0x32, 0x6d, 0x70, 0x34, 0x31, 0x00, 0x00,
        0x00, 0x08, 0x66, 0x72, 0x65, 0x65, 0x00, 0x00, 0x00, 0x1e, 0x6d, 0x64, 0x61, 0x74, 0x00,
        0x00, 0x00, 0x12, 0x28, 0x01, 0xaf, 0x13, 0x80, 0xf5, 0x24, 0xa3, 0xff, 0xb0, 0xc3, 0x58,
        0x3b, 0x9a, 0x41, 0xf5, 0xff, 0x80, 0x00, 0x00, 0x03, 0x5e, 0x6d, 0x6f, 0x6f, 0x76, 0x00,
        0x00, 0x00, 0x6c, 0x6d, 0x76, 0x68, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xe8, 0x00, 0x00, 0x03, 0xe8, 0x00, 0x01, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x02, 0x00, 0x00, 0x02, 0x89, 0x74, 0x72, 0x61, 0x6b, 0x00, 0x00, 0x00, 0x5c, 0x74,
        0x6b, 0x68, 0x64, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xe8, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x40, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x24, 0x65, 0x64, 0x74, 0x73, 0x00, 0x00, 0x00, 0x1c, 0x65, 0x6c, 0x73, 0x74, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x03, 0xe8, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x01, 0x6d, 0x64, 0x69, 0x61, 0x00, 0x00, 0x00, 0x20,
        0x6d, 0x64, 0x68, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x40, 0x00, 0x55, 0xc4, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x2d, 0x68, 0x64, 0x6c, 0x72, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x76,
        0x69, 0x64, 0x65, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x56, 0x69, 0x64, 0x65, 0x6f, 0x48, 0x61, 0x6e, 0x64, 0x6c, 0x65, 0x72, 0x00, 0x00, 0x00,
        0x01, 0xac, 0x6d, 0x69, 0x6e, 0x66, 0x00, 0x00, 0x00, 0x14, 0x76, 0x6d, 0x68, 0x64, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x24,
        0x64, 0x69, 0x6e, 0x66, 0x00, 0x00, 0x00, 0x1c, 0x64, 0x72, 0x65, 0x66, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0c, 0x75, 0x72, 0x6c, 0x20, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x01, 0x6c, 0x73, 0x74, 0x62, 0x6c, 0x00, 0x00, 0x01, 0x08, 0x73,
        0x74, 0x73, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xf8,
        0x68, 0x65, 0x76, 0x31, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10,
        0x00, 0x10, 0x00, 0x48, 0x00, 0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01, 0x15, 0x4c, 0x61, 0x76, 0x63, 0x36, 0x32, 0x2e, 0x31, 0x31, 0x2e, 0x31, 0x30, 0x30,
        0x20, 0x6c, 0x69, 0x62, 0x78, 0x32, 0x36, 0x35, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x18, 0xff, 0xff, 0x00, 0x00, 0x00, 0x74, 0x68, 0x76, 0x63, 0x43,
        0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1e, 0xf0, 0x00,
        0xfc, 0xfd, 0xf8, 0xf8, 0x00, 0x00, 0x0f, 0x03, 0x20, 0x00, 0x01, 0x00, 0x18, 0x40, 0x01,
        0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00,
        0x00, 0x03, 0x00, 0x1e, 0x95, 0x98, 0x09, 0x21, 0x00, 0x01, 0x00, 0x28, 0x42, 0x01, 0x01,
        0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x1e,
        0xa0, 0x88, 0x45, 0x96, 0x56, 0x6a, 0xbc, 0xaf, 0x01, 0x68, 0x08, 0x00, 0x00, 0x03, 0x00,
        0x08, 0x00, 0x00, 0x03, 0x00, 0x08, 0x40, 0x22, 0x00, 0x01, 0x00, 0x06, 0x44, 0x01, 0xc1,
        0x73, 0xd0, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x66, 0x69, 0x65, 0x6c, 0x01, 0x00, 0x00, 0x00,
        0x00, 0x10, 0x70, 0x61, 0x73, 0x70, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x00, 0x00, 0x14, 0x62, 0x74, 0x72, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xb0,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x73, 0x74, 0x74, 0x73, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00,
        0x00, 0x1c, 0x73, 0x74, 0x73, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x14,
        0x73, 0x74, 0x73, 0x7a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x16, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x14, 0x73, 0x74, 0x63, 0x6f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x61, 0x75, 0x64, 0x74, 0x61, 0x00,
        0x00, 0x00, 0x59, 0x6d, 0x65, 0x74, 0x61, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21,
        0x68, 0x64, 0x6c, 0x72, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x6d, 0x64, 0x69,
        0x72, 0x61, 0x70, 0x70, 0x6c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x2c, 0x69, 0x6c, 0x73, 0x74, 0x00, 0x00, 0x00, 0x24, 0xa9, 0x74, 0x6f, 0x6f,
        0x00, 0x00, 0x00, 0x1c, 0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x00, 0x4c, 0x61, 0x76, 0x66, 0x36, 0x32, 0x2e, 0x33, 0x2e, 0x31, 0x30, 0x30,
    ];

    #[test]
    fn vp9_recordings_decode_and_hevc_recordings_report_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vp9_path = dir.path().join("screen.webm");
        std::fs::write(&vp9_path, VP9_WEBM).expect("write vp9 webm");
        assert_eq!(super::recording_supported(&vp9_path).ok(), Some(true));

        let decoded = super::process_video(
            &vp9_path,
            WinnowConfig {
                dhash_threshold: 0,
                scene_cut_threshold: 0,
                min_stride_seconds: 0.0,
                start_reference: None,
            },
        );
        assert!(!decoded.decode_failed);
        assert_eq!((decoded.width, decoded.height), (Some(16), Some(16)));
        assert_eq!(decoded.winnow.map(|metrics| metrics.raw), Some(2));
        assert_eq!(decoded.qualified_frames.len(), 2);

        let mut pull = open(&vp9_path, true);
        pull.select_native_frames(BTreeSet::from([2]));
        let PullNext::Frame(frame) = pull.next(&|| false) else {
            panic!("expected second VP9 frame");
        };
        assert_eq!(frame.frame_id, 2);
        assert!(!frame.png.is_empty());
        assert!(matches!(pull.next(&|| false), PullNext::Done));

        let hevc_path = dir.path().join("screen.mp4");
        std::fs::write(&hevc_path, HEVC_MP4).expect("write hevc mp4");
        assert_eq!(super::recording_supported(&hevc_path).ok(), Some(false));
        assert!(super::process_video(&hevc_path, WinnowConfig::default()).decode_failed);
    }
}
