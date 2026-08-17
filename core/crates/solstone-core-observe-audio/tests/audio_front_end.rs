// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use solstone_core_observe_audio::{
    AudioReduction, SpeechSegment, VadResult, audio_to_wav_bytes, compute_loud_speech_windows,
    compute_nonspeech_rms, decode_f32_mono, get_nonspeech_segments, reduce_audio,
    write_f32le_exclusive,
};

/// PCM-16 WAV captured from Python `audio_to_wav_bytes` for `WRITER_SAMPLES`.
const EXPECTED_WAV: &[u8] = &[
    0x52, 0x49, 0x46, 0x46, 0x34, 0x00, 0x00, 0x00, 0x57, 0x41, 0x56, 0x45, 0x66, 0x6d, 0x74, 0x20,
    0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x80, 0x3e, 0x00, 0x00, 0x00, 0x7d, 0x00, 0x00,
    0x02, 0x00, 0x10, 0x00, 0x64, 0x61, 0x74, 0x61, 0x10, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x80,
    0xfe, 0xff, 0xff, 0xff, 0x00, 0x00, 0x01, 0x00, 0xff, 0x7f, 0xff, 0x7f,
];

/// Little-endian f32 bytes captured from Python `_write_f32le` for `WRITER_SAMPLES`.
const EXPECTED_F32LE: &[u8] = &[
    0x00, 0x00, 0xa0, 0xbf, 0x00, 0x00, 0x80, 0xbf, 0x00, 0x00, 0x40, 0xb8, 0x00, 0x00, 0x80, 0xb7,
    0x00, 0x00, 0x80, 0x37, 0x00, 0x00, 0x40, 0x38, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0xa0, 0x3f,
];

const WRITER_SAMPLES: [f32; 8] = [
    -1.25,
    -1.0,
    -1.5 / 32_768.0,
    -0.5 / 32_768.0,
    0.5 / 32_768.0,
    1.5 / 32_768.0,
    1.0,
    1.25,
];

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn temporary_path(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "solstone-observe-audio-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn read_f32le(path: &Path) -> Vec<f32> {
    fs::read(path)
        .expect("read f32le")
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .collect()
}

#[test]
fn wav_and_f32le_writers_match_the_frozen_python_wire() {
    assert_eq!(
        audio_to_wav_bytes(&WRITER_SAMPLES, 16_000).expect("Rust WAV"),
        EXPECTED_WAV
    );
    let sidecar = temporary_path("sidecar").with_extension("f32le");
    write_f32le_exclusive(&sidecar, &WRITER_SAMPLES).expect("Rust sidecar");
    let actual = fs::read(&sidecar).expect("read Rust sidecar");
    fs::remove_file(sidecar).expect("remove sidecar");
    assert_eq!(actual, EXPECTED_F32LE);
}

#[derive(Debug)]
struct ReducedExpectation {
    sample_count: usize,
    reduced_duration_s: f64,
    segments: &'static [SpeechSegment],
}

struct ReductionCase {
    duration_s: f64,
    speech: &'static [(f64, f64)],
    expected: Option<ReducedExpectation>,
}

#[test]
fn reduction_and_timestamp_restoration_match_the_frozen_python_cases() {
    let cases = [
        ReductionCase {
            duration_s: 4.0,
            speech: &[],
            expected: None,
        },
        ReductionCase {
            duration_s: 4.0,
            speech: &[(0.0, 1.0), (3.0, 4.0)],
            expected: None,
        },
        ReductionCase {
            duration_s: 4.0,
            speech: &[(0.0, 1.0), (2.999, 4.0)],
            expected: None,
        },
        ReductionCase {
            duration_s: 7.0,
            speech: &[(0.0, 1.0), (5.0, 6.0)],
            expected: Some(ReducedExpectation {
                sample_count: 80_000,
                reduced_duration_s: 5.0,
                segments: &[
                    SpeechSegment {
                        original_start_s: 0.0,
                        original_end_s: 1.0,
                        reduced_start_s: 0.0,
                        reduced_end_s: 1.0,
                    },
                    SpeechSegment {
                        original_start_s: 5.0,
                        original_end_s: 6.0,
                        reduced_start_s: 3.0,
                        reduced_end_s: 4.0,
                    },
                ],
            }),
        },
        ReductionCase {
            duration_s: 9.0,
            speech: &[(3.0, 4.0), (5.0, 6.0)],
            expected: Some(ReducedExpectation {
                sample_count: 80_000,
                reduced_duration_s: 5.0,
                segments: &[
                    SpeechSegment {
                        original_start_s: 3.0,
                        original_end_s: 4.0,
                        reduced_start_s: 1.0,
                        reduced_end_s: 2.0,
                    },
                    SpeechSegment {
                        original_start_s: 5.0,
                        original_end_s: 6.0,
                        reduced_start_s: 3.0,
                        reduced_end_s: 4.0,
                    },
                ],
            }),
        },
    ];
    for case in cases {
        let audio: Vec<f32> = (0..(case.duration_s * 16_000.0) as usize)
            .map(|sample| sample as f32)
            .collect();
        let vad = VadResult {
            duration_s: case.duration_s,
            speech_duration_s: 0.0,
            has_speech: !case.speech.is_empty(),
            speech_segments: case.speech.to_vec(),
            noisy_rms: None,
            noisy_s: 0.0,
            loud_windows: 0,
            speech_loud_windows: 0,
        };
        match (reduce_audio(&audio, &vad), case.expected) {
            (None, None) => {}
            (Some((reduced, mapping)), Some(expected)) => {
                assert_eq!(reduced.len(), expected.sample_count);
                assert_eq!(mapping.segments, expected.segments);
                assert!((mapping.reduced_duration_s - expected.reduced_duration_s).abs() <= 1e-12);
            }
            (actual, expected) => panic!("reduction mismatch: {actual:?} vs {expected:?}"),
        }
    }

    let mapping = AudioReduction {
        segments: vec![
            SpeechSegment {
                original_start_s: 3.0,
                original_end_s: 4.0,
                reduced_start_s: 1.0,
                reduced_end_s: 2.0,
            },
            SpeechSegment {
                original_start_s: 8.0,
                original_end_s: 9.0,
                reduced_start_s: 4.0,
                reduced_end_s: 5.0,
            },
        ],
        original_duration_s: 10.0,
        reduced_duration_s: 6.0,
    };
    assert_eq!(
        [0.5, 1.5, 3.0, 5.5].map(|time| mapping.restore_timestamp(time)),
        [2.5, 3.5, 6.0, 9.5]
    );
}

#[test]
fn nonspeech_analysis_and_vad_predicates_match_the_frozen_python_cases() {
    let audio: Vec<f32> = (0..12).map(|sample| sample as f32 / 20.0).collect();
    let speech = [(0.5, 1.0), (2.0, 2.5)];
    assert_eq!(
        get_nonspeech_segments(&speech, 3.0),
        vec![(0.0, 0.5), (1.0, 2.0), (2.5, 3.0)]
    );
    let (rms, duration) = compute_nonspeech_rms(&audio, &speech, 4, 0.5);
    assert!((rms.expect("Rust RMS") - 0.280_524_849_891_662_6).abs() <= 1e-6);
    assert!((duration - 2.0).abs() <= 1e-6);
    assert!(compute_nonspeech_rms(&audio, &speech, 4, 1.0).0.is_some());
    assert_eq!(
        compute_loud_speech_windows(&audio, &speech, 4, 0.5, 0.01),
        (6, 2)
    );
    let vad = VadResult {
        duration_s: 3.0,
        speech_duration_s: 1.0,
        has_speech: true,
        speech_segments: speech.to_vec(),
        noisy_rms: Some(0.01),
        noisy_s: 0.0,
        loud_windows: 3,
        speech_loud_windows: 2,
    };
    assert!(!vad.is_noisy(0.01));
    assert_eq!(vad.loud_speech_ratio().expect("Rust ratio"), 2.0 / 3.0);
}

#[test]
fn m4a_decode_matches_the_frozen_python_mix() {
    let root = repository_root();
    let fixture = root.join("tests/fixtures/audio/aac_multi_track.m4a");
    let expected = read_f32le(&root.join("core/fixtures/audio_m4a_mix_expected.f32le"));
    let actual = decode_f32_mono(&fixture).expect("Rust M4A decode");
    assert_eq!(actual.len(), expected.len());
    let max_abs_difference = actual
        .iter()
        .zip(&expected)
        .map(|(actual, expected)| (actual - expected).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_abs_difference <= 1e-6,
        "Rust M4A decode differs by {max_abs_difference}"
    );
}
