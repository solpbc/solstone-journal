// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Loopback proof that a non-context HTTP 400 becomes a capped daily unit.
//!
//! This target is outside the routine library harness because it binds a TCP
//! listener. The reviewed assertions are unchanged.

use std::io::{Read, Write};
use std::net::TcpListener;

use serde_json::{Value, json};
use solstone_core_journal_io::{DailyUnitIdentity, DailyUnitStatus, load_daily_unit_record};

#[test]
fn cross_boundary_loopback_400_to_capped_daily_unit_with_detail() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sentinels = [
        "SENTINEL_CRED_9f3a",
        "SENTINEL_PROVIDER_e1d4",
        "SENTINEL_OWNER_b7c2",
    ];
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request_buf = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let Ok(read) = stream.read(&mut buffer) else {
                break;
            };
            if read == 0 {
                break;
            }
            request_buf.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = request_buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&request_buf[..header_end]);
                let content_length = header
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .unwrap_or_default();
                if request_buf.len() >= header_end + 4 + content_length {
                    break;
                }
            }
        }
        let body = r#"{"type":"BadRequestError","code":400,"message":"Failed to compile json grammar: SENTINEL_CRED_9f3a SENTINEL_PROVIDER_e1d4 SENTINEL_OWNER_b7c2"}"#;
        let response = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
    });

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let day = "20260914";
    let identity = DailyUnitIdentity::new(day, "schedule", None);

    solstone_core_think_cli::test_support::reserve_daily_attempt(
        root,
        &identity,
        "E",
        "C",
        &json!({"packet":"original"}),
        day,
        "use-loopback-1",
        false,
        false,
        1,
    )
    .unwrap();

    let endpoint = solstone_core_local::ByoEndpoint {
        base_url: format!("http://127.0.0.1:{port}"),
        served_model_id: "test-model".into(),
        credential: None,
        parallel_slots: Some(1),
        is_confidential: false,
        is_bundled: false,
    };
    let request = solstone_core_generate::GenerateRequest {
        id: None,
        context: "schedule".into(),
        contents: vec![solstone_core_generate::ContentPart::Text {
            text: "prompt".into(),
        }],
        system_instruction: None,
        temperature: 0.2,
        max_output_tokens: 64,
        thinking_budget: None,
        timeout_s: Some(5.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: false,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    };
    let config = json!({"providers": {"local": {"served_context_window": 4096}}})
        .as_object()
        .unwrap()
        .clone();
    let runtime = solstone_core_generate_wire::EndpointRuntime::default();
    let result = solstone_core_generate_wire::endpoint_generate(
        &request, root, &endpoint, &config, &runtime,
    );
    let solstone_core_generate_wire::EndpointResult::Failed(failure) = result else {
        panic!("expected failure from 400 endpoint");
    };
    let refusal = solstone_core_generate_wire::refusal_for(
        &solstone_core_generate_wire::LaneOutcome::EndpointFailure(failure.clone()),
        "local",
        None,
    );
    let wire_reason_code = refusal
        .reason_code
        .as_ref()
        .map(solstone_core_generate::ReasonCodeValue::as_wire)
        .unwrap();

    let refusal_response = solstone_core_generate::RefusedResponse {
        id: None,
        reason: solstone_core_generate::RefusalReason::ProviderResponseInvalid,
        reason_code: refusal.reason_code.clone(),
        retryable: refusal.retryable,
        blocking: refusal.blocking,
        reset_at_ms: None,
        provider: Some("local".to_owned()),
        detail: refusal.detail.clone(),
    };

    let use_dir = root.join("talents/schedule");
    std::fs::create_dir_all(&use_dir).unwrap();
    let start_line = "{\"event\":\"start\",\"use_id\":\"use-loopback-1\"}\n";
    let mut worker_log_bytes = start_line.as_bytes().to_vec();
    let error = solstone_core_talent_runtime::StageError::new(
        "generate",
        "runtime",
        "schedule".to_owned(),
        refusal.detail.clone(),
    );
    solstone_core_talent_runtime::emit_outcome_for_test(
        &mut worker_log_bytes,
        solstone_core_talent_runtime::RuntimeOutcome::GenerateRefused {
            error,
            response: Box::new(refusal_response),
        },
    );
    std::fs::write(use_dir.join("use-loopback-1.jsonl"), worker_log_bytes).unwrap();

    solstone_core_think_cli::test_support::log_daily_terminal(
        root,
        day,
        "schedule",
        "use-loopback-1",
        wire_reason_code,
        1_000_000,
    )
    .unwrap();

    let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
    assert_eq!(loaded.status, DailyUnitStatus::Capped);
    assert_eq!(loaded.reason_code.as_deref(), Some(wire_reason_code));
    assert_eq!(loaded.attempt_day.as_deref(), Some("20260914"));
    assert_eq!(loaded.environmental_retry_day.as_deref(), Some("20260914"));

    let health_dir = root.join("chronicle").join(day).join("health");
    let mut found = None;
    for entry in std::fs::read_dir(&health_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            let row: Value = serde_json::from_str(line).unwrap();
            if row.get("event") == Some(&Value::String("talent.fail".to_owned()))
                && row.get("name") == Some(&Value::String("schedule".to_owned()))
            {
                found = Some((row, line.to_owned()));
            }
        }
    }
    let (row, row_line) = found.expect("expected talent.fail event");
    assert_eq!(row["detail"]["detail"].as_str(), failure.detail.as_deref());

    for sentinel in &sentinels {
        assert!(!failure.detail.as_deref().unwrap_or("").contains(sentinel));
        assert!(!refusal.detail.contains(sentinel));
        assert!(!row_line.contains(sentinel));
    }

    server.join().unwrap();
}
