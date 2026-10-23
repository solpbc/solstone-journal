// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-boundary oracles backed by independent canonical FireRed features,
//! whole-recording graph outputs and sample-domain timestamp expectations.

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
const SAMPLE_RATE: f64 = 16_000.0;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn model_path() -> PathBuf {
    repository_root().join("core/models/assets/firered_vad.onnx")
}

fn helper_binary() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core-vad-analyze")
}

mod support;

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
        "models": {"firered_vad_onnx_path": model_path().to_string_lossy()},
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

#[test]
fn helper_matches_independent_canonical_corpus() {
    let metadata = support::reference();
    let temp = TempDir::new("corpus");
    for case in metadata["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().expect("case name");
        let audio = support::audio(case);
        let path = temp.write_f32le(&format!("{name}.f32le"), &audio);
        let actual = helper_response(&path, SHARED_SEED_MIN_SPEECH_SECONDS);
        assert_identities(&actual, audio.len(), SHARED_SEED_MIN_SPEECH_SECONDS);
        assert_eq!(
            spans(&actual["speech"]),
            support::spans(case)
                .into_iter()
                .map(|(start, end)| (start as u64, end as u64))
                .collect::<Vec<_>>(),
            "{name}: independent sample-domain spans"
        );
    }
}

#[test]
fn has_speech_agrees_at_the_exact_duration_threshold() {
    let metadata = support::reference();
    let case = metadata["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["name"] == "speech_seed")
        .expect("speech fixture");
    let samples = support::audio(case).repeat(2);
    let temp = TempDir::new("tie");
    let path = temp.write_f32le("speech.f32le", &samples);
    let baseline = helper_response(&path, CORPUS_MIN_SPEECH_SECONDS);
    let speech_duration = number(&baseline["speech_duration"]);
    assert!(speech_duration > 0.0);
    assert_eq!(
        helper_response(&path, speech_duration)["has_speech"],
        json!(true)
    );
    let next_up = f64::from_bits(speech_duration.to_bits() + 1);
    assert_eq!(helper_response(&path, next_up)["has_speech"], json!(false));
}
