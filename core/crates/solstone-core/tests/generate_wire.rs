// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Local;
use serde_json::{Value, json};
use solstone_core_generate::contract;

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

fn root(name: &str) -> PathBuf {
    let suffix = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "solstone-core-generate-wire-{name}-{stamp}-{suffix}"
    ));
    std::fs::create_dir_all(root.join("health")).expect("create health directory");
    root
}

fn fixture_vector(id: &str) -> &'static Value {
    contract()["conformance_vectors"]
        .as_array()
        .expect("vectors")
        .iter()
        .find(|vector| vector["id"] == id)
        .expect("fixture vector")
}

fn http_response(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn serve(completion: &str) -> (u16, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let port = listener.local_addr().expect("stub address").port();
    let completion = completion.to_owned();
    let handle = thread::spawn(move || {
        let mut requests = 0;
        for _ in 0..6 {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut bytes = [0_u8; 8192];
            let read = stream.read(&mut bytes).expect("read request");
            let request = String::from_utf8_lossy(&bytes[..read]);
            let path = request
                .split_whitespace()
                .nth(1)
                .expect("HTTP request path");
            let body = match path {
                "/health" => r#"{"loaded_model":"served"}"#,
                "/props" => r#"{"n_ctx":16384,"total_slots":1}"#,
                "/tokenize" => r#"{"tokens":[1]}"#,
                "/v1/chat/completions" => &completion,
                other => panic!("unexpected local HTTP path: {other}"),
            };
            stream
                .write_all(http_response(200, body).as_bytes())
                .expect("write response");
            requests += 1;
        }
        requests
    });
    (port, handle)
}

fn serve_endpoint() -> (u16, thread::JoinHandle<usize>) {
    serve_endpoint_with_completion(generated_body().to_owned())
}

fn serve_endpoint_with_completion(completion: String) -> (u16, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint stub server");
    let port = listener.local_addr().expect("endpoint stub address").port();
    let handle = thread::spawn(move || {
        let mut completions = 0;
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept endpoint request");
            let mut bytes = [0_u8; 8192];
            let read = stream.read(&mut bytes).expect("read endpoint request");
            let path = String::from_utf8_lossy(&bytes[..read])
                .split_whitespace()
                .nth(1)
                .expect("endpoint HTTP request path")
                .to_owned();
            let body = match path.as_str() {
                "/v1/models" => r#"{"data":[{"id":"served","max_model_len":4096}]}"#,
                "/v1/chat/completions" => {
                    completions += 1;
                    &completion
                }
                other => panic!("unexpected endpoint HTTP path: {other}"),
            };
            stream
                .write_all(http_response(200, body).as_bytes())
                .expect("write endpoint response");
        }
        completions
    });
    (port, handle)
}

fn write_config(root: &Path, config: Value) {
    std::fs::create_dir_all(root.join("config")).expect("create config directory");
    std::fs::write(root.join("config/journal.json"), config.to_string()).expect("write config");
}

fn bundled_config(confidential: bool) -> Value {
    if confidential {
        json!({"providers": {"active": {"provider": "local"}}, "services": {"confidential": {}}})
    } else {
        json!({"providers": {"active": {"provider": "local"}}})
    }
}

fn run(root: &Path, args: &[&str], input: Option<&Value>) -> Output {
    let input = input.map(Value::to_string);
    run_bytes(root, args, input.as_deref().map(str::as_bytes))
}

fn run_bytes(root: &Path, args: &[&str], input: Option<&[u8]>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(args)
        .env("SOLSTONE_JOURNAL", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start solstone-core generate");
    if let Some(input) = input {
        child
            .stdin
            .as_mut()
            .expect("generate stdin")
            .write_all(input)
            .expect("write generate input");
    }
    child.wait_with_output().expect("wait for generate")
}

fn one_shot(root: &Path, input: &Value) -> Output {
    run(root, &["generate", "--one-shot"], Some(input))
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("one JSON stdout record")
}

fn stderr_protocol_error(output: &Output) -> Value {
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    serde_json::from_slice(&output.stderr).expect("one protocol error stderr record")
}

fn generated_body() -> &'static str {
    r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#
}

fn completion_body(text: &str, finish_reason: &str, usage: Option<Value>) -> String {
    let mut body = json!({
        "choices": [{"message": {"content": text}, "finish_reason": finish_reason}],
    });
    if let Some(usage) = usage {
        body["usage"] = usage;
    }
    body.to_string()
}

fn token_entries(journal: &Path) -> Vec<Value> {
    let path = journal
        .join("tokens")
        .join(format!("{}.jsonl", Local::now().format("%Y%m%d")));
    std::fs::read_to_string(path)
        .expect("usage log")
        .lines()
        .map(|line| serde_json::from_str(line).expect("usage JSON"))
        .collect()
}

#[test]
fn contract_is_the_compiled_fixture_bytes() {
    let journal = root("contract");
    let output = run(&journal, &["generate", "--contract"], None);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert_eq!(
        output.stdout,
        include_bytes!("../../../fixtures/generate_contract.json")
    );
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn bundled_generation_emits_generated_response_and_hints() {
    for (name, attempt_index, expected_hints) in [
        ("plain", 0, None),
        ("attempt", 1, Some(json!(["attempt_index"]))),
    ] {
        let journal = root(name);
        write_config(&journal, bundled_config(false));
        let (port, server) = serve(generated_body());
        std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
        let mut request = fixture_vector("generated")["request"].clone();
        request["attempt_index"] = json!(attempt_index);
        let output = one_shot(&journal, &request);
        assert_eq!(
            output.status.code(),
            Some(fixture_vector("generated")["exit_code"].as_i64().unwrap() as i32)
        );
        assert_eq!(output.stderr, b"");
        let response = stdout_json(&output);
        let expected = &fixture_vector("generated")["response"];
        assert_eq!(response["schema"], expected["schema"]);
        assert_eq!(response["id"], expected["id"]);
        assert_eq!(response["outcome"], expected["outcome"]);
        assert_eq!(response["text"], expected["text"]);
        assert_eq!(response["usage"], expected["usage"]);
        assert_eq!(response["finish_reason"], expected["finish_reason"]);
        assert_eq!(response.get("hints_applied"), expected_hints.as_ref());
        assert_eq!(server.join().expect("join server"), 6);
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn malformed_requests_surface_as_protocol_errors() {
    let journal = root("malformed");
    let mut wrong_schema = fixture_vector("generated")["request"].clone();
    wrong_schema["schema"] = json!("wrong");
    let mut unknown_top_level = fixture_vector("generated")["request"].clone();
    unknown_top_level["unexpected"] = json!(true);
    let mut invalid_content = fixture_vector("generated")["request"].clone();
    invalid_content["contents"] = json!([{"type": "text", "text": "OK", "extra": true}]);
    for request in [&wrong_schema, &unknown_top_level, &invalid_content] {
        let output = one_shot(&journal, request);
        assert_eq!(
            output.status.code(),
            Some(
                contract()["exit_codes"]["malformed_request"]
                    .as_i64()
                    .unwrap() as i32
            )
        );
        let error = stderr_protocol_error(&output);
        assert_eq!(error["schema"], contract()["schema_identifiers"]["error"]);
        assert_eq!(error["reason"], "malformed-request");
        assert!(error["id"].is_null());
    }
    let error = stderr_protocol_error(&one_shot(&journal, &unknown_top_level));
    assert!(error["detail"].as_str().unwrap().contains("unexpected"));
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn invalid_utf8_stdin_is_a_malformed_request() {
    let journal = root("invalid-utf8");
    let output = run_bytes(&journal, &["generate", "--one-shot"], Some(&[0xff]));
    assert_eq!(
        output.status.code(),
        Some(
            contract()["exit_codes"]["malformed_request"]
                .as_i64()
                .unwrap() as i32
        )
    );
    let error = stderr_protocol_error(&output);
    assert_eq!(error["reason"], "malformed-request");
    assert!(error["id"].is_null());
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn lane_refusals_use_fixture_fields_without_network_calls() {
    let cases = [
        ("none", json!({}), "refused-no-engine-configured", true),
        (
            "attestation",
            json!({"providers": {"active": {"provider": "local"}, "local": {"endpoint_url": "https://endpoint", "served_model_id": "served"}}, "services": {"confidential": {}}}),
            "refused-attestation-not-verified",
            true,
        ),
        (
            "unimplemented",
            json!({"providers": {"active": {"provider": "openai"}}}),
            "refused-provider-response-invalid",
            false,
        ),
    ];
    for (name, config, vector_id, exact_reason_code) in cases {
        let journal = root(name);
        if name != "none" {
            write_config(&journal, config);
        }
        let output = one_shot(&journal, &fixture_vector("generated")["request"]);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stderr, b"");
        let response = stdout_json(&output);
        let expected = &fixture_vector(vector_id)["response"];
        for name in ["reason", "detail"] {
            assert_eq!(response[name], expected[name], "{vector_id} {name}");
        }
        if name == "unimplemented" {
            assert_eq!(response["provider"], "openai");
        } else {
            assert_eq!(
                response["provider"], expected["provider"],
                "{vector_id} provider"
            );
        }
        if exact_reason_code {
            for name in ["reason_code", "retryable", "blocking"] {
                assert_eq!(response[name], expected[name], "{vector_id} {name}");
            }
        }
        assert!(!journal.join("tokens").exists());
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn unreachable_byo_endpoint_uses_the_endpoint_unreachable_code() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve endpoint port");
    let port = listener.local_addr().expect("endpoint address").port();
    drop(listener);
    let journal = root("endpoint-unreachable");
    write_config(
        &journal,
        json!({"providers": {"active": {"provider": "local"}, "local": {
            "endpoint_url": format!("http://127.0.0.1:{port}"),
            "served_model_id": "served",
        }}}),
    );
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    let response = stdout_json(&output);
    let expected = &fixture_vector("refused-provider-response-invalid")["response"];
    for name in ["reason", "provider", "detail"] {
        assert_eq!(response[name], expected[name], "{name}");
    }
    assert_eq!(response["reason_code"], "local_endpoint_unreachable");
    assert_eq!(response["retryable"], true);
    assert_eq!(response["blocking"], true);
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn bundled_failure_uses_provider_response_invalid_fixture_details() {
    let journal = root("bundled-failure");
    write_config(&journal, bundled_config(false));
    let (port, server) = serve(r#"{"choices":"bad"}"#);
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    let response = stdout_json(&output);
    let expected = &fixture_vector("refused-provider-response-invalid")["response"];
    for name in ["reason", "provider", "detail"] {
        assert_eq!(response[name], expected[name], "{name}");
    }
    assert_eq!(response["reason_code"], "response_invalid");
    assert_eq!(response["retryable"], false);
    assert_eq!(response["blocking"], true);
    assert_eq!(server.join().expect("join server"), 6);
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn bundled_confidential_is_generated_and_logs_usage_while_refusals_do_not_log() {
    let journal = root("usage");
    write_config(&journal, bundled_config(true));
    let (port, server) = serve(generated_body());
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(server.join().expect("join server"), 6);
    let token_path = journal
        .join("tokens")
        .join(format!("{}.jsonl", Local::now().format("%Y%m%d")));
    let text = std::fs::read_to_string(&token_path).expect("usage log");
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    let value: Value = serde_json::from_str(lines[0]).expect("usage JSON");
    let keys = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from([
            "context".into(),
            "model".into(),
            "timestamp".into(),
            "type".into(),
            "usage".into()
        ])
    );

    let refusal_journal = root("no-usage-refusal");
    let refusal = one_shot(&refusal_journal, &fixture_vector("generated")["request"]);
    assert_eq!(refusal.status.code(), Some(0));
    assert!(!refusal_journal.join("tokens").exists());
    let _ = std::fs::remove_dir_all(journal);
    let _ = std::fs::remove_dir_all(refusal_journal);
}

#[test]
fn malformed_generate_arguments_use_protocol_errors() {
    let journal = root("arguments");
    let minimum = contract()["framing"]["session"]["concurrency"]["minimum"]
        .as_u64()
        .unwrap();
    let below_minimum = minimum.saturating_sub(1).to_string();
    for args in [
        &["generate", "--session"][..],
        &["generate", "--session", "--max-in-flight", &below_minimum][..],
        &["generate", "--session", "--max-in-flight", "-1"][..],
        &["generate", "--session", "--max-in-flight", "not-an-integer"][..],
        &["generate", "--session", "wrong-flag", "3"][..],
        &["generate"][..],
        &["generate", "--bogus"][..],
        &["generate", "--contract", "extra-arg"][..],
    ] {
        let output = run(&journal, args, None);
        assert_eq!(
            output.status.code(),
            Some(
                contract()["exit_codes"]["malformed_request"]
                    .as_i64()
                    .unwrap() as i32
            )
        );
        let error = stderr_protocol_error(&output);
        assert_eq!(error["reason"], "malformed-request");
    }
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn n3_boundary_does_not_emit_incomplete_text_or_schema_validation_failed() {
    let journal = root("truncated-text");
    write_config(&journal, bundled_config(false));
    let body = completion_body(
        "partial answer",
        "length",
        Some(json!({"prompt_tokens": 2, "completion_tokens": 1})),
    );
    let (port, server) = serve(&body);
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    let response = stdout_json(&output);
    assert_eq!(response["outcome"], "generated");
    assert_eq!(response["finish_reason"], "max_tokens");
    assert_ne!(
        response.get("reason").and_then(Value::as_str),
        Some("incomplete-text")
    );
    assert_eq!(server.join().expect("join server"), 6);

    let schema_journal = root("schema-advisory");
    write_config(&schema_journal, bundled_config(false));
    let body = completion_body("{}", "stop", Some(json!({"prompt_tokens": 2})));
    let (port, server) = serve(&body);
    std::fs::write(schema_journal.join("health/local.port"), port.to_string()).expect("write port");
    let mut request = fixture_vector("generated")["request"].clone();
    request["json_schema"] = json!({"type": "object", "required": ["answer"]});
    request["json_output"] = json!(true);
    let response = stdout_json(&one_shot(&schema_journal, &request));
    assert_eq!(response["outcome"], "generated");
    assert_ne!(
        response.get("reason").and_then(Value::as_str),
        Some("schema-validation-failed")
    );
    assert_eq!(server.join().expect("join server"), 6);
    let _ = std::fs::remove_dir_all(journal);
    let _ = std::fs::remove_dir_all(schema_journal);
}

#[test]
fn schema_validation_is_advisory_for_bundled_and_byo_lanes() {
    let schema = json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
    });
    let cases = [
        ("invalid", "{}", false),
        ("valid", r#"{"answer":"OK"}"#, true),
    ];

    for (name, text, expected_valid) in cases {
        let journal = root(&format!("schema-bundled-{name}"));
        write_config(&journal, bundled_config(false));
        let body = completion_body(text, "stop", Some(json!({"prompt_tokens": 2})));
        let (port, server) = serve(&body);
        std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
        let mut request = fixture_vector("generated")["request"].clone();
        request["json_schema"] = schema.clone();
        request["json_output"] = json!(true);
        let response = stdout_json(&one_shot(&journal, &request));
        assert_eq!(response["outcome"], "generated");
        assert_eq!(response["schema_validation"]["valid"], expected_valid);
        if expected_valid {
            assert_eq!(response["schema_validation"]["errors"], json!([]));
        } else {
            assert_eq!(
                response["schema_validation"]["errors"][0]["constraint"],
                "required"
            );
        }
        assert_eq!(server.join().expect("join bundled server"), 6);
        let _ = std::fs::remove_dir_all(journal);
    }

    for (name, text, expected_valid) in cases {
        let journal = root(&format!("schema-endpoint-{name}"));
        let body = completion_body(text, "stop", Some(json!({"prompt_tokens": 2})));
        let (port, server) = serve_endpoint_with_completion(body);
        write_config(
            &journal,
            json!({
                "providers": {"active": {"provider": "local"}, "local": {
                    "endpoint_url": format!("http://127.0.0.1:{port}"),
                    "served_model_id": "served",
                }},
            }),
        );
        let mut request = fixture_vector("generated")["request"].clone();
        request["json_schema"] = schema.clone();
        request["json_output"] = json!(true);
        let response = stdout_json(&one_shot(&journal, &request));
        assert_eq!(response["outcome"], "generated");
        assert_eq!(response["schema_validation"]["valid"], expected_valid);
        if expected_valid {
            assert_eq!(response["schema_validation"]["errors"], json!([]));
        } else {
            assert_eq!(
                response["schema_validation"]["errors"][0]["constraint"],
                "required"
            );
        }
        assert_eq!(server.join().expect("join endpoint server"), 1);
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn completed_provider_results_log_before_strict_validation() {
    let cases = [
        (
            "blank-with-usage",
            "",
            "stop",
            Some(json!({"prompt_tokens": 2, "completion_tokens": 1})),
            true,
            "provider-response-invalid",
        ),
        (
            "blank-without-usage",
            "",
            "stop",
            None,
            false,
            "provider-response-invalid",
        ),
        (
            "json-truncated",
            "partial JSON",
            "length",
            Some(json!({"prompt_tokens": 2, "completion_tokens": 1})),
            true,
            "incomplete-json",
        ),
    ];
    for (name, text, finish_reason, usage, writes_usage, reason) in cases {
        let journal = root(name);
        write_config(&journal, bundled_config(false));
        let body = completion_body(text, finish_reason, usage);
        let (port, server) = serve(&body);
        std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
        let mut request = fixture_vector("generated")["request"].clone();
        if reason == "incomplete-json" {
            request["json_output"] = json!(true);
        }
        let response = stdout_json(&one_shot(&journal, &request));
        assert_eq!(response["outcome"], "refused");
        assert_eq!(response["reason"], reason);
        if reason == "incomplete-json" {
            assert_eq!(response["reason_code"], "incomplete_json_length");
        } else {
            assert_eq!(response["reason_code"], "provider_response_invalid");
        }
        assert_eq!(journal.join("tokens").exists(), writes_usage);
        if writes_usage {
            assert_eq!(token_entries(&journal).len(), 1);
        }
        assert_eq!(server.join().expect("join server"), 6);
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn non_responsive_empty_usage_logs_diagnostics_and_json_truncation_wins() {
    let journal = root("non-responsive-empty-usage");
    write_config(&journal, bundled_config(false));
    let body = completion_body("I cannot complete that request.", "stop", None);
    let (port, server) = serve(&body);
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
    let response = stdout_json(&one_shot(&journal, &fixture_vector("generated")["request"]));
    assert_eq!(response["reason"], "non-responsive-output");
    let entry = token_entries(&journal).pop().expect("one usage entry");
    assert_eq!(entry["usage"], json!({}));
    assert_eq!(entry["non_responsive_matched_signal"], "i cannot");
    assert_eq!(
        entry["non_responsive_output"],
        "I cannot complete that request."
    );
    assert_eq!(server.join().expect("join server"), 6);

    let precedence_journal = root("strict-before-responsiveness");
    write_config(&precedence_journal, bundled_config(false));
    let body = completion_body(
        "I cannot complete that request.",
        "length",
        Some(json!({"prompt_tokens": 2})),
    );
    let (port, server) = serve(&body);
    std::fs::write(
        precedence_journal.join("health/local.port"),
        port.to_string(),
    )
    .expect("write port");
    let mut request = fixture_vector("generated")["request"].clone();
    request["json_output"] = json!(true);
    request["json_schema"] = json!({"type": "object", "required": ["answer"]});
    let response = stdout_json(&one_shot(&precedence_journal, &request));
    assert_eq!(response["reason"], "incomplete-json");
    let entry = token_entries(&precedence_journal)
        .pop()
        .expect("one usage entry");
    assert_eq!(entry["non_responsive_matched_signal"], "i cannot");
    assert_eq!(server.join().expect("join server"), 6);
    let _ = std::fs::remove_dir_all(journal);
    let _ = std::fs::remove_dir_all(precedence_journal);
}

#[test]
#[ignore = "N5: attestation failed and stale vectors require real attestation verification"]
fn n5_only_refusal_vectors_are_deferred() {}

#[test]
fn byo_endpoint_generates_without_confidential_downgrade() {
    let (port, server) = serve_endpoint();
    let endpoint_url = format!("http://127.0.0.1:{port}");
    let config = json!({
        "providers": {"active": {"provider": "local"}, "local": {
            "endpoint_url": endpoint_url,
            "served_model_id": "served",
        }},
    });
    let journal = root("byo-endpoint");
    write_config(&journal, config.clone());
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout_json(&output)["outcome"], "generated");
    assert_eq!(server.join().expect("join endpoint server"), 1);

    let confidential_journal = root("byo-confidential");
    let mut confidential = config;
    confidential["services"] = json!({"confidential": {}});
    write_config(&confidential_journal, confidential);
    let output = one_shot(
        &confidential_journal,
        &fixture_vector("generated")["request"],
    );
    assert_eq!(output.status.code(), Some(0));
    let response = stdout_json(&output);
    assert_eq!(response["outcome"], "refused");
    assert_eq!(response["reason"], "attestation-not-verified");
    let _ = std::fs::remove_dir_all(journal);
    let _ = std::fs::remove_dir_all(confidential_journal);
}

#[test]
fn byo_endpoint_reports_only_honored_exclusive_admission_hint() {
    for exclusive_admission in [false, true] {
        let (port, server) = serve_endpoint();
        let journal = root("endpoint-hints");
        write_config(
            &journal,
            json!({
                "providers": {"active": {"provider": "local"}, "local": {
                    "endpoint_url": format!("http://127.0.0.1:{port}"),
                    "served_model_id": "served",
                }},
            }),
        );
        let mut request = fixture_vector("generated")["request"].clone();
        request["exclusive_admission"] = json!(exclusive_admission);
        let output = one_shot(&journal, &request);
        assert_eq!(output.status.code(), Some(0));
        let response = stdout_json(&output);
        if exclusive_admission {
            assert_eq!(response["hints_applied"], json!(["exclusive_admission"]));
        } else {
            assert!(response.get("hints_applied").is_none());
        }
        assert_eq!(server.join().expect("join endpoint server"), 1);
        let _ = std::fs::remove_dir_all(journal);
    }
}
