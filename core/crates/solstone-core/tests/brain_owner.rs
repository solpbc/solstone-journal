// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");
const SENTINEL: &str = "\x1fsolstone-journal-brain-owner-v1";

fn write_bundled_config(journal: &TempDir) {
    fs::create_dir_all(journal.path().join("config")).expect("config dir");
    fs::write(
        journal.path().join("config/journal.json"),
        br#"{"providers":{"active":{"provider":"local"}}}"#,
    )
    .expect("bundled config");
}

fn owner_refresh(journal: &TempDir) {
    let output = Command::new(BINARY)
        .args([SENTINEL, "brain", "refresh", "--json"])
        .env("SOLSTONE_JOURNAL", journal.path())
        .output()
        .expect("run owner refresh");
    assert!(
        matches!(output.status.code(), Some(0..=2)),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn bundled_runtime_fingerprint(journal: &TempDir) -> String {
    let model_id = solstone_core_local::install::resolve_bundled_model_id(
        "local/qwen3.5-4b",
        cfg!(target_os = "macos"),
    );
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut input = serde_json::Map::from_iter([
        (
            "journal".into(),
            json!(journal.path().display().to_string()),
        ),
        ("model_id".into(), json!(model_id)),
    ]);
    #[cfg(target_os = "macos")]
    input.insert("backend".into(), json!("metal"));
    #[cfg(target_os = "macos")]
    let readiness =
        solstone_core_local::install::metal_candidate::inspect(&input).expect("metal inspect");
    #[cfg(not(target_os = "macos"))]
    let readiness = solstone_core_local::install::readiness::inspect_local(input);
    solstone_core_brain::bundled_runtime_desired_fingerprint(
        readiness["host"]["backend"].as_str().unwrap_or("metal"),
        &model_id,
        readiness["target"]["target_fingerprint_sha256"]
            .as_str()
            .unwrap_or(""),
        readiness["artifacts"]["binary_path"].as_str(),
        readiness["artifacts"]["model_path"]
            .as_str()
            .expect("model_path"),
        readiness["artifacts"]["projector_path"].as_str(),
    )
    .expect("unified desired fingerprint")
    .sha256
}

#[test]
fn owner_status_json_has_the_python_sorted_key_shape() {
    let journal = TempDir::new_in("/var/tmp").expect("journal");
    fs::create_dir_all(journal.path().join("config")).expect("config dir");
    fs::write(journal.path().join("config/journal.json"), b"{}").expect("config");
    let output = Command::new(BINARY)
        .args([SENTINEL, "brain", "status", "--json"])
        .env("SOLSTONE_JOURNAL", journal.path())
        .output()
        .expect("run owner status");
    assert_eq!(output.status.code(), Some(2));
    let line = std::str::from_utf8(&output.stdout).expect("utf8");
    let value: Value = serde_json::from_str(line).expect("owner JSON");
    assert_eq!(
        value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "aggregate_state",
            "expires_at",
            "failing_component",
            "fingerprint_sha256",
            "lane",
            "model",
            "observed_at",
            "path",
            "provider",
            "reason_code"
        ]
    );
}

#[test]
fn owner_refresh_creates_and_reuses_the_fingerprint_key() {
    let fresh = TempDir::new_in("/var/tmp").expect("fresh journal");
    write_bundled_config(&fresh);
    let fresh_key = solstone_core_brain::brain_fingerprint_key_path(fresh.path());
    assert!(!fresh_key.exists());
    owner_refresh(&fresh);
    let generated = fs::read(&fresh_key).expect("owner refresh creates key");
    assert_eq!(generated.len(), 32);
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::MetadataExt::mode(&fs::metadata(&fresh_key).expect("key metadata"))
            & 0o777,
        0o600
    );

    let prepopulated = TempDir::new_in("/var/tmp").expect("prepopulated journal");
    write_bundled_config(&prepopulated);
    let prepopulated_key = solstone_core_brain::brain_fingerprint_key_path(prepopulated.path());
    fs::create_dir_all(prepopulated_key.parent().expect("key parent")).expect("key parent dir");
    let distinctive = [0xa5_u8; 32];
    fs::write(&prepopulated_key, distinctive).expect("prepopulate key");
    owner_refresh(&prepopulated);
    assert_eq!(
        fs::read(&prepopulated_key).expect("read reused key"),
        distinctive,
        "owner refresh must preserve the existing fingerprint key"
    );
}

#[test]
fn owner_refresh_default_fence_compares_the_bundled_runtime_fingerprint() {
    let journal = TempDir::new_in("/var/tmp").expect("journal");
    write_bundled_config(&journal);
    let expected = bundled_runtime_fingerprint(&journal);
    let matching = Command::new(BINARY)
        .args([
            SENTINEL,
            "brain",
            "refresh",
            "--json",
            "--expected-fingerprint",
            &expected,
        ])
        .env("SOLSTONE_JOURNAL", journal.path())
        .output()
        .expect("run matching bundled fence");
    assert_ne!(
        matching.status.code(),
        Some(3),
        "matching runtime fence was treated as stale: stdout={} stderr={}",
        String::from_utf8_lossy(&matching.stdout),
        String::from_utf8_lossy(&matching.stderr)
    );

    let state_path = solstone_core_brain::brain_state_path(journal.path());
    let before = fs::read(&state_path).expect("matching refresh writes state");
    let stale = Command::new(BINARY)
        .args([
            SENTINEL,
            "brain",
            "refresh",
            "--json",
            "--expected-fingerprint",
            &"b".repeat(64),
        ])
        .env("SOLSTONE_JOURNAL", journal.path())
        .output()
        .expect("run stale bundled fence");
    assert_eq!(stale.status.code(), Some(3));
    assert_eq!(
        fs::read(state_path).expect("read state after stale fence"),
        before,
        "stale bundled fence must not mutate durable state"
    );
}
