// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-isolation and live-endpoint tests for cogitate wire dispatch.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use solstone_core_cogitate_runtime::{
    RecordingEventSink, ToolExecution, ToolExecutor, run_cogitate,
};
use solstone_core_cogitate_wire::{
    COGITATE_API_KEY_OVERRIDE_ENV, COGITATE_ENDPOINT_URL_OVERRIDE_ENV, CogitateRequest,
    DispatchConverseProvider, EndpointOverrides, REQUEST_SCHEMA, serialize_event_validated,
    validate_event,
};
use solstone_core_generate_wire::{ConverseToolCall, ConverseToolSpec, resolve_lane};

const ENV_CHILD: &str = "SOLSTONE_COGITATE_WIRE_ENV_CHILD";

fn request_value() -> Value {
    json!({
        "schema": REQUEST_SCHEMA,
        "access_tier": "normal",
        "outbound_approval": null,
        "diagnostic": false,
        "talent_instruction": "Be concise.",
        "sol_tool_name": "sol",
        "read_scope": [],
        "output_path": null,
        "schedule": "daily",
        "max_turns": 4,
        "cost_cap_usd": 1.5,
        "context_window": 4096,
        "timeout_ms": 30_000,
        "read_call_budget": 5,
        "model": "fixture-model",
        "correlation_id": "corr-1",
        "initial_prompt": "Do the task.",
        "journal_root": "/tmp/solstone-cogitate-wire-test"
    })
}

fn request() -> CogitateRequest {
    CogitateRequest::from_value(&request_value()).expect("fixture request is valid")
}

fn env_child(test: &str, endpoint: Option<&str>, api_key: Option<&str>) -> bool {
    if std::env::var_os(ENV_CHILD).is_some() {
        return false;
    }
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .arg("--exact")
        .arg(test)
        .env(ENV_CHILD, "1")
        .env_remove(COGITATE_ENDPOINT_URL_OVERRIDE_ENV)
        .env_remove(COGITATE_API_KEY_OVERRIDE_ENV);
    if let Some(endpoint) = endpoint {
        command.env(COGITATE_ENDPOINT_URL_OVERRIDE_ENV, endpoint);
    }
    if let Some(api_key) = api_key {
        command.env(COGITATE_API_KEY_OVERRIDE_ENV, api_key);
    }
    assert!(command.status().expect("child status").success());
    true
}

#[test]
fn endpoint_override_environment_reads_present_values() {
    if env_child(
        "endpoint_override_environment_reads_present_values",
        Some(" http://endpoint "),
        Some(" credential "),
    ) {
        return;
    }
    let overrides = EndpointOverrides::from_process();
    assert_eq!(overrides.endpoint_url(), Some("http://endpoint"));
    assert_eq!(overrides.api_key(), Some("credential"));
}

#[test]
fn endpoint_override_environment_reads_absent_values() {
    if env_child(
        "endpoint_override_environment_reads_absent_values",
        None,
        None,
    ) {
        return;
    }
    let overrides = EndpointOverrides::from_process();
    assert_eq!(overrides.endpoint_url(), None);
    assert_eq!(overrides.api_key(), None);
}

struct FinalToolExecutor;

impl ToolExecutor for FinalToolExecutor {
    fn offered_tools(
        &self,
        _config: &solstone_core_cogitate_runtime::RunConfig,
    ) -> Result<Vec<ConverseToolSpec>, String> {
        Ok(vec![ConverseToolSpec {
            name: "emit_final".into(),
            description: "finish".into(),
            parameters: json!({"type": "object"}),
        }])
    }

    fn execute(
        &mut self,
        _config: &solstone_core_cogitate_runtime::RunConfig,
        _call: &ConverseToolCall,
    ) -> ToolExecution {
        panic!("emit_final is terminal and must not be executed")
    }
}

fn response(status: u16, body: Value) -> String {
    format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        if (200..300).contains(&status) {
            "OK"
        } else {
            "Error"
        },
        body.to_string().len(),
        body
    )
}

fn spawn_stub(
    status: u16,
    body: Value,
) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("stub bind");
    let address = listener.local_addr().expect("stub address");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let response = response(status, body);
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("stub accept");
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let read = stream.read(&mut buffer).expect("stub read");
            assert!(read > 0, "request closed before headers");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let header = std::str::from_utf8(&bytes[..header_end]).expect("headers are UTF-8");
        let content_length = header
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .expect("request content length");
        while bytes.len() < header_end + content_length {
            let read = stream.read(&mut buffer).expect("stub body read");
            assert!(read > 0, "request closed before body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        captured
            .lock()
            .expect("stub lock")
            .push(String::from_utf8(bytes).expect("request UTF-8"));
        stream.write_all(response.as_bytes()).expect("stub write");
    });
    (format!("http://{address}"), requests, handle)
}

fn final_turn_response() -> Value {
    json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": {"name": "emit_final", "arguments": "{\"content\":\"done\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
    })
}

fn run_endpoint_request(status: u16, body: Value, secret: &str) -> (Vec<Value>, String) {
    let (url, requests, handle) = spawn_stub(status, body);
    let request = request();
    let config = json!({
        "providers": {
            "active": {"provider": "local"},
            "local": {"endpoint_url": "http://configured", "served_model_id": "configured"}
        }
    })
    .as_object()
    .expect("config is an object")
    .clone();
    let (_, lane) = resolve_lane(&config);
    let mut provider = DispatchConverseProvider::from_lane(
        &request,
        config,
        lane,
        EndpointOverrides::from_values(Some(url), Some(secret.to_owned())),
    )
    .expect("endpoint lane provider");
    let mut tools = FinalToolExecutor;
    let mut sink = RecordingEventSink::default();
    run_cogitate(&mut provider, &mut tools, request.to_run_input(), &mut sink);
    handle.join().expect("stub joins");
    let request_text = requests.lock().expect("request lock")[0].clone();
    (
        sink.events
            .into_iter()
            .map(|event| serialize_event_validated(event).expect("event validates"))
            .collect(),
        request_text,
    )
}

fn has_bearer(request: &str, credential: &str) -> bool {
    request.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.eq_ignore_ascii_case("authorization") && value.trim() == format!("Bearer {credential}")
    })
}

#[test]
fn endpoint_provider_threads_bearer_and_never_serializes_credential() {
    let secret = "wire-secret-credential";
    let (events, request_text) = run_endpoint_request(200, final_turn_response(), secret);
    assert!(has_bearer(&request_text, secret));
    let stream = events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!stream.contains(secret));
    let malformed =
        CogitateRequest::from_value(&json!({"credential": secret})).expect_err("bad request");
    assert!(!format!("{malformed}").contains(secret));
    let validation = validate_event(&json!({
        "event": "text_delta",
        "ts": 1,
        "correlation_id": "corr-1",
        "delta": "chunk",
        "model": "model",
        "credential": secret
    }))
    .expect_err("undeclared credential is rejected");
    assert!(!format!("{validation}").contains(secret));
    assert!(!format!("{validation:?}").contains(secret));
}

#[test]
fn credential_redaction_holds_for_forced_provider_failure() {
    let secret = "wire-secret-credential";
    let (events, request_text) = run_endpoint_request(500, json!({"detail": secret}), secret);
    assert!(has_bearer(&request_text, secret));
    let stream = events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!stream.contains(secret));
    // Mutation tripwire, verified 2026-08-10 against generate-wire
    // endpoint.rs:389-390: replacing its normalized failure with one carrying
    // the raw response body makes this assertion fail, so the test detects a
    // real redaction regression rather than merely observing a happy path.
    assert!(stream.contains("provider_response_invalid"));
}
