// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Proves the native Silero VAD helper agrees with the Python it replaces.
//!
//! The oracle is the vendored reference itself --
//! `solstone/observe/_silero_vad.py::get_speech_timestamps` at the settings
//! `solstone/observe/vad.py::run_vad` uses in production (`threshold=0.3`,
//! `min_silence_duration_ms=1000`, `VadOptions` defaults elsewhere) -- and the
//! Rust side is exercised through the helper's real stdin/stdout contract, not
//! through its internals, because what ships is the process.
//!
//! Both sides read the *same file*. The corpus is built once here, from a
//! single committed seed (`core/fixtures/vad_speech_seed.f32le`), written to
//! disk, and handed to Rust and Python by path. Independently synthesising
//! "the same" audio on each side would let a synthesis difference masquerade
//! as agreement -- or as divergence.
//!
//! Every comparison is exact. There is no tolerance anywhere in this file: the
//! spans are integers, the durations are the same two divisions by 16000 in
//! both languages, and the frozen probability oracle is compared on raw float
//! bits.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_vad_analyze::{
    REQUEST_SCHEMA, RESPONSE_SCHEMA, SAMPLE_RATE_HZ, SILERO_VAD_V6_SHA256,
    loaded_onnx_runtime_version, speech_probabilities,
};

/// Production `run_vad` setting; the helper's own default, restated here so a
/// silent default change cannot quietly move what this differential covers.
const THRESHOLD: f64 = 0.3;
const MIN_SILENCE_DURATION_MS: u32 = 1000;
const MIN_SPEECH_SECONDS: f64 = 0.5;

const WINDOW_SIZE_SAMPLES: usize = 512;
/// `speech_pad_ms=400` at 16 kHz; the corpus names a case around this radius.
const SPEECH_PAD_SAMPLES: usize = 6400;
/// `speech_near_sample_zero` only tests the start clamp while the four windows
/// of silence it opens with sit inside the pad radius.
const _: () = assert!(4 * WINDOW_SIZE_SAMPLES < SPEECH_PAD_SAMPLES);

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

fn python() -> PathBuf {
    let venv = repository_root().join(".venv/bin/python3");
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

fn helper_binary() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core-vad-analyze")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
            "solstone-vad-differential-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create differential temp dir");
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

// ---------------------------------------------------------------------------
// Corpus construction
// ---------------------------------------------------------------------------

/// The committed voiced tile every speech-bearing case is built from.
///
/// Generated once by `scripts/generate_vad_fixtures.py` (see that script's
/// docstring for the synthesis and why a stationary tone will not do) and
/// committed as raw little-endian f32. It is 16384 samples -- exactly 32
/// encoder windows -- and periodic over its own length, so repeating it joins
/// without a discontinuity.
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

/// The seed repeated three times under an exponential amplitude decay.
///
/// A linear fade crosses the `[neg_threshold, threshold)` band in one or two
/// windows, which proves nothing about the band. Decaying geometrically walks
/// the probability down through it: measured against the committed seed this
/// spends eight windows inside `[0.15, 0.3)`, dips below and comes back --
/// which also exercises the rule that only a probability at or above
/// `threshold` clears `temp_end`, never a mere return above `neg_threshold`.
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

/// Every named case, in a fixed order, as `(name, samples)`.
fn corpus(seed: &[f32]) -> Vec<(&'static str, Vec<f32>)> {
    vec![
        // No speech at all: the pipeline must return an empty span list.
        ("pure_silence", silence(32)),
        // 128 windows of contiguous voiced audio.
        ("pure_speech", repeat(seed, 4)),
        // Silence runs that stay under the close threshold and merge. The
        // reference closes a segment only once `512*i - temp_end >=
        // min_silence_samples`, and `temp_end` lands where the *probability*
        // drops below neg_threshold, several windows into the silence.
        ("silence_30_windows", concat(&[seed, &silence(30), seed])),
        ("silence_31_windows", concat(&[seed, &silence(31), seed])),
        ("silence_32_windows", concat(&[seed, &silence(32), seed])),
        ("silence_33_windows", concat(&[seed, &silence(33), seed])),
        // The measured window-quantisation boundary for this seed: 36 merges,
        // 37 splits. It is not the naive 16000/512 = 31.25 -> 32 windows,
        // because Silero's offset decay puts `temp_end` four windows into the
        // silence rather than at its first sample.
        ("silence_36_windows", concat(&[seed, &silence(36), seed])),
        ("silence_37_windows", concat(&[seed, &silence(37), seed])),
        // A first chunk whose padded start does not clamp at zero.
        ("leading_silence", concat(&[&silence(33), seed])),
        // 16421 samples: the final window is partial, so the reference pads
        // only the remainder.
        ("non_multiple_of_512", concat(&[seed, &[0.0; 37]])),
        // 32768 samples: already a whole number of windows, so the reference
        // pads a *full* extra window rather than none.
        ("exact_multiple_of_512", repeat(seed, 2)),
        // Below one window: padding produces a single window of mostly zeros.
        ("shorter_than_512_samples", seed[..400].to_vec()),
        // Speech begins 2048 samples in, inside the 6400-sample pad radius, so
        // the padded start clamps at zero.
        (
            "speech_near_sample_zero",
            concat(&[&silence(4), seed, &silence(40)]),
        ),
        // Probability walks down through [neg_threshold, threshold), then
        // stays below neg_threshold for well over a second, then silence.
        (
            "decay_through_band",
            concat(&[seed, &decaying_tail(seed), &silence(64)]),
        ),
        // 5_128_192 samples = 320.512 s = 10016 windows, 10017 after padding:
        // past both 320 seconds and the 10_000-window encoder batch size, so
        // the LSTM state has to carry from the first encoder batch into the
        // second.
        ("crosses_320s_and_encoder_batch_boundary", repeat(seed, 313)),
    ]
}

// ---------------------------------------------------------------------------
// The Python oracle
// ---------------------------------------------------------------------------

/// Runs the real reference over each `(name, path, min_speech_seconds)` case.
///
/// `_silero_vad.py` is loaded directly by file path with `importlib` rather
/// than imported as `solstone.observe._silero_vad`, so the oracle drags in
/// nothing beyond what that one module needs; a package import would execute
/// `solstone/observe/__init__.py`'s whole transitive graph as a side effect of
/// layout. `get_vad_model` is replaced with one that opens the *same* model
/// file this test hands the helper, so both sides provably run one graph --
/// the reference's own resolver would find its own copy.
///
/// The three arithmetic lines after `get_speech_timestamps` are
/// `solstone/observe/vad.py::run_vad`'s, restated so `speech_duration` and
/// `has_speech` are computed by Python rather than re-derived in Rust and
/// compared against themselves.
fn python_oracle(cases: &Value) -> Value {
    let script = concat!(
        "import hashlib, importlib.util, json, math, os, sys\n",
        "import numpy as np\n",
        "path = os.path.join(\n",
        "    os.environ['SOLSTONE_REPO_ROOT'],\n",
        "    'solstone/observe/_silero_vad.py',\n",
        ")\n",
        "spec = importlib.util.spec_from_file_location('silero_vad_reference', path)\n",
        "reference = importlib.util.module_from_spec(spec)\n",
        "spec.loader.exec_module(reference)\n",
        "model_path = os.environ['SOLSTONE_SILERO_VAD_MODEL']\n",
        "model = reference.SileroVADModel(model_path)\n",
        "reference.get_vad_model = lambda: model\n",
        "sample_rate = 16000\n",
        "options = reference.VadOptions(threshold=0.3, min_silence_duration_ms=1000)\n",
        "results = {}\n",
        "for case in json.load(sys.stdin):\n",
        "    audio = np.fromfile(case['path'], dtype='<f4')\n",
        "    chunks = reference.get_speech_timestamps(audio, options, sampling_rate=sample_rate)\n",
        "    speech_samples = sum(c['end'] - c['start'] for c in chunks)\n",
        "    speech_duration = speech_samples / sample_rate\n",
        "    minimum = case['min_speech_seconds']\n",
        "    results[case['name']] = {\n",
        "        'duration': len(audio) / sample_rate,\n",
        "        'speech_duration': speech_duration,\n",
        "        'has_speech': speech_duration >= minimum,\n",
        "        'speech': [[c['start'], c['end']] for c in chunks],\n",
        "        'min_speech_seconds_exact': speech_duration,\n",
        "        'min_speech_seconds_next_up': math.nextafter(speech_duration, math.inf),\n",
        "        'has_speech_at_exact': speech_duration >= speech_duration,\n",
        "        'has_speech_at_next_up': speech_duration >= math.nextafter(speech_duration, math.inf),\n",
        "    }\n",
        "import onnxruntime\n",
        "json.dump({\n",
        "    'onnxruntime_version': onnxruntime.__version__,\n",
        "    'model_sha256': hashlib.sha256(open(model_path, 'rb').read()).hexdigest(),\n",
        "    'cases': results,\n",
        "}, sys.stdout)\n",
    );
    let mut child = Command::new(python())
        .args(["-c", script])
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .env("SOLSTONE_SILERO_VAD_MODEL", model_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the Python VAD oracle");
    child
        .stdin
        .take()
        .expect("Python stdin")
        .write_all(cases.to_string().as_bytes())
        .expect("write the corpus manifest to Python stdin");
    let output = child.wait_with_output().expect("Python oracle exit");
    assert!(
        output.status.success(),
        "Python oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Python oracle JSON output")
}

// ---------------------------------------------------------------------------
// The helper's real contract
// ---------------------------------------------------------------------------

/// Runs the shipped binary over its stdin/stdout contract and returns the
/// single response line it prints.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// AC-1 and AC-11: the helper's spans, and the duration arithmetic layered on
/// them, match the Python reference exactly on every named case.
#[test]
fn helper_matches_the_python_reference_across_the_named_corpus() {
    let seed = speech_seed();
    let corpus = corpus(&seed);
    let temp = TempDir::new("corpus");

    let mut paths = BTreeMap::new();
    let mut manifest = Vec::new();
    for (name, samples) in &corpus {
        let path = temp.write_f32le(&format!("{name}.f32le"), samples);
        manifest.push(json!({
            "name": name,
            "path": path.to_string_lossy(),
            "min_speech_seconds": MIN_SPEECH_SECONDS,
        }));
        paths.insert(*name, path);
    }

    let oracle = python_oracle(&Value::Array(manifest));
    assert_eq!(
        oracle["model_sha256"],
        json!(SILERO_VAD_V6_SHA256),
        "the Python oracle ran a different graph than the helper's pinned model"
    );

    let mut report = Vec::new();
    for (name, samples) in &corpus {
        let expected = &oracle["cases"][name];
        assert!(
            !expected.is_null(),
            "the Python oracle returned no result for {name}"
        );
        let actual = helper_response(&paths[name], MIN_SPEECH_SECONDS);

        let expected_spans = spans(&expected["speech"]);
        let actual_spans = spans(&actual["speech"]);
        assert_eq!(
            actual_spans, expected_spans,
            "{name}: helper spans differ from the Python reference"
        );
        assert_eq!(
            number(&actual["duration"]),
            number(&expected["duration"]),
            "{name}: duration differs"
        );
        assert_eq!(
            number(&actual["speech_duration"]),
            number(&expected["speech_duration"]),
            "{name}: speech_duration differs"
        );
        assert_eq!(
            actual["has_speech"], expected["has_speech"],
            "{name}: has_speech differs"
        );
        assert_eq!(
            actual["min_speech_seconds"],
            json!(MIN_SPEECH_SECONDS),
            "{name}: the response must echo the requested minimum"
        );

        report.push(format!(
            "{name}: samples={} spans={:?} speech_duration={} has_speech={}",
            samples.len(),
            actual_spans,
            number(&actual["speech_duration"]),
            actual["has_speech"],
        ));
    }
    println!("{}", report.join("\n"));

    // A corpus that silently lost a case would still pass every assertion above.
    assert_eq!(
        corpus.len(),
        15,
        "the named corpus lost or gained a case without this count being updated"
    );
}

/// AC-11 boundary: `has_speech` is a `>=` against a float, and both sides must
/// agree on the exact tie. Python supplies both the tie value and its next
/// representable neighbour, plus the verdicts it reaches for each.
#[test]
fn has_speech_agrees_with_python_at_the_exact_threshold_tie() {
    let seed = speech_seed();
    let temp = TempDir::new("tie");
    let path = temp.write_f32le("exact_multiple_of_512.f32le", &repeat(&seed, 2));

    let oracle = python_oracle(&json!([{
        "name": "exact_multiple_of_512",
        "path": path.to_string_lossy(),
        "min_speech_seconds": MIN_SPEECH_SECONDS,
    }]));
    let case = &oracle["cases"]["exact_multiple_of_512"];
    let exact = number(&case["min_speech_seconds_exact"]);
    let next_up = number(&case["min_speech_seconds_next_up"]);
    assert!(exact > 0.0, "the tie case must actually contain speech");
    assert!(next_up > exact, "Python's nextafter must step upward");

    let at_exact = helper_response(&path, exact);
    assert_eq!(
        at_exact["has_speech"], case["has_speech_at_exact"],
        "helper disagrees with Python at min_speech_seconds == speech_duration"
    );
    let at_next_up = helper_response(&path, next_up);
    assert_eq!(
        at_next_up["has_speech"], case["has_speech_at_next_up"],
        "helper disagrees with Python one ULP above speech_duration"
    );
    println!(
        "tie: speech_duration={exact} has_speech_at_exact={} has_speech_one_ulp_above={}",
        at_exact["has_speech"], at_next_up["has_speech"],
    );
}

/// AC-6: the helper must run the same ONNX Runtime the Python oracle does. A
/// silent divergence here would make every other comparison in this file a
/// comparison between two different runtimes.
#[test]
fn loaded_onnx_runtime_matches_the_python_package_version() {
    // Force the runtime to be mapped before the version is read.
    let probe = speech_probabilities(&[0.0_f32; WINDOW_SIZE_SAMPLES], &model_path())
        .expect("probe the committed model");
    assert!(!probe.is_empty());

    let rust_version = loaded_onnx_runtime_version().expect("loaded ONNX Runtime version");
    let oracle = python_oracle(&json!([]));
    let python_version = oracle["onnxruntime_version"]
        .as_str()
        .expect("the oracle reports its onnxruntime version");

    assert_eq!(
        rust_version, python_version,
        "the helper loaded ONNX Runtime {rust_version} while the Python oracle used \
         {python_version}; the differential would be comparing two runtimes"
    );
    println!("onnxruntime: rust={rust_version} python={python_version}");
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
