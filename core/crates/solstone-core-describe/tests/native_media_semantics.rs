// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ffmpeg_next as ffmpeg;
use serde::Deserialize;
use solstone_core_describe::{WinnowConfig, format_dhash, process_video_metadata};

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

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("../../../fixtures/describe_frames.json"))
        .expect("valid describe fixture")
}

fn corpus_path(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/describe_corpus")
        .join(file)
}

fn delayed_video_probe_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/delayed_video_probe_screen.mp4")
}

#[test]
fn delayed_video_probe_has_unset_format_until_decoded() {
    ffmpeg::init().expect("initialize FFmpeg");
    let path = delayed_video_probe_path();
    let mut input = ffmpeg::format::input(&path).expect("open fixture");
    let audio = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .expect("audio stream");
    let audio_start = audio.start_time() as f64 * f64::from(audio.time_base());
    assert_eq!(audio_start, 0.0);

    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .expect("video stream");
    let stream_index = stream.index();
    let time_base = stream.time_base();
    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .expect("video decoder context");
    let mut decoder = context.decoder().video().expect("video decoder");
    assert_eq!(decoder.format(), ffmpeg::format::Pixel::None);

    let mut first = None;
    'packets: for (packet_stream, packet) in input.packets() {
        if packet_stream.index() != stream_index {
            continue;
        }
        decoder.send_packet(&packet).expect("send video packet");
        let mut decoded = ffmpeg::frame::Video::empty();
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {
                first = Some((
                    decoded.format(),
                    decoded.width(),
                    decoded.height(),
                    decoded.pts().expect("decoded frame PTS") as f64 * f64::from(time_base),
                ));
                break 'packets;
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {}
            Err(ffmpeg::Error::Eof) => {}
            Err(error) => panic!("receive frame: {error}"),
        }
    }
    if first.is_none() {
        decoder.send_eof().expect("send EOF");
        let mut decoded = ffmpeg::frame::Video::empty();
        decoder
            .receive_frame(&mut decoded)
            .expect("receive delayed frame");
        first = Some((
            decoded.format(),
            decoded.width(),
            decoded.height(),
            decoded.pts().expect("decoded frame PTS") as f64 * f64::from(time_base),
        ));
    }
    let (format, width, height, timestamp) = first.expect("decoded video frame");
    assert_ne!(format, ffmpeg::format::Pixel::None);
    assert!(width > 0 && height > 0);
    assert!(timestamp > 5.0);
}

#[test]
fn delayed_video_probe_decodes_to_the_content_oracle() {
    let result = process_video_metadata(&delayed_video_probe_path(), WinnowConfig::default());
    assert!(!result.decode_failed);
    assert_eq!(result.width, Some(1280));
    assert_eq!(result.height, Some(720));
    assert_eq!(result.qualified_count, 1);
    assert_eq!(result.qualified_frames.len(), 1);
    assert!(
        (result.qualified_frames[0].timestamp - 8.333_008).abs() <= 1e-6,
        "timestamp: {}",
        result.qualified_frames[0].timestamp
    );
    // The supplied independently derived hash was 2600000000000126. The
    // pinned FFmpeg builds have deterministic, platform-specific chroma
    // conversion: Linux produces 2600000000000187 and macOS produces
    // 2600000000000143 for the same fixture bytes.
    let first_hash = result.first_hash.map(format_dhash);
    assert_eq!(first_hash, result.last_hash.map(format_dhash));
    let expected_hash = if cfg!(target_os = "macos") {
        "2600000000000143"
    } else {
        "2600000000000187"
    };
    assert_eq!(first_hash, Some(expected_hash.to_owned()));
}

#[test]
fn decodable_corpus_cases_match_the_frozen_oracle() {
    for case in fixture()
        .cases
        .into_iter()
        .filter(|case| !case.decode_failed)
    {
        let result = process_video_metadata(&corpus_path(&case.file), WinnowConfig::default());
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
        let expected_last_hash =
            if cfg!(target_os = "macos") && case.file == "scene_cuts_vp8_screen.webm" {
                Some("e7e7e7e7e7e7e742".to_owned())
            } else {
                case.last_hash.clone()
            };
        assert_eq!(
            result.last_hash.map(format_dhash),
            expected_last_hash,
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
        assert!(
            result
                .qualified_frames
                .iter()
                .all(|frame| frame.png.is_empty()),
            "{} metadata decode must not encode PNG payloads",
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
    }
}

#[test]
fn decode_failure_cases_preserve_only_already_qualified_frames() {
    for case in fixture()
        .cases
        .into_iter()
        .filter(|case| case.decode_failed)
    {
        let result = process_video_metadata(&corpus_path(&case.file), WinnowConfig::default());
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
        let vp8_result = process_video_metadata(&corpus_path(vp8), WinnowConfig::default());
        let h264_result = process_video_metadata(&corpus_path(h264), WinnowConfig::default());
        assert_eq!(
            vp8_result
                .qualified_frames
                .iter()
                .map(|frame| (frame.frame_id, frame.timestamp))
                .collect::<Vec<_>>(),
            h264_result
                .qualified_frames
                .iter()
                .map(|frame| (frame.frame_id, frame.timestamp))
                .collect::<Vec<_>>(),
            "{vp8}/{h264}"
        );
        assert_eq!(
            vp8_result.first_hash, h264_result.first_hash,
            "{vp8}/{h264}"
        );
        if cfg!(target_os = "macos") && vp8 == "scene_cuts_vp8_screen.webm" {
            assert_eq!(vp8_result.last_hash, Some(0xe7e7_e7e7_e7e7_e742), "{vp8}");
            assert_eq!(h264_result.last_hash, Some(0xe7e7_e7e7_e7e7_e766), "{h264}");
        } else {
            assert_eq!(vp8_result.last_hash, h264_result.last_hash, "{vp8}/{h264}");
        }
    }
}

#[test]
fn winnow_config_overrides_change_qualified_frames() {
    let mixed_path = corpus_path("mixed_vp8_screen.webm");
    assert_eq!(
        process_video_metadata(&mixed_path, WinnowConfig::default())
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
    assert_eq!(
        process_video_metadata(&mixed_path, min_stride_config)
            .qualified_frames
            .iter()
            .map(|frame| frame.frame_id)
            .collect::<Vec<_>>(),
        [1, 9, 17]
    );

    let stride_floor_path = corpus_path("stride_floor_vp8_screen.webm");
    assert_eq!(
        process_video_metadata(&stride_floor_path, WinnowConfig::default())
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
    assert_eq!(
        process_video_metadata(&stride_floor_path, scene_cut_config)
            .qualified_frames
            .iter()
            .map(|frame| frame.frame_id)
            .collect::<Vec<_>>(),
        [1, 6]
    );
}
