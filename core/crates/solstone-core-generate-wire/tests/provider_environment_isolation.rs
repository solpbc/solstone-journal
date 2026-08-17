// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real process-environment isolation for generate-wire overrides.
//!
//! Each case re-execs this binary with a cleared environment so ambient host
//! credentials cannot satisfy the assertion. The child prints one receipt line
//! after the assertion; the parent treats a missing, duplicate, or wrong-case
//! receipt as failure, including the rustc harness's "0 tests" success path.

use std::process::Command;

use serde_json::{Map, Value, json};
use solstone_core_generate_wire::overrides::{
    API_KEY_OVERRIDE_ENV, MODEL_OVERRIDE_ENV, PROVIDER_OVERRIDE_ENV, configured_api_key,
    configured_model, configured_provider,
};

const MARKER: &str = "SOLSTONE_GW_ISOLATION_CHILD";
const RECEIPT_PREFIX: &str = "SOLSTONE_GW_RECEIPT ";

fn config(provider: Option<&str>, model: Option<&str>, key: Option<&str>) -> Map<String, Value> {
    let mut active = Map::new();
    if let Some(provider) = provider {
        active.insert("provider".into(), json!(provider));
    }
    if let Some(model) = model {
        active.insert("model".into(), json!(model));
    }
    let mut env = Map::new();
    if let Some(key) = key {
        env.insert("OPENAI_API_KEY".into(), json!(key));
    }
    Map::from_iter([
        (
            "providers".into(),
            Value::Object(Map::from_iter([("active".into(), Value::Object(active))])),
        ),
        ("env".into(), Value::Object(env)),
    ])
}

fn receipt(case: &str, provider: Option<&str>, model: Option<&str>, key: Option<&str>) -> Value {
    json!({
        "v": 1,
        "case": case,
        "provider": provider,
        "model": model,
        "key": key,
        "network": "none",
    })
}

fn emit_receipt(value: &Value) {
    println!("{RECEIPT_PREFIX}{value}");
}

fn enter(case: &str, environment: &[(&str, &str)], expected: &Value) -> bool {
    match std::env::var(MARKER) {
        Ok(seen) if seen == case => true,
        Ok(seen) => panic!("stale isolation marker {seen:?} for case {case}"),
        Err(_) => {
            let mut command = Command::new(std::env::current_exe().expect("current test exe"));
            command
                .arg("--exact")
                .arg(case)
                .arg("--nocapture")
                .env_clear()
                .env("PATH", "")
                .env(MARKER, case);
            for (key, value) in environment {
                command.env(key, value);
            }
            let output = command.output().expect("spawn isolation child");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout}{stderr}");
            assert!(
                output.status.success(),
                "isolation child {case} failed: {combined}"
            );
            assert!(
                !combined.contains("running 0 tests"),
                "isolation child {case} ran no tests: {combined}"
            );
            let receipts: Vec<&str> = stdout
                .lines()
                .filter(|line| line.starts_with(RECEIPT_PREFIX))
                .collect();
            assert_eq!(
                receipts.len(),
                1,
                "isolation child {case} must print exactly one receipt: {combined}"
            );
            let observed: Value = serde_json::from_str(&receipts[0][RECEIPT_PREFIX.len()..])
                .expect("isolation receipt is JSON");
            assert_eq!(observed, *expected, "isolation receipt mismatch for {case}");
            false
        }
    }
}

#[test]
fn api_override_without_config_wins() {
    let case = "api_override_without_config_wins";
    let expected = receipt(case, None, None, Some("override"));
    if !enter(case, &[(API_KEY_OVERRIDE_ENV, "override")], &expected) {
        return;
    }
    assert_eq!(
        configured_api_key(&config(None, None, None), "OPENAI_API_KEY").as_deref(),
        Some("override")
    );
    emit_receipt(&expected);
}

#[test]
fn api_override_beats_config() {
    let case = "api_override_beats_config";
    let expected = receipt(case, None, None, Some("override"));
    if !enter(case, &[(API_KEY_OVERRIDE_ENV, "override")], &expected) {
        return;
    }
    assert_eq!(
        configured_api_key(&config(None, None, Some("stored")), "OPENAI_API_KEY").as_deref(),
        Some("override")
    );
    emit_receipt(&expected);
}

#[test]
fn api_config_ignores_conventional_process_env() {
    let case = "api_config_ignores_conventional_process_env";
    let expected = receipt(case, None, None, Some("stored"));
    if !enter(case, &[("OPENAI_API_KEY", "ambient")], &expected) {
        return;
    }
    assert_eq!(
        configured_api_key(&config(None, None, Some("stored")), "OPENAI_API_KEY").as_deref(),
        Some("stored")
    );
    emit_receipt(&expected);
}

#[test]
fn provider_override_without_config_wins() {
    let case = "provider_override_without_config_wins";
    let expected = receipt(case, Some("google"), None, None);
    if !enter(case, &[(PROVIDER_OVERRIDE_ENV, "google")], &expected) {
        return;
    }
    assert_eq!(configured_provider(&config(None, None, None)), "google");
    emit_receipt(&expected);
}

#[test]
fn provider_override_beats_config() {
    let case = "provider_override_beats_config";
    let expected = receipt(case, Some("google"), None, None);
    if !enter(case, &[(PROVIDER_OVERRIDE_ENV, "google")], &expected) {
        return;
    }
    assert_eq!(
        configured_provider(&config(Some("openai"), None, None)),
        "google"
    );
    emit_receipt(&expected);
}

#[test]
fn provider_config_is_used_without_override() {
    let case = "provider_config_is_used_without_override";
    let expected = receipt(case, Some("openai"), None, None);
    if !enter(case, &[], &expected) {
        return;
    }
    assert_eq!(
        configured_provider(&config(Some("openai"), None, None)),
        "openai"
    );
    emit_receipt(&expected);
}

#[test]
fn model_override_without_config_wins() {
    let case = "model_override_without_config_wins";
    let expected = receipt(case, None, Some("candidate"), None);
    if !enter(case, &[(MODEL_OVERRIDE_ENV, "candidate")], &expected) {
        return;
    }
    assert_eq!(
        configured_model(&config(None, None, None), "default"),
        "candidate"
    );
    emit_receipt(&expected);
}

#[test]
fn model_override_beats_config() {
    let case = "model_override_beats_config";
    let expected = receipt(case, None, Some("candidate"), None);
    if !enter(case, &[(MODEL_OVERRIDE_ENV, "candidate")], &expected) {
        return;
    }
    assert_eq!(
        configured_model(&config(None, Some("stored"), None), "default"),
        "candidate"
    );
    emit_receipt(&expected);
}

#[test]
fn model_config_is_used_without_override() {
    let case = "model_config_is_used_without_override";
    let expected = receipt(case, None, Some("stored"), None);
    if !enter(case, &[], &expected) {
        return;
    }
    assert_eq!(
        configured_model(&config(None, Some("stored"), None), "default"),
        "stored"
    );
    emit_receipt(&expected);
}

#[test]
fn anthropic_conventional_key_ignored() {
    let case = "anthropic_conventional_key_ignored";
    let expected = receipt(case, None, None, None);
    if !enter(
        case,
        &[("ANTHROPIC_API_KEY", "process-only-secret")],
        &expected,
    ) {
        return;
    }
    assert_eq!(
        configured_api_key(&config(None, None, None), "ANTHROPIC_API_KEY").as_deref(),
        None
    );
    emit_receipt(&expected);
}

#[test]
fn google_conventional_key_ignored() {
    let case = "google_conventional_key_ignored";
    let expected = receipt(case, None, None, None);
    if !enter(
        case,
        &[("GOOGLE_API_KEY", "process-only-secret")],
        &expected,
    ) {
        return;
    }
    assert_eq!(
        configured_api_key(&config(None, None, None), "GOOGLE_API_KEY").as_deref(),
        None
    );
    emit_receipt(&expected);
}

#[test]
fn openai_conventional_key_ignored() {
    let case = "openai_conventional_key_ignored";
    let expected = receipt(case, None, None, None);
    if !enter(
        case,
        &[("OPENAI_API_KEY", "process-only-secret")],
        &expected,
    ) {
        return;
    }
    assert_eq!(
        configured_api_key(&config(None, None, None), "OPENAI_API_KEY").as_deref(),
        None
    );
    emit_receipt(&expected);
}
