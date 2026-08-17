// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native VAD helper oracles. Expected spans were recorded from
//! `solstone-core-vad-analyze` on 2026-08-16 against the committed seed and
//! the pinned Silero VAD v6 graph.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_vad_analyze::{REQUEST_SCHEMA, RESPONSE_SCHEMA};

const THRESHOLD: f64 = 0.3;
const MIN_SILENCE_DURATION_MS: u32 = 1000;
const CORPUS_MIN_SPEECH_SECONDS: f64 = 0.5;
const SHARED_SEED_MIN_SPEECH_SECONDS: f64 = 1.0;
const WINDOW_SIZE_SAMPLES: usize = 512;
const SAMPLE_RATE: f64 = 16_000.0;

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
        .join("core/models/assets/silero_vad_v6.onnx")
}

fn helper_binary() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core-vad-analyze")
}

fn speech_seed() -> Vec<f32> {
    let path = fixtures().join("vad_speech_seed.f32le");
    let bytes = fs::read(&path).unwrap_or_else(|error| {
        panic!("read {}: {error}", path.display());
    });
    assert_eq!(
        bytes.len(),
        16384 * size_of::<f32>(),
        "the committed speech seed must be 16384 f32 samples"
    );
    bytes
        .chunks_exact(size_of::<f32>())
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four bytes")))
        .collect()
}

fn silence(windows: usize) -> Vec<f32> {
    vec![0.0; windows * WINDOW_SIZE_SAMPLES]
}

fn repeat(seed: &[f32], times: usize) -> Vec<f32> {
    seed.repeat(times)
}

fn concat(parts: &[&[f32]]) -> Vec<f32> {
    parts.concat()
}

fn decaying_tail(seed: &[f32]) -> Vec<f32> {
    let tiled = repeat(seed, 3);
    let length = tiled.len() as f64;
    tiled
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let ramp = (-8.0 * index as f64 / length).exp() as f32;
            sample * ramp
        })
        .collect()
}

fn corpus(seed: &[f32]) -> Vec<(&'static str, Vec<f32>)> {
    vec![
        ("pure_silence", silence(32)),
        ("pure_speech", repeat(seed, 4)),
        ("silence_30_windows", concat(&[seed, &silence(30), seed])),
        ("silence_31_windows", concat(&[seed, &silence(31), seed])),
        ("silence_32_windows", concat(&[seed, &silence(32), seed])),
        ("silence_33_windows", concat(&[seed, &silence(33), seed])),
        ("silence_36_windows", concat(&[seed, &silence(36), seed])),
        ("silence_37_windows", concat(&[seed, &silence(37), seed])),
        ("leading_silence", concat(&[&silence(33), seed])),
        ("non_multiple_of_512", concat(&[seed, &[0.0; 37]])),
        ("exact_multiple_of_512", repeat(seed, 2)),
        ("shorter_than_512_samples", seed[..400].to_vec()),
        (
            "speech_near_sample_zero",
            concat(&[&silence(4), seed, &silence(40)]),
        ),
        (
            "decay_through_band",
            concat(&[seed, &decaying_tail(seed), &silence(64)]),
        ),
        ("crosses_320s_and_encoder_batch_boundary", repeat(seed, 313)),
    ]
}

struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "solstone-vad-oracles-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create oracle temp dir");
        Self { root }
    }

    fn write_f32le(&self, name: &str, samples: &[f32]) -> PathBuf {
        let path = self.root.join(name);
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(&path, bytes).expect("write corpus audio");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn helper_response(audio_path: &Path, min_speech_seconds: f64) -> Value {
    let request = json!({
        "schema": REQUEST_SCHEMA,
        "audio_f32le_path": audio_path.to_string_lossy(),
        "models": {"silero_vad_onnx_path": model_path().to_string_lossy()},
        "min_speech_seconds": min_speech_seconds,
        "options": {
            "threshold": THRESHOLD,
            "min_silence_duration_ms": MIN_SILENCE_DURATION_MS,
        },
    });
    let mut child = Command::new(helper_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the native VAD helper");
    child
        .stdin
        .take()
        .expect("helper stdin")
        .write_all(request.to_string().as_bytes())
        .expect("write the request to helper stdin");
    let output = child.wait_with_output().expect("helper exit");
    assert!(
        output.status.success(),
        "helper exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("helper stdout is UTF-8");
    let mut lines = stdout.lines();
    let line = lines.next().expect("helper printed a response line");
    assert_eq!(lines.next(), None, "helper printed more than one line");
    let response: Value = serde_json::from_str(line).expect("helper response JSON");
    assert_eq!(response["schema"], json!(RESPONSE_SCHEMA));
    response
}

fn spans(value: &Value) -> Vec<(u64, u64)> {
    value
        .as_array()
        .expect("speech spans are an array")
        .iter()
        .map(|span| match span {
            Value::Object(map) => (
                map["start"].as_u64().expect("start is an integer"),
                map["end"].as_u64().expect("end is an integer"),
            ),
            Value::Array(pair) => (
                pair[0].as_u64().expect("start is an integer"),
                pair[1].as_u64().expect("end is an integer"),
            ),
            other => panic!("unexpected span shape {other}"),
        })
        .collect()
}

fn number(value: &Value) -> f64 {
    value.as_f64().expect("a JSON number")
}

fn assert_identities(actual: &Value, samples: usize, min_speech_seconds: f64) {
    let speech = spans(&actual["speech"]);
    let speech_samples: u64 = speech.iter().map(|(start, end)| end - start).sum();
    assert_eq!(number(&actual["duration"]), samples as f64 / SAMPLE_RATE);
    assert_eq!(
        number(&actual["speech_duration"]),
        speech_samples as f64 / SAMPLE_RATE
    );
    assert_eq!(
        actual["has_speech"],
        json!(number(&actual["speech_duration"]) >= min_speech_seconds)
    );
}

// Recorded from solstone-core-vad-analyze on 2026-08-16 (linux-x86_64,
// silero_vad_v6, threshold=0.3, min_silence_duration_ms=1000).
fn expected_corpus_spans() -> BTreeMap<&'static str, Vec<(u64, u64)>> {
    BTreeMap::from([
        ("pure_silence", vec![]),
        ("pure_speech", vec![(0, 65536)]),
        ("silence_30_windows", vec![(0, 48128)]),
        ("silence_31_windows", vec![(0, 48640)]),
        ("silence_32_windows", vec![(0, 49152)]),
        ("silence_33_windows", vec![(0, 49664)]),
        ("silence_36_windows", vec![(0, 51200)]),
        ("silence_37_windows", vec![(0, 24832), (28928, 51712)]),
        ("leading_silence", vec![(11008, 33280)]),
        ("non_multiple_of_512", vec![(0, 16421)]),
        ("exact_multiple_of_512", vec![(0, 32768)]),
        ("shorter_than_512_samples", vec![]),
        ("speech_near_sample_zero", vec![(0, 26368)]),
        ("decay_through_band", vec![(0, 52480)]),
        (
            "crosses_320s_and_encoder_batch_boundary",
            vec![(0, 5_128_192)],
        ),
    ])
}

#[test]
fn shared_seed_at_one_second_matches_the_recorded_helper() {
    let seed = speech_seed();
    let temp = TempDir::new("shared-seed");
    let path = temp.write_f32le("vad_speech_seed.f32le", &seed);
    let actual = helper_response(&path, SHARED_SEED_MIN_SPEECH_SECONDS);
    assert_identities(&actual, seed.len(), SHARED_SEED_MIN_SPEECH_SECONDS);
    // Recorded from solstone-core-vad-analyze on 2026-08-16 against
    // core/fixtures/vad_speech_seed.f32le at min_speech_seconds=1.0.
    assert_eq!(spans(&actual["speech"]), vec![(0, 16384)]);
    assert_eq!(number(&actual["duration"]), 16384.0 / SAMPLE_RATE);
    assert_eq!(number(&actual["speech_duration"]), 16384.0 / SAMPLE_RATE);
    assert_eq!(actual["has_speech"], json!(true));
}

#[test]
fn helper_matches_the_recorded_named_corpus() {
    let seed = speech_seed();
    let corpus = corpus(&seed);
    assert_eq!(corpus.len(), 15);
    let expected = expected_corpus_spans();
    assert_eq!(expected.len(), 15);
    let temp = TempDir::new("corpus");
    for (name, samples) in &corpus {
        let path = temp.write_f32le(&format!("{name}.f32le"), samples);
        let actual = helper_response(&path, CORPUS_MIN_SPEECH_SECONDS);
        assert_identities(&actual, samples.len(), CORPUS_MIN_SPEECH_SECONDS);
        assert_eq!(
            spans(&actual["speech"]),
            expected[name],
            "{name}: recorded spans drifted"
        );
        if *name == "pure_silence" {
            assert!(spans(&actual["speech"]).is_empty());
        }
    }
}

#[test]
fn has_speech_agrees_at_the_exact_threshold_tie() {
    let seed = speech_seed();
    let temp = TempDir::new("tie");
    let samples = repeat(&seed, 2);
    let path = temp.write_f32le("exact_multiple_of_512.f32le", &samples);
    let baseline = helper_response(&path, CORPUS_MIN_SPEECH_SECONDS);
    let speech_duration = number(&baseline["speech_duration"]);
    assert!(speech_duration > 0.0);
    let at_exact = helper_response(&path, speech_duration);
    assert_eq!(at_exact["has_speech"], json!(true));
    let next_up = f64::from_bits(speech_duration.to_bits() + 1);
    assert!(next_up > speech_duration);
    let at_next_up = helper_response(&path, next_up);
    assert_eq!(at_next_up["has_speech"], json!(false));
}
