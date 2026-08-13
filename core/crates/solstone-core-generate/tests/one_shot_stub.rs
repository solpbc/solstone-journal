// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use solstone_core_generate::{ContentPart, GenerateRequest, GenerateResponse, OneShotClient};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn stub_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_solstone-generate-one-shot-stub"))
}

fn overrides_path() -> PathBuf {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "solstone-one-shot-stub-overrides-{}-{}-{sequence}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ))
}

fn request() -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: "test.generate".to_owned(),
        contents: vec![ContentPart::Text {
            text: "test".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.0,
        max_output_tokens: 16,
        thinking_budget: None,
        timeout_s: Some(3.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

#[test]
fn one_shot_stub_round_trips_and_records_child_overrides() {
    let path = overrides_path();
    let response = OneShotClient::at_path(stub_path())
        .with_env(
            "SOLSTONE_GENERATE_ONE_SHOT_STUB_OVERRIDES_PATH",
            path.to_string_lossy(),
        )
        .with_env("SOLSTONE_GENERATE_API_KEY_OVERRIDE", "test-key")
        .with_env("SOLSTONE_GENERATE_PROVIDER_OVERRIDE", "openai")
        .with_env("SOLSTONE_GENERATE_MODEL_OVERRIDE", "test-model")
        .execute(&request())
        .expect("stub response");

    let GenerateResponse::Generated(response) = response else {
        panic!("expected generated response");
    };
    assert_eq!(response.text, "one-shot-stub");
    let overrides: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("override record reads"))
            .expect("override record parses");
    assert_eq!(
        overrides,
        json!({"api_key":"test-key","provider":"openai","model":"test-model"})
    );
    fs::remove_file(path).expect("override record removes");
}
