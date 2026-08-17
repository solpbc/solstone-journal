// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use solstone_core_observe_audio::{SAMPLE_RATE, decode_f32_mono};

/// `ffprobe` duration of `parakeet_sample.wav`. The nine ffmpeg renders are
/// derived from that file, so they share this expected decoded length.
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

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-observe-audio-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create temporary test directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn fixture_source() -> PathBuf {
    repository_root().join("solstone/observe/transcribe/_fixtures/parakeet_sample.wav")
}

fn fixture_m4a_multi_track() -> PathBuf {
    repository_root().join("tests/fixtures/audio/aac_multi_track.m4a")
}

fn render_fixture(output: &Path, arguments: &[&str]) {
    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-v")
        .arg("error")
        .args(arguments)
        .arg(output)
        .status()
        .expect("run ffmpeg fixture renderer");
    assert!(
        status.success(),
        "ffmpeg fixture renderer failed for {output:?}"
    );
}

fn generated_decode_corpus(temp: &TempDir) -> Vec<PathBuf> {
    let source = fixture_source();
    let source = source.to_str().expect("UTF-8 source path");
    let mut outputs = Vec::new();
    for (name, rate) in [
        ("stereo-44100.wav", "44100"),
        ("stereo-44100.flac", "44100"),
        ("stereo-48000.wav", "48000"),
        ("stereo-48000.flac", "48000"),
    ] {
        let output = temp.path().join(name);
        render_fixture(&output, &["-i", source, "-ar", rate, "-ac", "2"]);
        outputs.push(output);
    }
    for (name, codec) in [
        ("render.flac", None),
        ("render.opus", Some("libopus")),
        ("render.ogg", Some("libvorbis")),
        ("render.mp3", Some("libmp3lame")),
        ("render.wav", None),
    ] {
        let output = temp.path().join(name);
        let args = match codec {
            Some(codec) => vec!["-i", source, "-ac", "2", "-c:a", codec],
            None => vec!["-i", source, "-ac", "2"],
        };
        render_fixture(&output, &args);
        outputs.push(output);
    }
    outputs.push(fixture_m4a_multi_track());
    outputs
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
        // Lossy encoders add priming/padding, so the decoded length can drift
        // by tens to hundreds of milliseconds.
        Some("opus" | "ogg" | "mp3") => LOSSY_SAMPLE_SLOP,
        // `.m4a` mixes every stream and does not flush the resampler.
        Some("m4a") => M4A_SAMPLE_SLOP,
        other => panic!("unexpected corpus extension: {other:?} for {path:?}"),
    }
}

#[test]
fn decode_f32_mono_covers_the_generated_codec_corpus() {
    let temp = TempDir::new("decode-corpus");
    for fixture in generated_decode_corpus(&temp) {
        let actual = decode_f32_mono(&fixture).expect("decode generated corpus fixture");
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
