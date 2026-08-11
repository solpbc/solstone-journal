// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "solstone-core-brain-runtime-failure-{name}-{stamp}"
    ))
}

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("test path has parent")).expect("create parent");
    fs::write(path, contents).expect("write test file");
}

fn run(root: &Path, stdin: &[u8]) -> Output {
    let mut child = Command::new(bin())
        .args(["brain", "record-runtime-failure", "--journal"])
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("solstone-core should execute");
    child
        .stdin
        .as_mut()
        .expect("stdin should be piped")
        .write_all(stdin)
        .expect("write stdin");
    child.wait_with_output().expect("wait for solstone-core")
}

fn configured_journal(
    name: &str,
    bundled_runtime_fingerprint_sha256: Option<Value>,
) -> (PathBuf, String) {
    let root = temp_path(name);
    fs::create_dir(&root).expect("create root");
    let config = json!({
        "env": {"ANTHROPIC_API_KEY": "sk-test"},
        "providers": {"active": {"provider": "anthropic"}},
    });
    write(
        &root,
        "config/journal.json",
        &serde_json::to_vec(&config).unwrap(),
    );
    let key = [7_u8; 32];
    write(&root, "health/brain-fingerprint.key", &key);
    let fingerprint = solstone_core_brain::build_active_brain_fingerprint(
        config.as_object().expect("config object"),
        &key,
        bundled_runtime_fingerprint_sha256,
    )
    .expect("fingerprint build")
    .expect("active fingerprint");
    (root, fingerprint)
}

#[test]
fn runtime_failure_stdout_answers_for_accepted_and_rejected_requests() {
    let (root, fingerprint) = configured_journal("answer", None);
    let accepted_request = json!({
        "reason_code": "provider_unavailable",
        "component": "generate",
        "expected_fingerprint_sha256": fingerprint,
    });
    let accepted = run(&root, &serde_json::to_vec(&accepted_request).unwrap());
    assert_eq!(accepted.status.code(), Some(0));
    assert_eq!(accepted.stderr, b"");
    let accepted: Value = serde_json::from_slice(&accepted.stdout).expect("JSON result");
    assert_eq!(accepted["accepted"], true);
    assert!(accepted["record"].is_object());
    assert!(accepted["rejected_reason"].is_null());
    assert!(accepted["error"].is_null());

    let rejected_request = json!({
        "reason_code": "provider_unavailable",
        "component": "generate",
        "expected_fingerprint_sha256": "a".repeat(64),
        "diagnostic": {},
    });
    let rejected = run(&root, &serde_json::to_vec(&rejected_request).unwrap());
    assert_eq!(rejected.status.code(), Some(0));
    assert_eq!(rejected.stderr, b"");
    let rejected: Value = serde_json::from_slice(&rejected.stdout).expect("JSON result");
    assert_eq!(rejected["accepted"], false);
    assert!(rejected["record"].is_null());
    assert_eq!(rejected["rejected_reason"], "fingerprint_mismatch");
    assert!(rejected["error"].is_null());
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn runtime_failure_accepts_null_or_omitted_bundled_fingerprint() {
    for (name, bundled_runtime_fingerprint_sha256) in [
        ("bundled-omitted", None),
        ("bundled-null", Some(Value::Null)),
    ] {
        let (root, fingerprint) = configured_journal(name, None);
        let mut request = json!({
            "reason_code": "provider_unavailable",
            "component": "generate",
            "expected_fingerprint_sha256": fingerprint,
        });
        if let Some(bundled_runtime_fingerprint_sha256) = bundled_runtime_fingerprint_sha256 {
            request["bundled_runtime_fingerprint_sha256"] = bundled_runtime_fingerprint_sha256;
        }
        let output = run(&root, &serde_json::to_vec(&request).unwrap());
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stderr, b"");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).expect("JSON result")["accepted"],
            true
        );
        fs::remove_dir_all(root).expect("cleanup root");
    }

    let bundled_runtime_fingerprint_sha256 = "b".repeat(64);
    let (root, fingerprint) = configured_journal(
        "bundled-string",
        Some(Value::String(bundled_runtime_fingerprint_sha256.clone())),
    );
    let string_request = json!({
        "reason_code": "provider_unavailable",
        "component": "generate",
        "expected_fingerprint_sha256": fingerprint,
        "bundled_runtime_fingerprint_sha256": bundled_runtime_fingerprint_sha256,
    });
    let string_output = run(&root, &serde_json::to_vec(&string_request).unwrap());
    assert_eq!(string_output.status.code(), Some(0));
    assert_eq!(string_output.stderr, b"");
    assert_eq!(
        serde_json::from_slice::<Value>(&string_output.stdout).expect("JSON result")["accepted"],
        true
    );
    fs::remove_dir_all(root).expect("cleanup root");

    let root = temp_path("bundled-wrong-type");
    fs::create_dir(&root).expect("create root");
    let wrong_type_request = json!({
        "reason_code": "provider_unavailable",
        "component": "generate",
        "expected_fingerprint_sha256": "a".repeat(64),
        "bundled_runtime_fingerprint_sha256": 7,
    });
    let wrong_type_output = run(&root, &serde_json::to_vec(&wrong_type_request).unwrap());
    assert_eq!(wrong_type_output.status.code(), Some(64));
    assert_eq!(wrong_type_output.stdout, b"");
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn runtime_failure_rejects_malformed_and_oversized_stdin() {
    let root = temp_path("invalid-stdin");
    fs::create_dir(&root).expect("create root");
    for stdin in [b"{not json".as_slice(), b"[]".as_slice()] {
        let output = run(&root, stdin);
        assert_eq!(output.status.code(), Some(64));
        assert_eq!(output.stdout, b"");
    }
    let oversized = vec![b'x'; 1024 * 1024 + 1];
    let output = run(&root, &oversized);
    assert_eq!(output.status.code(), Some(64));
    assert_eq!(output.stdout, b"");
    fs::remove_dir_all(root).expect("cleanup root");
}
