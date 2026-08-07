// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    std::env::temp_dir().join(format!("solstone-core-brain-fingerprint-{name}-{stamp}"))
}

fn key() -> [u8; 32] {
    [7_u8; 32]
}

fn key_hex() -> String {
    key().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn run(root: &Path, request: Value) -> Output {
    let mut child = Command::new(bin())
        .args(["brain", "fingerprint"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("solstone-core should execute");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(&serde_json::to_vec(&request).expect("encode request"))
        .expect("write request");
    child.wait_with_output().expect("wait for solstone-core")
}

fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    fn collect(root: &Path, current: &Path, entries: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
        for entry in fs::read_dir(current).expect("read test directory") {
            let entry = entry.expect("read test entry");
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("test path should be under root")
                .to_path_buf();
            if path.is_dir() {
                entries.push((relative, None));
                collect(root, &path, entries);
            } else {
                entries.push((relative, Some(fs::read(path).expect("read test file"))));
            }
        }
    }

    let mut entries = Vec::new();
    collect(root, root, &mut entries);
    entries.sort();
    entries
}

fn config() -> Map<String, Value> {
    json!({
        "env": {"ANTHROPIC_API_KEY": "sk-test"},
        "providers": {"active": {"provider": "anthropic"}},
    })
    .as_object()
    .expect("config object")
    .clone()
}

#[test]
fn fingerprint_matches_the_library_without_touching_the_working_directory() {
    let root = temp_path("pure");
    fs::create_dir(&root).expect("create test root");
    let before = snapshot_tree(&root);
    let config = config();
    let output = run(&root, json!({"config": config, "hmac_key_hex": key_hex()}));

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    let actual: Value = serde_json::from_slice(&output.stdout).expect("fingerprint JSON");
    let resolution = solstone_core_brain::derive_active_brain_lane(&config);
    let fingerprint = solstone_core_brain::build_active_brain_fingerprint(&config, &key(), None)
        .expect("fingerprint build")
        .expect("fingerprint should be available");
    assert_eq!(
        actual,
        json!({
            "ok": true,
            "fingerprint_sha256": fingerprint,
            "active_lane": resolution.lane,
            "active_provider": resolution.provider,
            "active_model": resolution.model,
            "reason_code": Value::Null,
            "diagnostic": {},
            "bundled_runtime_fingerprint_sha256": Value::Null,
        })
    );
    assert_eq!(snapshot_tree(&root), before);

    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn fingerprint_reports_an_unresolvable_configuration_as_a_normal_result() {
    let root = temp_path("invalid");
    fs::create_dir(&root).expect("create test root");
    let output = run(
        &root,
        json!({
            "config": {"providers": {"active": {"provider": "unknown"}}},
            "hmac_key_hex": key_hex(),
        }),
    );

    assert_eq!(output.status.code(), Some(0));
    let actual: Value = serde_json::from_slice(&output.stdout).expect("fingerprint JSON");
    assert_eq!(actual["ok"], false);
    assert_eq!(actual["reason_code"], "configuration_invalid");
    assert_eq!(
        actual["diagnostic"],
        json!({"field": "providers.active.provider"})
    );

    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn fingerprint_rejects_malformed_or_wrong_length_hmac_keys() {
    let root = temp_path("bad-key");
    fs::create_dir(&root).expect("create test root");
    for hmac_key_hex in ["not-hex".to_owned(), "aa".repeat(31)] {
        let output = run(
            &root,
            json!({"config": config(), "hmac_key_hex": hmac_key_hex}),
        );

        assert_eq!(output.status.code(), Some(64));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("brain fingerprint failed"));
    }

    fs::remove_dir_all(root).expect("cleanup root");
}
