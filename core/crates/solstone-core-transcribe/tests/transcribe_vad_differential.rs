// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! AC38's real-VAD companion differential, executed only with the ORT runtime.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const REQUEST_SCHEMA: &str = "solstone-vad-request-v1";
const RESPONSE_SCHEMA: &str = "solstone-vad-response-v1";

#[test]
fn real_vad_helper_matches_python_for_the_shared_seed() {
    let root = repository_root();
    let audio = root.join("core/fixtures/vad_speech_seed.f32le");
    let model = root
        .join("packages/solstone-journal-models/solstone_journal_models/assets/silero_vad_v6.onnx");
    let helper = root.join("core/target/debug/solstone-core-vad-analyze");
    assert!(audio.is_file(), "shared VAD input fixture is missing");
    assert!(model.is_file(), "shared Silero model asset is missing");
    assert!(
        helper.is_file(),
        "the preceding vad_differential leg must build the real VAD helper"
    );

    let actual = helper_response(&helper, &audio, &model);
    let expected = python_response(&root, &audio, &model);
    assert_eq!(actual["schema"], RESPONSE_SCHEMA);
    assert_eq!(actual["speech"], expected["speech"]);
    assert_eq!(actual["duration"], expected["duration"]);
    assert_eq!(actual["speech_duration"], expected["speech_duration"]);
    assert_eq!(actual["has_speech"], expected["has_speech"]);
}

fn helper_response(helper: &Path, audio: &Path, model: &Path) -> Value {
    let request = json!({
        "schema": REQUEST_SCHEMA,
        "audio_f32le_path": audio,
        "models": {"silero_vad_onnx_path": model},
        "min_speech_seconds": 1.0,
    });
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start real VAD helper");
    child
        .stdin
        .take()
        .expect("VAD stdin")
        .write_all(request.to_string().as_bytes())
        .expect("send VAD request");
    let output = child.wait_with_output().expect("wait for VAD helper");
    assert!(
        output.status.success(),
        "real VAD helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("real VAD response JSON")
}

fn python_response(root: &Path, audio: &Path, model: &Path) -> Value {
    let script = concat!(
        "import importlib.util, json, numpy as np, os, sys\n",
        "path = os.path.join(os.environ['SOLSTONE_REPO_ROOT'], 'solstone/observe/_silero_vad.py')\n",
        "spec = importlib.util.spec_from_file_location('silero_vad_reference', path)\n",
        "reference = importlib.util.module_from_spec(spec)\n",
        "spec.loader.exec_module(reference)\n",
        "model = reference.SileroVADModel(os.environ['SOLSTONE_SILERO_VAD_MODEL'])\n",
        "reference.get_vad_model = lambda: model\n",
        "audio = np.fromfile(os.environ['SOLSTONE_VAD_AUDIO'], dtype='<f4')\n",
        "chunks = reference.get_speech_timestamps(audio, reference.VadOptions(), sampling_rate=16000)\n",
        "speech_samples = sum(chunk['end'] - chunk['start'] for chunk in chunks)\n",
        "speech_duration = speech_samples / 16000\n",
        "json.dump({'speech': [{'start': c['start'], 'end': c['end']} for c in chunks], 'duration': len(audio) / 16000, 'speech_duration': speech_duration, 'has_speech': speech_duration >= 1.0}, sys.stdout)\n",
    );
    let python = root.join(".venv/bin/python3");
    assert!(
        python.is_file(),
        "make check-differentials must provision .venv"
    );
    let output = Command::new(python)
        .args(["-c", script])
        .env("SOLSTONE_REPO_ROOT", root)
        .env("SOLSTONE_SILERO_VAD_MODEL", model)
        .env("SOLSTONE_VAD_AUDIO", audio)
        .output()
        .expect("start Python VAD reference");
    assert!(
        output.status.success(),
        "Python VAD reference failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Python VAD response JSON")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}
