// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_observe_audio::{SAMPLE_RATE, decode_f32_mono};

/// `ffprobe` duration of `parakeet_sample.wav`. The nine checked-in codec
/// fixtures are derived from that file, so they share this expected decoded length.
const SOURCE_DURATION_S: f64 = 2.827_625;
/// `ffprobe` duration of `aac_multi_track.m4a`. `.m4a` takes the mix-all-streams
/// path and does not share the lossless resample slop of the wav/flac renders.
const M4A_DURATION_S: f64 = 1.0;
/// Measured |delta| was 0 on every wav/flac render; 16 samples is resampler-edge pad.
const LOSSLESS_SAMPLE_SLOP: usize = 16;
/// Measured |delta| was 0 on opus/ogg/mp3 this host; 320 (~20 ms) covers encoder priming.
const LOSSY_SAMPLE_SLOP: usize = 320;
/// Measured 16_384 vs 16_000 (delta 384) on the AAC mix path; 448 is 384 + 64 pad.
const M4A_SAMPLE_SLOP: usize = 448;
const MIN_PEAK_ABS: f32 = 0.05;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn fixture_m4a_multi_track() -> PathBuf {
    repository_root().join("tests/fixtures/audio/aac_multi_track.m4a")
}

fn checked_in_decode_corpus() -> Vec<PathBuf> {
    let directory = repository_root().join("core/fixtures/audio_decode_corpus");
    let mut fixtures = [
        "stereo-44100.wav",
        "stereo-44100.flac",
        "stereo-48000.wav",
        "stereo-48000.flac",
        "render.flac",
        "render.opus",
        "render.ogg",
        "render.mp3",
        "render.wav",
    ]
    .map(|name| directory.join(name))
    .to_vec();
    fixtures.push(fixture_m4a_multi_track());
    fixtures
}

fn expected_sample_count(path: &Path) -> usize {
    let duration_s = match path.extension().and_then(|extension| extension.to_str()) {
        Some("m4a") => M4A_DURATION_S,
        _ => SOURCE_DURATION_S,
    };
    (duration_s * f64::from(SAMPLE_RATE)).round() as usize
}

fn sample_count_slop(path: &Path) -> usize {
    match path.extension().and_then(|extension| extension.to_str()) {
        // Lossless PCM/FLAC only have resampler-edge slop after the 16 kHz
        // downmix; a few frames is enough.
        Some("wav" | "flac") => LOSSLESS_SAMPLE_SLOP,
        // Lossy encoders add priming/padding, so the decoded length can drift;
        // 320 samples (~20 ms) covers what this host's encoders produced.
        Some("opus" | "ogg" | "mp3") => LOSSY_SAMPLE_SLOP,
        // `.m4a` mixes every stream and does not flush the resampler.
        Some("m4a") => M4A_SAMPLE_SLOP,
        other => panic!("unexpected corpus extension: {other:?} for {path:?}"),
    }
}

#[test]
fn decode_f32_mono_covers_the_checked_in_codec_corpus() {
    for fixture in checked_in_decode_corpus() {
        let actual = decode_f32_mono(&fixture).expect("decode checked-in corpus fixture");
        let expected = expected_sample_count(&fixture);
        let slop = sample_count_slop(&fixture);
        // A stereo-treated-as-mono bug would roughly double or halve the
        // count and miss every slop; Vec<f32> is already type-level mono.
        assert!(
            actual.len().abs_diff(expected) <= slop,
            "sample count {} off expected {expected} by more than {slop} for {fixture:?}",
            actual.len()
        );
        assert!(
            actual
                .iter()
                .fold(0.0_f32, |acc, sample| acc.max(sample.abs()))
                >= MIN_PEAK_ABS,
            "decoded audio is silent for {fixture:?}"
        );
    }
}
