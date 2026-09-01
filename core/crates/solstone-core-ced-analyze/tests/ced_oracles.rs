// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real subprocess round-trip for the CED helper: spawns the actual compiled
//! `solstone-core-ced-analyze` binary (not the in-process library functions
//! `src/lib.rs`'s own tests call directly) against a compiled stub engine.
//!
//! What this proves, and what it cannot: this dev build of the helper is
//! dynamically linked like every other dev/test binary in this workspace, so
//! it can `dlopen` the stub `libced.so` here regardless of the class of bug
//! Brief D describes. It proves the stdin/stdout wire contract end to end
//! (argv dispatch, JSON request/response, exit codes) through a genuine
//! process boundary. It does not and cannot prove that a `zig-gnu-2.27`
//! cross-compiled release build of this binary, invoked from a genuinely
//! `static-pie` `solstone-core`, resolves and loads correctly on a real
//! host -- only the release build and on-hardware verification can show that.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use solstone_core_ced_analyze::{
    PROBE_COMMAND, PROBE_REQUEST_SCHEMA, PROBE_RESPONSE_SCHEMA, REQUEST_SCHEMA, RESPONSE_SCHEMA,
};

fn helper_binary() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core-ced-analyze")
}

fn compile_stub(output: &Path, abi: i32) -> bool {
    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".to_owned());
    if Command::new(&compiler).arg("--version").output().is_err() {
        return false;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ced_stub.c");
    let mut command = Command::new(compiler);
    if std::env::consts::OS == "macos" {
        command.arg("-dynamiclib");
    } else {
        command.args(["-shared", "-fPIC"]);
    }
    let output = command
        .arg(format!("-DCED_TEST_ABI={abi}"))
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .expect("start C compiler");
    assert!(
        output.status.success(),
        "compile CED stub failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

fn write_f32le(path: &Path, values: &[f32]) {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    fs::write(path, bytes).expect("write audio");
}

struct Fixture {
    _dir: tempfile::TempDir,
    library: PathBuf,
    model: PathBuf,
}

fn fixture() -> Option<Fixture> {
    let dir = tempfile::tempdir().expect("temp dir");
    let library = dir.path().join("libced.so");
    if !compile_stub(&library, 1) {
        eprintln!("skipping: no usable C compiler");
        return None;
    }
    let model = dir.path().join("model.gguf");
    fs::write(&model, b"real model bytes").unwrap();
    Some(Fixture {
        _dir: dir,
        library,
        model,
    })
}

fn run_helper(args: &[&str], request: &Value) -> (bool, Value, String) {
    let mut child = Command::new(helper_binary())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the native CED helper");
    child
        .stdin
        .take()
        .expect("helper stdin")
        .write_all(request.to_string().as_bytes())
        .expect("write request to helper stdin");
    let output = child.wait_with_output().expect("helper exit");
    let stdout = String::from_utf8(output.stdout).expect("helper stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("helper stderr is UTF-8");
    let parsed = if output.status.success() {
        let mut lines = stdout.lines();
        let line = lines.next().unwrap_or_default();
        assert_eq!(lines.next(), None, "helper printed more than one line");
        serde_json::from_str(line).unwrap_or(Value::Null)
    } else {
        stderr
            .lines()
            .next()
            .and_then(|line| serde_json::from_str(line).ok())
            .unwrap_or(Value::Null)
    };
    (output.status.success(), parsed, stderr)
}

#[test]
fn real_subprocess_probe_succeeds_against_a_loadable_stub() {
    let Some(fixture) = fixture() else { return };
    let request = json!({
        "schema": PROBE_REQUEST_SCHEMA,
        "models": {
            "ced_library_path": fixture.library,
            "ced_model_path": fixture.model,
        },
    });
    let (ok, response, stderr) = run_helper(&[PROBE_COMMAND], &request);
    assert!(ok, "probe should succeed: {stderr}");
    assert_eq!(
        response,
        json!({"schema": PROBE_RESPONSE_SCHEMA, "ok": true})
    );
}

#[test]
fn real_subprocess_probe_reports_unloadable_for_a_wrong_abi_stub() {
    let dir = tempfile::tempdir().expect("temp dir");
    let library = dir.path().join("libced.so");
    if !compile_stub(&library, 2) {
        eprintln!("skipping: no usable C compiler");
        return;
    }
    let model = dir.path().join("model.gguf");
    fs::write(&model, b"model").unwrap();
    let request = json!({
        "schema": PROBE_REQUEST_SCHEMA,
        "models": {"ced_library_path": library, "ced_model_path": model},
    });
    let (ok, response, _stderr) = run_helper(&[PROBE_COMMAND], &request);
    assert!(!ok, "a wrong-ABI engine must not report ready");
    assert_eq!(response["reason"], json!("library-unloadable"));
}

#[test]
fn real_subprocess_classify_matches_the_two_window_contract() {
    let Some(fixture) = fixture() else { return };
    let dir = tempfile::tempdir().expect("temp dir");
    let audio_path = dir.path().join("audio.f32le");
    // Window 0: a leading negative sample trips the stub's classify failure.
    // Window 1: ordinary samples classify successfully.
    let mut audio = vec![-1.0_f32; 16_000];
    audio.extend(vec![0.0_f32; 16_000]);
    write_f32le(&audio_path, &audio);

    let request = json!({
        "schema": REQUEST_SCHEMA,
        "models": {
            "ced_library_path": fixture.library,
            "ced_model_path": fixture.model,
        },
        "audio_f32le_path": audio_path,
        "sample_rate_hz": 16_000,
        "top_k": 0,
        "windows": [
            {"start_sample": 0, "end_sample": 16_000},
            {"start_sample": 16_000, "end_sample": 32_000},
        ],
    });
    let (ok, response, stderr) = run_helper(&[], &request);
    assert!(ok, "classify request itself should succeed: {stderr}");
    assert_eq!(response["schema"], json!(RESPONSE_SCHEMA));
    assert_eq!(response["windows"][0]["ok"], json!(false));
    assert_eq!(response["windows"][1]["ok"], json!(true));
    assert_eq!(response["windows"][1]["tags"]["Music"], json!(0.9));
}

// The usage-error path (`evaluate_args` rejects an unknown argument) exits
// before the helper ever reads stdin, which races this test harness's stdin
// write against the child's exit -- `argv_accepts_bare_and_probe_and_rejects_unknown`
// in `src/lib.rs` covers it deterministically in-process instead.
