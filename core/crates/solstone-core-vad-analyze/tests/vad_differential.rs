// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Frozen Silero VAD probability replay. The Python-spawning corpus tests that
//! used to share this file now live in `vad_oracles.rs`.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_vad_analyze::{SAMPLE_RATE_HZ, SILERO_VAD_V6_SHA256, speech_probabilities};

const WINDOW_SIZE_SAMPLES: usize = 512;
const ORACLE_SCHEMA: &str = "solstone-vad-probability-oracle-v1";
const ORACLE_ARCHITECTURE: &str = "linux-x86_64";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn fixtures() -> PathBuf {
    repository_root().join("core/fixtures")
}

fn model_path() -> PathBuf {
    repository_root()
        .join("packages/solstone-journal-models/solstone_journal_models/assets/silero_vad_v6.onnx")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn repeat(seed: &[f32], times: usize) -> Vec<f32> {
    seed.repeat(times)
}

fn number(value: &Value) -> f64 {
    value.as_f64().expect("a JSON number")
}

/// AC-3: the frozen per-window probability sequence replays bit-for-bit.
///
/// Spans hide per-window drift -- padding, merging, and the pad radius all
/// absorb small probability differences -- so this compares the raw encoder
/// output, on `f32::to_bits`, with no tolerance. The buffer is reconstructed
/// by reading the *committed* seed and tiling it exactly as
/// `scripts/generate_vad_fixtures.py` did; regenerating the seed here would
/// test the synthesis rather than the model.
#[test]
fn frozen_probability_oracle_replays_bit_for_bit() {
    let oracle: Value = serde_json::from_slice(
        &fs::read(fixtures().join("vad_probability_oracle.json")).expect("read the oracle fixture"),
    )
    .expect("oracle fixture JSON");

    assert_eq!(oracle["schema"], json!(ORACLE_SCHEMA));
    assert_eq!(
        oracle["architecture"],
        json!(ORACLE_ARCHITECTURE),
        "the oracle was frozen on another architecture"
    );
    assert_eq!(
        (std::env::consts::OS, std::env::consts::ARCH),
        ("linux", "x86_64"),
        "the frozen oracle covers {ORACLE_ARCHITECTURE} only"
    );
    assert_eq!(oracle["model_sha256"], json!(SILERO_VAD_V6_SHA256));
    assert_eq!(oracle["sample_rate_hz"], json!(SAMPLE_RATE_HZ));
    assert_eq!(oracle["window_size_samples"], json!(WINDOW_SIZE_SAMPLES));

    let recorded_runtime = oracle["onnxruntime_version"]
        .as_str()
        .expect("the oracle records its ONNX Runtime version");
    let seed_bytes = fs::read(fixtures().join("vad_probability_seed.f32le"))
        .expect("read the committed probability seed");
    assert_eq!(
        sha256_hex(&seed_bytes),
        oracle["seed_sha256"].as_str().expect("seed digest"),
        "the committed probability seed is not the one the oracle was frozen against"
    );
    let seed_samples = oracle["seed_samples"].as_u64().expect("seed sample count") as usize;
    assert_eq!(seed_bytes.len(), seed_samples * size_of::<f32>());

    let seed: Vec<f32> = seed_bytes
        .chunks_exact(size_of::<f32>())
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four bytes")))
        .collect();
    let tile_count = oracle["tile_count"].as_u64().expect("tile count") as usize;
    let audio = repeat(&seed, tile_count);
    assert_eq!(
        audio.len(),
        oracle["raw_samples"].as_u64().expect("raw sample count") as usize,
        "the replayed tiling does not reproduce the oracle's input length"
    );

    let expected: Vec<f32> = oracle["speech_probabilities"]
        .as_array()
        .expect("frozen probabilities")
        .iter()
        .map(|value| number(value) as f32)
        .collect();
    let window_count = oracle["window_count"].as_u64().expect("window count") as usize;
    assert_eq!(expected.len(), window_count);
    assert!(
        window_count > oracle["encoder_batch_size"].as_u64().expect("batch size") as usize,
        "the oracle must cross the encoder batch seam to cover the carried LSTM state"
    );

    let actual = speech_probabilities(&audio, &model_path()).expect("replay the probability path");
    assert_eq!(
        actual.len(),
        expected.len(),
        "replay produced {} windows, the oracle froze {}",
        actual.len(),
        expected.len()
    );

    let divergences: Vec<String> = actual
        .iter()
        .zip(expected.iter())
        .enumerate()
        .filter(|(_index, (left, right))| left.to_bits() != right.to_bits())
        .take(8)
        .map(|(index, (left, right))| {
            format!(
                "window {index}: replay {left:?} (0x{:08x}) vs frozen {right:?} (0x{:08x})",
                left.to_bits(),
                right.to_bits()
            )
        })
        .collect();
    assert!(
        divergences.is_empty(),
        "the probability replay is not bit-identical to the oracle frozen on \
         onnxruntime {recorded_runtime}:\n{}",
        divergences.join("\n")
    );
    println!(
        "probability oracle: {window_count} windows replayed bit-for-bit from {tile_count} tiles \
         of {seed_samples} samples (frozen on onnxruntime {recorded_runtime})"
    );
}
