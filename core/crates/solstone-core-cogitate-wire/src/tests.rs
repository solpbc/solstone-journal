// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use solstone_core_cogitate_runtime::events::{BudgetLadder, BudgetStage};
use solstone_core_cogitate_runtime::{
    ConverseProvider, RecordingEventSink, RunOutcome, RuntimeEvent, ToolExecution, ToolExecutor,
    Usage, run_cogitate,
};
use solstone_core_generate_wire::{
    ConverseFailure, ConverseMessage, ConverseToolCall, ConverseToolSpec,
};

use crate::{
    COGITATE_API_KEY_OVERRIDE_ENV, COGITATE_ENDPOINT_URL_OVERRIDE_ENV, CogitateRequest,
    EndpointConfigurationError, EndpointConverseProvider, EndpointOverrides, NativeRun,
    REQUEST_SCHEMA, run_or_dry_run, serialize_dry_run, serialize_event, serialize_event_validated,
    validate_event,
};

fn request_value() -> Value {
    json!({
        "schema": REQUEST_SCHEMA,
        "access_tier": "normal",
        "outbound_approval": null,
        "expects_emit_final": true,
        "max_turns": 4,
        "cost_cap_usd": 1.5,
        "context_window": 4096,
        "timeout_ms": 30_000,
        "read_call_budget": 5,
        "model": "fixture-model",
        "correlation_id": "corr-1",
        "initial_prompt": "Do the task.",
        "system_instruction": "Be concise.",
        "journal_root": "/tmp/solstone-cogitate-wire-test"
    })
}

fn request() -> CogitateRequest {
    CogitateRequest::from_value(&request_value()).expect("fixture request is valid")
}

fn clean_outcome() -> RunOutcome {
    RunOutcome::clean(
        Some("done".to_owned()),
        Usage::default(),
        "corr-1".to_owned(),
    )
}

#[test]
fn valid_request_round_trips_to_runtime_input() {
    let request = request();
    let input = request.to_run_input();
    assert_eq!(input.config.access_tier, "normal");
    assert_eq!(input.config.max_turns, 4);
    assert_eq!(input.config.timeout.as_millis(), 30_000);
    assert_eq!(input.initial_prompt, "Do the task.");
    assert_eq!(input.system_instruction.as_deref(), Some("Be concise."));
    assert_eq!(
        input.journal_root,
        std::path::PathBuf::from("/tmp/solstone-cogitate-wire-test")
    );
    assert!(!request.dry_run);
}

#[test]
fn malformed_requests_are_rejected_with_specific_messages() {
    let cases = [
        ("unknown", json!({"unexpected": true}), "unknown field"),
        (
            "wrong type",
            json!({"max_turns": "four"}),
            "max_turns must be a positive integer",
        ),
        (
            "non-positive",
            json!({"timeout_ms": 0}),
            "timeout_ms must be a positive integer",
        ),
        (
            "tier",
            json!({"access_tier": "invented"}),
            "invalid access_tier",
        ),
        (
            "journal",
            json!({"journal_root": "relative"}),
            "journal_root must be an absolute path",
        ),
    ];
    for (name, replacement, expected) in cases {
        let mut value = request_value();
        let object = value.as_object_mut().expect("request is object");
        for (key, value) in replacement.as_object().expect("replacement is object") {
            object.insert(key.clone(), value.clone());
        }
        let error = CogitateRequest::from_value(&value).expect_err(name);
        assert!(error.to_string().contains(expected), "{name}: {error}");
    }
}

#[test]
fn request_rejects_provider_endpoint_and_credential_fields() {
    for field in ["provider", "endpoint_url", "credential"] {
        let mut value = request_value();
        value[field] = json!("forbidden");
        let error = CogitateRequest::from_value(&value).expect_err(field);
        assert_eq!(
            error.to_string(),
            format!("malformed request: unknown field {field:?}")
        );
    }
}

const ENV_CHILD: &str = "SOLSTONE_COGITATE_WIRE_ENV_CHILD";

fn env_child(test: &str, endpoint: Option<&str>, api_key: Option<&str>) -> bool {
    if std::env::var_os(ENV_CHILD).is_some() {
        return false;
    }
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .arg("--exact")
        .arg(format!("tests::{test}"))
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

#[test]
fn every_runtime_variant_maps_to_its_wire_kind_and_common_fields() {
    let events = vec![
        (
            RuntimeEvent::TextDelta {
                delta: "chunk".into(),
                model: "model".into(),
                correlation_id: "corr-1".into(),
            },
            "text_delta",
        ),
        (
            RuntimeEvent::Reasoning {
                summary: "summary".into(),
                payload: Some(json!({"detail": true})),
                model: "model".into(),
                correlation_id: "corr-1".into(),
            },
            "thinking",
        ),
        (
            RuntimeEvent::ToolStart {
                call_id: "call-1".into(),
                tool: "sol".into(),
                arguments: json!({"command": "sol status"}),
                correlation_id: "corr-1".into(),
            },
            "tool_start",
        ),
        (
            RuntimeEvent::ToolEnd {
                call_id: "call-1".into(),
                tool: "sol".into(),
                arguments: json!({"command": "sol status"}),
                result: "ok".into(),
                is_error: false,
                correlation_id: "corr-1".into(),
            },
            "tool_end",
        ),
        (
            RuntimeEvent::SolBudgetExhausted {
                budget: 5,
                count: 6,
                correlation_id: "corr-1".into(),
            },
            "tool_budget_exhausted",
        ),
        (
            RuntimeEvent::BudgetEscalation {
                ladder: BudgetLadder::Turn,
                stage: BudgetStage::Warning,
                message: Some("wrap up".into()),
                correlation_id: "corr-1".into(),
            },
            "budget_escalation",
        ),
        (
            RuntimeEvent::Terminal {
                outcome: clean_outcome(),
            },
            "finish",
        ),
    ];
    for (event, expected_kind) in events {
        let value = serialize_event(event);
        assert_eq!(value["event"], expected_kind);
        assert_eq!(value["correlation_id"], "corr-1");
        assert!(value["ts"].as_u64().is_some());
    }
}

#[test]
fn event_field_renames_and_tool_error_flag_are_exact() {
    let reasoning = serialize_event(RuntimeEvent::Reasoning {
        summary: "summary".into(),
        payload: Some(json!({"payload": true})),
        model: "model".into(),
        correlation_id: "corr-1".into(),
    });
    assert_eq!(reasoning["summary"], "summary");
    assert_eq!(reasoning["payload"], json!({"payload": true}));
    let start = serialize_event(RuntimeEvent::ToolStart {
        call_id: "call-1".into(),
        tool: "sol".into(),
        arguments: json!({"command": "status"}),
        correlation_id: "corr-1".into(),
    });
    assert_eq!(start["args"], json!({"command": "status"}));
    assert!(start.get("arguments").is_none());
    let end = serialize_event(RuntimeEvent::ToolEnd {
        call_id: "call-1".into(),
        tool: "sol".into(),
        arguments: json!({"command": "status"}),
        result: "failed".into(),
        is_error: true,
        correlation_id: "corr-1".into(),
    });
    assert_eq!(end["args"], json!({"command": "status"}));
    assert_eq!(end["is_error"], true);
}

#[test]
fn terminal_outcomes_split_and_preserve_partial_error_result() {
    let clean = serialize_event(RuntimeEvent::Terminal {
        outcome: clean_outcome(),
    });
    assert_eq!(clean["event"], "finish");
    assert_eq!(clean["terminal"], true);
    for outcome in [
        RunOutcome {
            reason_code: Some("reason".into()),
            error_text: None,
            result: None,
            usage: Usage::default(),
            raw_payload: None,
            terminal: true,
            correlation_id: "corr-1".into(),
            provider_failure: None,
        },
        RunOutcome {
            reason_code: None,
            error_text: None,
            result: None,
            usage: Usage::default(),
            raw_payload: None,
            terminal: true,
            correlation_id: "corr-1".into(),
            provider_failure: Some(ConverseFailure {
                reason_code: "provider_response_invalid".into(),
                retryable: false,
                blocking: false,
            }),
        },
        RunOutcome {
            reason_code: None,
            error_text: Some("failure".into()),
            result: Some("partial answer".into()),
            usage: Usage::default(),
            raw_payload: None,
            terminal: true,
            correlation_id: "corr-1".into(),
            provider_failure: None,
        },
    ] {
        let value = serialize_event(RuntimeEvent::Terminal { outcome });
        assert_eq!(value["event"], "error");
        assert_eq!(value["terminal"], true);
    }
    let partial = serialize_event(RuntimeEvent::Terminal {
        outcome: RunOutcome {
            reason_code: Some("failed".into()),
            error_text: Some("failed after partial".into()),
            result: Some("partial answer".into()),
            usage: Usage::default(),
            raw_payload: None,
            terminal: true,
            correlation_id: "corr-1".into(),
            provider_failure: None,
        },
    });
    let serialized = serde_json::to_string(&partial).expect("event serializes");
    assert!(serialized.contains("\"result\":\"partial answer\""));

    // This models a hypothetical non-terminal condition such as quota
    // exhaustion. solstone-core-cogitate-runtime does not construct a
    // terminal: false outcome today, but the wire must preserve it when it does.
    let non_terminal = serialize_event(RuntimeEvent::Terminal {
        outcome: RunOutcome {
            reason_code: Some("quota_exhausted".into()),
            error_text: Some("quota exhausted".into()),
            result: None,
            usage: Usage::default(),
            raw_payload: None,
            terminal: false,
            correlation_id: "corr-1".into(),
            provider_failure: None,
        },
    });
    assert_eq!(non_terminal["event"], "error");
    assert_eq!(non_terminal["terminal"], false);
}

#[test]
fn raw_is_terminal_only() {
    let mut clean = clean_outcome();
    clean.raw_payload = Some(json!([{"raw": true}]));
    let finish = serialize_event(RuntimeEvent::Terminal { outcome: clean });
    assert_eq!(finish["raw"], json!([{"raw": true}]));
    let error = serialize_event(RuntimeEvent::Terminal {
        outcome: RunOutcome {
            reason_code: Some("failed".into()),
            error_text: Some("failed".into()),
            result: None,
            usage: Usage::default(),
            raw_payload: Some(json!({"raw": true})),
            terminal: true,
            correlation_id: "corr-1".into(),
            provider_failure: None,
        },
    });
    assert_eq!(error["raw"], json!({"raw": true}));
    for value in all_native_values().into_iter().take(6) {
        assert!(
            value.get("raw").is_none(),
            "{} must not carry raw",
            value["event"]
        );
    }
}

#[test]
fn tool_budget_and_budget_escalation_shapes_are_exact() {
    let budget = serialize_event(RuntimeEvent::SolBudgetExhausted {
        budget: 5,
        count: 6,
        correlation_id: "corr-1".into(),
    });
    assert_eq!(budget["tool"], "sol");
    assert_eq!(budget["budget"], 5);
    assert_eq!(budget["count"], 6);
    assert!(budget.get("read_tools").is_none());
    let escalation = serialize_event(RuntimeEvent::BudgetEscalation {
        ladder: BudgetLadder::Resource,
        stage: BudgetStage::ForceStopped,
        message: None,
        correlation_id: "corr-1".into(),
    });
    assert_eq!(escalation["ladder"], "resource");
    assert_eq!(escalation["stage"], "force_stopped");
    assert!(escalation["message"].is_null());
    assert_eq!(escalation["correlation_id"], "corr-1");
    assert!(escalation["ts"].as_u64().is_some());
}

fn all_native_values() -> Vec<Value> {
    vec![
        serialize_event(RuntimeEvent::TextDelta {
            delta: "chunk".into(),
            model: "model".into(),
            correlation_id: "corr-1".into(),
        }),
        serialize_event(RuntimeEvent::Reasoning {
            summary: "summary".into(),
            payload: None,
            model: "model".into(),
            correlation_id: "corr-1".into(),
        }),
        serialize_event(RuntimeEvent::ToolStart {
            call_id: "call-1".into(),
            tool: "sol".into(),
            arguments: json!({}),
            correlation_id: "corr-1".into(),
        }),
        serialize_event(RuntimeEvent::ToolEnd {
            call_id: "call-1".into(),
            tool: "sol".into(),
            arguments: json!({}),
            result: "ok".into(),
            is_error: false,
            correlation_id: "corr-1".into(),
        }),
        serialize_event(RuntimeEvent::SolBudgetExhausted {
            budget: 5,
            count: 6,
            correlation_id: "corr-1".into(),
        }),
        serialize_event(RuntimeEvent::BudgetEscalation {
            ladder: BudgetLadder::Turn,
            stage: BudgetStage::FinalTurn,
            message: Some("finish".into()),
            correlation_id: "corr-1".into(),
        }),
        serialize_event(RuntimeEvent::Terminal {
            outcome: clean_outcome(),
        }),
        serialize_event(RuntimeEvent::Terminal {
            outcome: RunOutcome {
                reason_code: Some("failed".into()),
                error_text: Some("failed".into()),
                result: None,
                usage: Usage::default(),
                raw_payload: None,
                terminal: true,
                correlation_id: "corr-1".into(),
                provider_failure: None,
            },
        }),
        serialize_dry_run(&request()).expect("dry run validates"),
    ]
}

#[test]
fn validator_accepts_every_serialized_native_event() {
    for value in all_native_values() {
        validate_event(&value).expect("serializer value validates");
    }
}

#[test]
fn validator_rejects_missing_required_fields_for_each_native_kind() {
    let fields = [
        "delta", "summary", "call_id", "result", "tool", "ladder", "terminal", "error", "dry_run",
    ];
    for (mut value, field) in all_native_values().into_iter().zip(fields) {
        value
            .as_object_mut()
            .expect("event is object")
            .remove(field);
        assert!(
            validate_event(&value).is_err(),
            "{} without {field}",
            value["event"]
        );
    }
}

#[test]
fn validator_wiring_source_guard_requires_emit_path_call() {
    // RuntimeEvent is type-safe and serialize_event is total, so no public
    // RuntimeEvent can make serialize_event_validated produce a malformed
    // value. Validator behavior is tested separately; this source guard is
    // the structural proof that the normal emit path invokes it.
    let source = include_str!("event.rs");
    let signature = "pub fn serialize_event_validated";
    let start = source.find(signature).expect("validated serializer exists");
    let body_start = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .expect("validated serializer has a body");
    let body_end = matching_brace(source, body_start).expect("validated serializer body closes");
    let body = &source[body_start..=body_end];
    assert!(
        body.contains("validate_event(&value)?;"),
        "serialize_event_validated must invoke validate_event before returning"
    );
}

fn matching_brace(source: &str, opening: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (offset, character) in source[opening..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(opening + offset);
                }
            }
            _ => {}
        }
    }
    None
}

#[test]
fn dry_run_emits_one_validated_terminal_event_without_provider() {
    let mut request = request();
    request.dry_run = true;
    let mut provider = PanicProvider;
    let mut tools = FinalToolExecutor;
    let mut sink = RecordingEventSink::default();
    let NativeRun::DryRun(event) =
        run_or_dry_run(&request, &mut provider, &mut tools, &mut sink).expect("dry run event")
    else {
        panic!("dry run must not enter the runtime");
    };
    assert!(sink.events.is_empty());
    assert_eq!(event["event"], "dry_run");
    assert_eq!(event["terminal"], true);
    validate_event(&event).expect("dry run validates");
}

struct PanicProvider;

impl ConverseProvider for PanicProvider {
    fn converse(
        &mut self,
        _model: &str,
        _system_instruction: Option<&str>,
        _messages: &[ConverseMessage],
        _tools: &[ConverseToolSpec],
        _deadline: std::time::Duration,
    ) -> Result<solstone_core_cogitate_runtime::ProviderResponse, ConverseFailure> {
        panic!("dry-run must not invoke a provider")
    }
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
    let mut provider = EndpointConverseProvider::new(
        &request,
        EndpointOverrides::from_values(Some(url), Some(secret.to_owned())),
    )
    .expect("provider configuration");
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
    let config_error = EndpointConfigurationError::MissingEndpointUrl;
    assert!(!format!("{config_error}").contains(secret));
    assert!(!format!("{config_error:?}").contains(secret));
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
