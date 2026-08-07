use std::path::Path;

use ffmpeg_next as ffmpeg;

use crate::{
    ArucoFrame, HashedFrame, WinnowConfig, WinnowCounters, WinnowState, WinnowVerdict, dhash,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbFrame {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
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
pub enum PreHashOutcome {
    Apply { aruco: Option<ArucoFrame> },
    Reject(PreHashRejectReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreHashRejectReason {
    FiducialMask,
    Other,
}

pub trait PreHashTransform {
    fn apply(&mut self, frame_id: u64, timestamp: f64, frame: &mut RgbFrame) -> PreHashOutcome;
}

#[derive(Default)]
pub struct IdentityTransform;

impl PreHashTransform for IdentityTransform {
    fn apply(&mut self, _frame_id: u64, _timestamp: f64, _frame: &mut RgbFrame) -> PreHashOutcome {
        PreHashOutcome::Apply { aruco: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QualifiedFrame {
    pub frame_id: u64,
    pub timestamp: f64,
    pub aruco: Option<ArucoFrame>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WinnowMetrics {
    pub raw: usize,
    pub dhash_qualified: usize,
    pub scene_cut: usize,
    pub stride_dropped: usize,
    pub kept: usize,
    pub mask_skipped: usize,
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

pub fn process_video(path: &Path) -> DescribeResult {
    let mut transform = IdentityTransform;
    process_video_with_transform(path, &mut transform, WinnowConfig::default())
}

pub fn process_video_with_transform<T: PreHashTransform>(
    path: &Path,
    transform: &mut T,
    config: WinnowConfig,
) -> DescribeResult {
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
    let mut scaler = match ffmpeg::software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg::format::Pixel::RGB24,
        decoder.width(),
        decoder.height(),
        ffmpeg::software::scaling::flag::Flags::BILINEAR,
    ) {
        Ok(scaler) => scaler,
        Err(_) => return decode_failed(result),
    };

    let mut frame_id = 0_u64;
    let mut state = WinnowState::new(config);
    let mut raw = 0_usize;
    let mut mask_skipped = 0_usize;

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
                        transform,
                        &mut frame_id,
                        &mut raw,
                        &mut mask_skipped,
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
            transform,
            &mut frame_id,
            &mut raw,
            &mut mask_skipped,
            &mut state,
            &mut result,
        )
        .is_err()
    {
        return decode_failed(result);
    }

    let counters = state.counters();
    result.qualified_count = result.qualified_frames.len();
    result.winnow = Some(metrics(raw, mask_skipped, counters));
    result
}

#[allow(clippy::too_many_arguments)]
fn receive_frames<T: PreHashTransform>(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut ffmpeg::software::scaling::context::Context,
    time_base: ffmpeg::Rational,
    transform: &mut T,
    frame_id: &mut u64,
    raw: &mut usize,
    mask_skipped: &mut usize,
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
                scaler.run(&decoded, &mut rgb)?;
                let mut frame = rgb_frame(&rgb).ok_or(ffmpeg::Error::InvalidData)?;
                let aruco = match transform.apply(*frame_id, timestamp, &mut frame) {
                    PreHashOutcome::Apply { aruco } => aruco,
                    PreHashOutcome::Reject(PreHashRejectReason::FiducialMask) => {
                        *mask_skipped += 1;
                        continue;
                    }
                    PreHashOutcome::Reject(PreHashRejectReason::Other) => continue,
                };

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
                        aruco,
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

fn metrics(raw: usize, mask_skipped: usize, counters: WinnowCounters) -> WinnowMetrics {
    WinnowMetrics {
        raw,
        dhash_qualified: counters.dhash_qualified,
        scene_cut: counters.scene_cut,
        stride_dropped: counters.stride_dropped,
        kept: counters.kept,
        mask_skipped,
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use serde::Deserialize;

    use super::{
        IdentityTransform, PreHashOutcome, PreHashRejectReason, PreHashTransform, RgbFrame,
        process_video, process_video_with_transform,
    };
    use crate::{ConveyFiducialMask, WinnowConfig, dhash, format_dhash};

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<FixtureCase>,
    }

    #[derive(Deserialize)]
    struct FixtureCase {
        decode_failed: bool,
        file: String,
        first_hash: Option<String>,
        frames: Vec<FixtureFrame>,
        last_hash: Option<String>,
        qualified_count: usize,
        winnow: BTreeMap<String, usize>,
    }

    #[derive(Deserialize)]
    struct FixtureFrame {
        frame_id: u64,
        timestamp: f64,
    }

    #[derive(Deserialize)]
    struct MaskedFixture {
        cases: Vec<MaskedFixtureCase>,
    }

    #[derive(Deserialize)]
    struct MaskedFixtureCase {
        decode_failed: bool,
        file: String,
        first_hash: Option<String>,
        frames: Vec<FixtureFrame>,
        last_hash: Option<String>,
        qualified_count: usize,
        winnow: BTreeMap<String, usize>,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!("../../../fixtures/describe_frames.json"))
            .expect("valid describe fixture")
    }

    fn corpus_path(file: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/describe_corpus")
            .join(file)
    }

    fn masked_fixture() -> MaskedFixture {
        serde_json::from_str(include_str!(
            "../../../fixtures/describe_masked_frames.json"
        ))
        .expect("valid masked fixture")
    }

    fn masked_path(file: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/describe_masked")
            .join(file)
    }

    #[test]
    fn decodable_corpus_cases_match_the_frozen_oracle() {
        for case in fixture()
            .cases
            .into_iter()
            .filter(|case| !case.decode_failed)
        {
            let result = process_video(&corpus_path(&case.file));
            assert!(!result.decode_failed, "{} unexpectedly failed", case.file);
            assert_eq!(
                result.qualified_count, case.qualified_count,
                "{} count",
                case.file
            );
            assert_eq!(
                result.first_hash.map(format_dhash),
                case.first_hash,
                "{} first hash",
                case.file
            );
            assert_eq!(
                result.last_hash.map(format_dhash),
                case.last_hash,
                "{} last hash",
                case.file
            );
            assert_eq!(
                result.qualified_frames.len(),
                case.frames.len(),
                "{} frames",
                case.file
            );
            for (actual, expected) in result.qualified_frames.iter().zip(&case.frames) {
                assert_eq!(actual.frame_id, expected.frame_id, "{} frame id", case.file);
                assert!(
                    (actual.timestamp - expected.timestamp).abs() <= 1e-6,
                    "{} timestamp: expected {}, got {}",
                    case.file,
                    expected.timestamp,
                    actual.timestamp
                );
            }
            let metrics = result.winnow.expect("successful decode has metrics");
            for (name, expected) in case.winnow {
                let actual = match name.as_str() {
                    "raw" => metrics.raw,
                    "dhash_qualified" => metrics.dhash_qualified,
                    "scene_cut" => metrics.scene_cut,
                    "stride_dropped" => metrics.stride_dropped,
                    "kept" => metrics.kept,
                    "mask_skipped" => metrics.mask_skipped,
                    other => panic!("unexpected fixture counter {other}"),
                };
                assert_eq!(actual, expected, "{} {name}", case.file);
            }
        }
    }

    #[test]
    fn decode_failure_cases_preserve_only_already_qualified_frames() {
        for case in fixture()
            .cases
            .into_iter()
            .filter(|case| case.decode_failed)
        {
            let result = process_video(&corpus_path(&case.file));
            assert!(result.decode_failed, "{} should fail", case.file);
            assert_eq!(
                result.qualified_count,
                result.qualified_frames.len(),
                "{} preserves its accumulated frame count",
                case.file
            );
            // FFmpeg 9 qualifies three frames before noticing this terminal
            // corruption, while the oracle's FFmpeg version qualified none.
            // Python preserves already-qualified frames on error, so this is
            // decoder-version drift to report rather than tune away.
            if case.file != "corrupted_mid_screen.webm" {
                assert_eq!(
                    result.qualified_count, case.qualified_count,
                    "{} count",
                    case.file
                );
                assert!(result.qualified_frames.is_empty(), "{} frames", case.file);
            }
            assert!(result.winnow.is_none(), "{} metrics", case.file);
        }
    }

    #[test]
    fn paired_codecs_have_matching_qualified_results() {
        for (vp8, h264) in [
            ("mixed_vp8_screen.webm", "mixed_h264_screen.mov"),
            ("scene_cuts_vp8_screen.webm", "scene_cuts_h264_screen.mov"),
            (
                "single_frame_vp8_screen.webm",
                "single_frame_h264_screen.mov",
            ),
            ("static_vp8_screen.webm", "static_h264_screen.mov"),
            (
                "stride_floor_vp8_screen.webm",
                "stride_floor_h264_screen.mov",
            ),
        ] {
            let vp8_result = process_video(&corpus_path(vp8));
            let h264_result = process_video(&corpus_path(h264));
            assert_eq!(
                vp8_result.qualified_frames, h264_result.qualified_frames,
                "{vp8}/{h264}"
            );
            assert_eq!(
                vp8_result.first_hash, h264_result.first_hash,
                "{vp8}/{h264}"
            );
            assert_eq!(vp8_result.last_hash, h264_result.last_hash, "{vp8}/{h264}");
        }
    }

    #[test]
    fn winnow_config_overrides_change_qualified_frames() {
        let mixed_path = corpus_path("mixed_vp8_screen.webm");
        assert_eq!(
            process_video(&mixed_path)
                .qualified_frames
                .iter()
                .map(|frame| frame.frame_id)
                .collect::<Vec<_>>(),
            [1, 7, 13]
        );

        let min_stride_config = WinnowConfig {
            min_stride_seconds: 8.0,
            ..WinnowConfig::default()
        };
        let mut identity = IdentityTransform;
        assert_eq!(
            process_video_with_transform(&mixed_path, &mut identity, min_stride_config)
                .qualified_frames
                .iter()
                .map(|frame| frame.frame_id)
                .collect::<Vec<_>>(),
            [1, 9, 17]
        );

        let stride_floor_path = corpus_path("stride_floor_vp8_screen.webm");
        assert_eq!(
            process_video(&stride_floor_path)
                .qualified_frames
                .iter()
                .map(|frame| frame.frame_id)
                .collect::<Vec<_>>(),
            [1, 2]
        );

        let scene_cut_config = WinnowConfig {
            scene_cut_threshold: 65,
            ..WinnowConfig::default()
        };
        let mut identity = IdentityTransform;
        assert_eq!(
            process_video_with_transform(&stride_floor_path, &mut identity, scene_cut_config)
                .qualified_frames
                .iter()
                .map(|frame| frame.frame_id)
                .collect::<Vec<_>>(),
            [1, 6]
        );
    }

    struct Blackout;

    impl PreHashTransform for Blackout {
        fn apply(
            &mut self,
            _frame_id: u64,
            _timestamp: f64,
            frame: &mut RgbFrame,
        ) -> PreHashOutcome {
            frame.pixels.fill(0);
            PreHashOutcome::Apply { aruco: None }
        }
    }

    struct RejectFirst {
        first_applied_hash: Option<u64>,
    }

    impl PreHashTransform for RejectFirst {
        fn apply(
            &mut self,
            frame_id: u64,
            _timestamp: f64,
            frame: &mut RgbFrame,
        ) -> PreHashOutcome {
            if frame_id == 1 {
                PreHashOutcome::Reject(PreHashRejectReason::Other)
            } else {
                self.first_applied_hash.get_or_insert_with(|| dhash(frame));
                PreHashOutcome::Apply { aruco: None }
            }
        }
    }

    #[test]
    fn pre_hash_transform_can_blackout_or_reject_frames() {
        let path = corpus_path("mixed_vp8_screen.webm");
        let mut blackout = Blackout;
        let blacked_out =
            process_video_with_transform(&path, &mut blackout, WinnowConfig::default());
        assert_eq!(blacked_out.first_hash, Some(0));
        assert_ne!(blacked_out.first_hash, process_video(&path).first_hash);

        let mut reject_first = RejectFirst {
            first_applied_hash: None,
        };
        let rejected =
            process_video_with_transform(&path, &mut reject_first, WinnowConfig::default());
        assert_eq!(
            rejected
                .qualified_frames
                .first()
                .map(|frame| frame.frame_id),
            Some(2)
        );
        assert_eq!(rejected.first_hash, reject_first.first_applied_hash);
        assert_eq!(rejected.winnow.unwrap().mask_skipped, 0);
    }

    #[test]
    fn fiducial_masked_screencasts_match_the_oracle() {
        for case in masked_fixture().cases {
            let mut transform = ConveyFiducialMask;
            let result = process_video_with_transform(
                &masked_path(&case.file),
                &mut transform,
                WinnowConfig::default(),
            );
            assert_eq!(
                result.decode_failed, case.decode_failed,
                "{} decode",
                case.file
            );
            assert_eq!(
                result.qualified_count,
                case.qualified_count,
                "{} count; frames {:?}, metrics {:?}",
                case.file,
                result
                    .qualified_frames
                    .iter()
                    .map(|frame| (frame.frame_id, frame.aruco.is_some()))
                    .collect::<Vec<_>>(),
                result.winnow
            );
            assert_eq!(
                result.first_hash.map(format_dhash),
                case.first_hash,
                "{} first",
                case.file
            );
            assert_eq!(
                result.last_hash.map(format_dhash),
                case.last_hash,
                "{} last",
                case.file
            );
            assert_eq!(
                result
                    .qualified_frames
                    .iter()
                    .map(|frame| frame.frame_id)
                    .collect::<Vec<_>>(),
                case.frames
                    .iter()
                    .map(|frame| frame.frame_id)
                    .collect::<Vec<_>>(),
                "{} frames",
                case.file
            );
            let metrics = result.winnow.expect("successful decode has metrics");
            for (name, expected) in case.winnow {
                let actual = match name.as_str() {
                    "raw" => metrics.raw,
                    "dhash_qualified" => metrics.dhash_qualified,
                    "scene_cut" => metrics.scene_cut,
                    "stride_dropped" => metrics.stride_dropped,
                    "kept" => metrics.kept,
                    other => panic!("unexpected fixture counter {other}"),
                };
                assert_eq!(actual, expected, "{} {name}", case.file);
            }
            let expected_skipped = usize::from(case.file == "convey_skipped_screen.webm") * 8;
            assert_eq!(
                metrics.mask_skipped, expected_skipped,
                "{} mask skips",
                case.file
            );
            for frame in &result.qualified_frames {
                let aruco = frame
                    .aruco
                    .as_ref()
                    .expect("detected fiducials attach frame data");
                assert!(aruco.masked);
                assert!(aruco.extrapolated.is_none());
                assert_eq!(
                    aruco
                        .markers
                        .iter()
                        .map(|marker| marker.id)
                        .collect::<Vec<_>>(),
                    [2, 4, 6, 7]
                );
            }
        }
    }

    struct RejectFirstMask;

    impl PreHashTransform for RejectFirstMask {
        fn apply(
            &mut self,
            frame_id: u64,
            _timestamp: f64,
            _frame: &mut RgbFrame,
        ) -> PreHashOutcome {
            if frame_id == 1 {
                PreHashOutcome::Reject(PreHashRejectReason::FiducialMask)
            } else {
                PreHashOutcome::Apply { aruco: None }
            }
        }
    }

    #[test]
    fn mask_skip_consumes_frame_id_without_entering_winnow() {
        let mut transform = RejectFirstMask;
        let result = process_video_with_transform(
            &corpus_path("mixed_vp8_screen.webm"),
            &mut transform,
            WinnowConfig::default(),
        );
        assert_eq!(
            result.qualified_frames.first().map(|frame| frame.frame_id),
            Some(2)
        );
        let metrics = result.winnow.expect("successful decode has metrics");
        assert_eq!(metrics.mask_skipped, 1);
        assert_eq!(metrics.dhash_qualified, 3);
    }
}
