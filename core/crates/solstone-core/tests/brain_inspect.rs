// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, Utc};
use serde_json::{Map, Value, json};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    std::env::temp_dir().join(format!("solstone-core-brain-inspect-{name}-{stamp}"))
}

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("test path has parent")).expect("create parent");
    fs::write(path, contents).expect("write test file");
}

fn config() -> Value {
    json!({
        "env": {"ANTHROPIC_API_KEY": "sk-test"},
        "providers": {"active": {"provider": "anthropic"}},
    })
}

fn configured_journal(name: &str, with_key: bool) -> PathBuf {
    let root = temp_path(name);
    fs::create_dir(&root).expect("create root");
    write(
        &root,
        "config/journal.json",
        &serde_json::to_vec(&config()).expect("encode config"),
    );
    if with_key {
        write(&root, "health/brain-fingerprint.key", &[7_u8; 32]);
    }
    root
}

fn run(root: &Path) -> Output {
    Command::new(bin())
        .args(["brain", "inspect", "--journal"])
        .arg(root)
        .output()
        .expect("solstone-core should execute")
}

fn output(root: &Path) -> Value {
    let output = run(root);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, b"");
    serde_json::from_slice(&output.stdout).expect("inspect output should be JSON")
}

fn ready_outcome() -> Value {
    let now = Utc::now();
    let observed_at = now.to_rfc3339();
    let expires_at = (now + Duration::hours(1)).to_rfc3339();
    let component = || {
        json!({
            "status": "ok",
            "observed_at": observed_at,
            "expires_at": expires_at,
        })
    };
    json!({
        "configuration": component(),
        "lane_prerequisites": component(),
        "generate": component(),
        "cogitate": component(),
    })
}

fn projection_json(projection: solstone_core_brain::BrainProjection) -> Value {
    json!({
        "aggregate_state": projection.aggregate_state,
        "reason_code": projection.reason_code,
        "active_lane": projection.active_lane,
        "active_provider": projection.active_provider,
        "active_model": projection.active_model,
        "fingerprint_sha256": projection.fingerprint_sha256,
        "runtime_transition_in_progress": projection.runtime_transition_in_progress,
    })
}

fn direct_inspection(root: &Path) -> solstone_core_brain::BrainInspection {
    let config = solstone_core_brain::read_journal_config(root)
        .expect("read config")
        .config
        .unwrap_or_default();
    solstone_core_brain::inspect_brain_state(root, &config, Utc::now())
}

fn expected_active_fingerprint(root: &Path) -> Value {
    let config = solstone_core_brain::read_journal_config(root)
        .expect("read config")
        .config
        .unwrap_or_default();
    let resolution = solstone_core_brain::derive_active_brain_lane(&config);
    let key = solstone_core_brain::load_existing_fingerprint_key(root);
    let fingerprint = key.and_then(|key| {
        solstone_core_brain::build_active_brain_fingerprint(&config, &key, None)
            .expect("fingerprint build")
    });
    json!({
        "ok": fingerprint.is_some(),
        "fingerprint_sha256": fingerprint,
        "active_lane": resolution.lane,
        "active_provider": resolution.provider,
        "active_model": resolution.model,
        "reason_code": if key.is_some() { Value::Null } else { Value::String("fingerprint_key_unavailable".to_owned()) },
        "diagnostic": Map::<String, Value>::new(),
        "bundled_runtime_fingerprint_sha256": Value::Null,
    })
}

#[test]
fn inspect_matches_the_library_for_a_valid_ready_record() {
    let root = configured_journal("ready", true);
    let permit = solstone_core_brain::begin_refresh(&root, Utc::now(), None, None, false, None)
        .expect("begin refresh")
        .expect("refresh permit");
    solstone_core_brain::finish_refresh(&root, permit, ready_outcome(), Utc::now(), None)
        .expect("finish refresh");

    let expected = direct_inspection(&root);
    let output = output(&root);
    assert_eq!(output["status"], "ok");
    assert_eq!(output["record"], expected.record.expect("valid record"));
    assert_eq!(output["projection"], projection_json(expected.projection));
    assert_eq!(
        output["active_fingerprint"],
        expected_active_fingerprint(&root)
    );

    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn inspect_matches_the_library_when_the_record_is_missing() {
    let root = configured_journal("missing-record", true);

    let expected = direct_inspection(&root);
    let output = output(&root);
    assert_eq!(output["status"], "unavailable");
    assert!(output["record"].is_null());
    assert_eq!(output["projection"], projection_json(expected.projection));
    assert_eq!(
        output["active_fingerprint"],
        expected_active_fingerprint(&root)
    );

    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn inspect_reports_an_unavailable_active_fingerprint_without_a_key() {
    let root = configured_journal("missing-key", false);

    let output = output(&root);
    assert!(output["record"].is_null());
    assert_eq!(
        output["active_fingerprint"],
        expected_active_fingerprint(&root)
    );
    assert_eq!(output["active_fingerprint"]["ok"], false);
    assert_eq!(
        output["active_fingerprint"]["reason_code"],
        "fingerprint_key_unavailable"
    );

    fs::remove_dir_all(root).expect("cleanup root");
}
