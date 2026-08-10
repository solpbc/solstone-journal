// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use solstone_core_cogitate_runtime::events::{BudgetLadder, BudgetStage};
use solstone_core_cogitate_runtime::{
    CogitateToolExecutor, ConverseProvider, RecordingEventSink, RunOutcome, RuntimeEvent,
    ToolExecution, ToolExecutor, Usage, run_cogitate,
};
use solstone_core_cogitate_tools::NoopSlotLease;
use solstone_core_generate_wire::{
    ConverseFailure, ConverseMessage, ConverseToolCall, ConverseToolSpec, resolve_lane,
};

use crate::{
    COGITATE_API_KEY_OVERRIDE_ENV, COGITATE_ENDPOINT_URL_OVERRIDE_ENV, CogitateRequest,
    DispatchConverseProvider, EndpointOverrides, NativeRun, REQUEST_SCHEMA, contract_source,
    run_or_dry_run, serialize_dry_run, serialize_event, serialize_event_validated, validate_event,
};

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

/// The published request contract must describe the record the parser accepts.
///
/// `validate_event` enforces the `cortex_events` half of this fixture, so a
/// producer cannot drift from it. Nothing enforced the `request_schema` half,
/// and the v2 bump landed with the fixture still describing v1 -- two
/// disagreeing descriptions of one record, which is the class this contract
/// exists to prevent. Asserted in both directions so neither the fixture nor
/// the parser can move alone.
#[test]
fn request_contract_fixture_describes_the_record_the_parser_accepts() {
    let contract: Value =
        serde_json::from_str(contract_source()).expect("wire contract fixture is valid JSON");
    let schema = &contract["request_schema"];

    assert_eq!(
        schema["id"].as_str(),
        Some(REQUEST_SCHEMA),
        "fixture request_schema.id must name the schema the parser requires"
    );

    let mut declared: Vec<String> = Vec::new();
    for section in ["required_fields", "optional_fields"] {
        for key in schema[section]
            .as_object()
            .expect("field section is an object")
            .keys()
        {
            declared.push(key.clone());
        }
    }

    // Forward: nothing the fixture advertises is refused as unknown by the
    // parser. A field the contract promises but the record rejects is a lie.
    for field in &declared {
        let mut value = request_value();
        let object = value.as_object_mut().expect("request is an object");
        object.entry(field.clone()).or_insert(Value::Null);
        if let Err(error) = CogitateRequest::from_value(&value) {
            assert!(
                !error.to_string().contains("unknown field"),
                "fixture declares {field:?} but the parser rejects it as unknown"
            );
        }
    }

    // Reverse: a field the fixture does not declare is refused. Without this
    // the parser could quietly widen and the contract would still read true.
    let mut widened = request_value();
    widened
        .as_object_mut()
        .expect("request is an object")
        .insert("undeclared_field".to_owned(), Value::Bool(true));
    let error = CogitateRequest::from_value(&widened)
        .expect_err("a field absent from the contract must be rejected");
    assert!(
        error.to_string().contains("unknown field"),
        "unexpected rejection reason: {error}"
    );

    // The retired v1 fields must be gone from the description too.
    for retired in ["expects_emit_final", "system_instruction"] {
        assert!(
            !declared.iter().any(|field| field == retired),
            "{retired:?} is retired in v2 but still declared in the fixture"
        );
    }
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
    assert!(input.config.expects_emit_final);
    assert_eq!(input.initial_prompt, "Do the task.");
    assert!(
        input
            .system_instruction
            .as_deref()
            .is_some_and(|instruction| instruction.contains("Be concise."))
    );
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
        (
            "legacy finalization",
            json!({"expects_emit_final": true}),
            "unknown field \"expects_emit_final\"",
        ),
        (
            "legacy instruction",
            json!({"system_instruction": "legacy"}),
            "unknown field \"system_instruction\"",
        ),
        (
            "scope scalar",
            json!({"read_scope": "chronicle/20260809"}),
            "read_scope must be an array of strings or null",
        ),
        (
            "scope nested",
            json!({"read_scope": [["chronicle/20260809"]]}),
            "read_scope must be an array of strings or null",
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
fn v2_schema_rejects_v1_and_defaults_read_scope_to_empty() {
    let mut v1 = request_value();
    v1["schema"] = json!("solstone-cogitate-request-v1");
    let error = CogitateRequest::from_value(&v1).expect_err("v1 is rejected");
    assert_eq!(
        error.to_string(),
        "malformed request: schema must be \"solstone-cogitate-request-v2\", got \"solstone-cogitate-request-v1\""
    );

    let mut missing_scope = request_value();
    missing_scope
        .as_object_mut()
        .expect("request object")
        .remove("read_scope");
    assert!(
        CogitateRequest::from_value(&missing_scope)
            .expect("missing scope defaults")
            .read_scope
            .is_empty()
    );
}

#[test]
fn request_derives_terminal_tool_from_finalization_inputs() {
    for (schedule, expected_tool, expected_argument) in [
        ("daily", "emit_final", "content"),
        ("segment", "finish", "message"),
    ] {
        let mut value = request_value();
        value["schedule"] = json!(schedule);
        let input = CogitateRequest::from_value(&value)
            .expect("request is valid")
            .to_run_input();
        let mut slot = NoopSlotLease;
        let executor = CogitateToolExecutor::new(
            &input.journal_root,
            input.config.read_call_budget,
            &mut slot,
        );
        let terminal = executor
            .offered_tools(&input.config)
            .expect("offered tools")
            .into_iter()
            .find(|tool| tool.name == expected_tool)
            .expect("expected terminal tool is offered");
        assert!(input.config.expects_emit_final == (schedule == "daily"));
        assert_eq!(terminal.parameters["required"], json!([expected_argument]));
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
const PROVIDER_KEY_CHILD: &str = "SOLSTONE_COGITATE_WIRE_PROVIDER_KEY_CHILD";

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
fn ambient_provider_key_does_not_bypass_missing_journal_credential() {
    if std::env::var_os(PROVIDER_KEY_CHILD).is_none() {
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .arg("--exact")
            .arg("tests::ambient_provider_key_does_not_bypass_missing_journal_credential")
            .env(PROVIDER_KEY_CHILD, "1")
            .env("GOOGLE_API_KEY", "ambient-only-key")
            .env_remove("SOLSTONE_GENERATE_API_KEY_OVERRIDE")
            .status()
            .expect("run child test");
        assert!(status.success());
        return;
    }

    let config = json!({"providers": {"active": {"provider": "google"}}})
        .as_object()
        .expect("config is an object")
        .clone();
    let (_, lane) = resolve_lane(&config);
    let mut provider = DispatchConverseProvider::from_lane(
        &request(),
        config,
        lane,
        EndpointOverrides::from_values(None, None),
    )
    .expect("google provider constructs");
    let failure = provider
        .converse(
            "request-model",
            None,
            &[ConverseMessage::User {
                text: "hello".to_owned(),
            }],
            &[],
            std::time::Duration::from_secs(1),
        )
        .expect_err("missing config key refuses before transport");
    assert_eq!(failure.reason_code, "provider_key_missing");
}

#[test]
fn journal_configuration_selects_each_executable_cogitate_arm() {
    let cases = [
        (
            json!({"providers": {"active": {"provider": "local"}, "local": {"endpoint_url": "http://endpoint", "served_model_id": "served"}}}),
            "endpoint",
        ),
        (
            json!({"providers": {"active": {"provider": "local"}, "local": {"endpoint_url": "http://endpoint", "served_model_id": "served"}}, "services": {"confidential": {}}}),
            "confidential",
        ),
        (
            json!({"providers": {"active": {"provider": "google"}}}),
            "google",
        ),
        (
            json!({"providers": {"active": {"provider": "anthropic"}}}),
            "anthropic",
        ),
        (
            json!({"providers": {"active": {"provider": "openai"}}}),
            "openai",
        ),
    ];
    for (value, expected) in cases {
        let config = value.as_object().expect("config is an object").clone();
        let (_, lane) = resolve_lane(&config);
        let provider = DispatchConverseProvider::from_lane(
            &request(),
            config,
            lane,
            EndpointOverrides::from_values(None, None),
        )
        .expect("executable lane constructs a provider");
        assert_eq!(provider.arm_name(), expected);
    }
}

#[test]
fn confidential_dispatch_stops_at_attestation_before_endpoint_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint listener");
    let port = listener.local_addr().expect("endpoint address").port();
    let missing_nvattest_dir = std::env::temp_dir().join(format!(
        "solstone-missing-nvattest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
    ));
    let config = json!({
        "providers": {
            "active": {"provider": "local"},
            "local": {
                "endpoint_url": format!("http://127.0.0.1:{port}"),
                "served_model_id": "configured-model"
            }
        },
        "services": {"confidential": {"nvattest_dir": missing_nvattest_dir}}
    })
    .as_object()
    .expect("config is an object")
    .clone();
    let (_, lane) = resolve_lane(&config);
    let mut provider = DispatchConverseProvider::from_lane(
        &request(),
        config,
        lane,
        EndpointOverrides::from_values(None, None),
    )
    .expect("confidential provider constructs");
    let failure = provider
        .converse(
            "request-model",
            None,
            &[ConverseMessage::User {
                text: "hello".to_owned(),
            }],
            &[],
            std::time::Duration::from_secs(1),
        )
        .expect_err("attestation prerequisite refuses confidential lane");
    assert_eq!(failure.reason_code, "attestation_not_yet_verified");
    listener.set_nonblocking(true).expect("set nonblocking");
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == ErrorKind::WouldBlock
    ));
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
fn terminal_events_always_include_usage_on_finish_and_error() {
    let usage = Usage {
        input_tokens: 2,
        output_tokens: 3,
        cached_tokens: 4,
        cache_creation_tokens: 5,
        reasoning_tokens: 6,
        requests: 1,
    };
    let finish = serialize_event_validated(RuntimeEvent::Terminal {
        outcome: RunOutcome::clean(Some("done".to_owned()), usage.clone(), "corr-1".to_owned()),
    })
    .expect("finish validates");
    let error = serialize_event_validated(RuntimeEvent::Terminal {
        outcome: RunOutcome::provider_failure(
            ConverseFailure {
                reason_code: "provider_key_missing".to_owned(),
                retryable: false,
                blocking: true,
            },
            usage,
            "corr-1".to_owned(),
        ),
    })
    .expect("error validates");
    for event in [finish, error] {
        assert_eq!(event["usage"]["input_tokens"], 2);
        assert_eq!(event["usage"]["output_tokens"], 3);
        assert_eq!(event["usage"]["cached_tokens"], 4);
        assert_eq!(event["usage"]["cache_creation_tokens"], 5);
        assert_eq!(event["usage"]["reasoning_tokens"], 6);
        assert_eq!(event["usage"]["requests"], 1);
    }
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
