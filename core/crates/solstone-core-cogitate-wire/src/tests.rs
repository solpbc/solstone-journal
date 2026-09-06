// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_cogitate_runtime::events::{BudgetLadder, BudgetStage};
use solstone_core_cogitate_runtime::{
    CogitateToolExecutor, ConverseProvider, RecordingEventSink, RunOutcome, RuntimeEvent,
    ToolExecution, ToolExecutor, Usage, run_cogitate,
};
use solstone_core_cogitate_tools::NoopSlotLease;
use solstone_core_generate_wire::{
    ConverseFailure, ConverseMessage, ConverseToolCall, ConverseToolSpec, EndpointTransport,
    EndpointTransportError, resolve_lane,
};
use solstone_core_local::HttpResponse;

use crate::{
    CogitateRequest, DispatchConverseProvider, EndpointOverrides, NativeRun, REQUEST_SCHEMA,
    contract_source, run_or_dry_run, serialize_dry_run, serialize_event, serialize_event_validated,
    validate_event,
};

fn request_value() -> Value {
    json!({
        "schema": REQUEST_SCHEMA,
        "access_tier": "normal",
        "outbound_approval": null,
        "diagnostic": false,
        "talent_instruction": "Be concise.",
        "sol_tool_name": "solstone",
        "read_scope": [],
        "output_path": null,
        "schedule": "daily",
        "max_turns": 4,
        "context_window": 4096,
        "timeout_ms": 30_000,
        "read_call_budget": 5,
        "model": "fixture-model",
        "correlation_id": "corr-1",
        "initial_prompt": "Do the task.",
        "journal_root": "/var/tmp/solstone-cogitate-wire-test"
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
    for retired in ["expects_emit_final", "system_instruction", "cost_cap_usd"] {
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
        std::path::PathBuf::from("/var/tmp/solstone-cogitate-wire-test")
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

#[test]
fn journal_configuration_selects_each_executable_cogitate_arm() {
    let cases = [
        (
            json!({"providers": {"active": {"provider": "local"}}}),
            "bundled",
        ),
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
    let config = json!({
        "providers": {
            "active": {"provider": "local"},
            "local": {
                "endpoint_url": "http://127.0.0.1:1",
                "served_model_id": "configured-model"
            }
        },
        "services": {"confidential": {}}
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
        .converse_confidential_with_controls(
            "request-model",
            None,
            &[ConverseMessage::User {
                text: "hello".to_owned(),
            }],
            &[],
            std::time::Duration::from_secs(1),
            std::time::UNIX_EPOCH,
            |_| solstone_core_spp_ratls::NvattestEnsureStatus::Unavailable,
            |_, _| panic!("channel establishment must not run after failed readiness"),
        )
        .expect_err("attestation prerequisite refuses confidential lane");
    assert_eq!(failure.reason_code, "attestation_not_yet_verified");
}

#[test]
fn converse_dispatches_confidential_arm_without_downgrading() {
    let blocked_parent = std::env::temp_dir().join(format!(
        "solstone-blocked-nvattest-confidential-dispatch-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&blocked_parent);
    std::fs::write(&blocked_parent, "not a directory").expect("write nvattest install blocker");
    let unavailable_nvattest_dir = blocked_parent.join("nvattest");
    let config = json!({
        "providers": {
            "active": {"provider": "local"},
            "local": {
                "endpoint_url": "http://127.0.0.1:1",
                "served_model_id": "configured-model"
            }
        },
        "services": {"confidential": {"nvattest_dir": unavailable_nvattest_dir}}
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
    std::fs::remove_file(blocked_parent).expect("remove nvattest install blocker");
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
                tool: "solstone".into(),
                arguments: json!({"command": "sol status"}),
                correlation_id: "corr-1".into(),
            },
            "tool_start",
        ),
        (
            RuntimeEvent::ToolEnd {
                call_id: "call-1".into(),
                tool: "solstone".into(),
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
        tool: "solstone".into(),
        arguments: json!({"command": "status"}),
        correlation_id: "corr-1".into(),
    });
    assert_eq!(start["args"], json!({"command": "status"}));
    assert!(start.get("arguments").is_none());
    let end = serialize_event(RuntimeEvent::ToolEnd {
        call_id: "call-1".into(),
        tool: "solstone".into(),
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
                detail: None,
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
                detail: None,
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
    assert_eq!(budget["tool"], "solstone");
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
            tool: "solstone".into(),
            arguments: json!({}),
            correlation_id: "corr-1".into(),
        }),
        serialize_event(RuntimeEvent::ToolEnd {
            call_id: "call-1".into(),
            tool: "solstone".into(),
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
        "delta",
        "summary",
        "call_id",
        "result",
        "tool",
        "ladder",
        "terminal",
        "error",
        "expects_emit_final",
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
    assert_eq!(event["rendered_prompt"]["initial_prompt"], "Do the task.");
    assert!(
        event["rendered_prompt"]["system_instruction"]
            .as_str()
            .is_some_and(|instruction| instruction.contains("Be concise."))
    );
    assert_eq!(event["expects_emit_final"], true);
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

struct CapturingEndpointTransport {
    response: Result<HttpResponse, EndpointTransportError>,
    calls: Vec<(String, String, Value, Option<String>)>,
}

struct InjectedEndpointProvider<'a> {
    provider: &'a mut DispatchConverseProvider,
    transport: &'a mut CapturingEndpointTransport,
}

impl ConverseProvider for InjectedEndpointProvider<'_> {
    fn converse(
        &mut self,
        model: &str,
        system_instruction: Option<&str>,
        messages: &[ConverseMessage],
        tools: &[ConverseToolSpec],
        deadline: std::time::Duration,
    ) -> Result<solstone_core_cogitate_runtime::ProviderResponse, ConverseFailure> {
        self.provider.converse_endpoint_with_transport(
            model,
            system_instruction,
            messages,
            tools,
            deadline,
            self.transport,
        )
    }
}

fn serialized_events(sink: RecordingEventSink) -> String {
    sink.events
        .into_iter()
        .map(|event| {
            serialize_event_validated(event)
                .expect("captured event validates")
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl EndpointTransport for CapturingEndpointTransport {
    fn get(
        &mut self,
        _base_url: &str,
        _path: &str,
        _credential: Option<&str>,
        _timeout: std::time::Duration,
    ) -> Result<HttpResponse, EndpointTransportError> {
        panic!("served context is configured; model discovery must not run")
    }

    fn post_json(
        &mut self,
        base_url: &str,
        path: &str,
        body: &Value,
        credential: Option<&str>,
        _timeout: std::time::Duration,
    ) -> Result<HttpResponse, EndpointTransportError> {
        self.calls.push((
            base_url.to_owned(),
            path.to_owned(),
            body.clone(),
            credential.map(str::to_owned),
        ));
        self.response.clone()
    }
}

fn endpoint_provider(secret: &str, confidential: bool) -> DispatchConverseProvider {
    endpoint_provider_for(&request(), secret, confidential)
}

fn endpoint_provider_for(
    request: &CogitateRequest,
    secret: &str,
    confidential: bool,
) -> DispatchConverseProvider {
    let mut value = json!({
        "providers": {
            "active": {"provider": "local"},
            "local": {
                "endpoint_url": "http://configured.invalid",
                "served_model_id": "configured",
                "served_context_window": 32768
            }
        }
    });
    if confidential {
        value["services"] = json!({"confidential": {}});
    }
    let config = value.as_object().expect("config object").clone();
    let (_, lane) = resolve_lane(&config);
    DispatchConverseProvider::from_lane(
        request,
        config,
        lane,
        EndpointOverrides::from_values(
            Some("http://127.0.0.1:9443".to_owned()),
            Some(secret.to_owned()),
        ),
    )
    .expect("endpoint provider")
}

fn final_turn_response() -> String {
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
    .to_string()
}

#[test]
fn endpoint_request_mapping_forwards_credential_without_serializing_it() {
    let secret = "deterministic-wire-secret";
    let mut provider = endpoint_provider(secret, false);
    let mut transport = CapturingEndpointTransport {
        response: Ok(HttpResponse {
            status: 200,
            body: final_turn_response(),
        }),
        calls: Vec::new(),
    };
    let response = provider
        .converse_endpoint_with_transport(
            "requested-model",
            Some("system"),
            &[ConverseMessage::User {
                text: "hello".to_owned(),
            }],
            &[ConverseToolSpec {
                name: "emit_final".to_owned(),
                description: "finish".to_owned(),
                parameters: json!({"type": "object"}),
            }],
            std::time::Duration::from_secs(1),
            &mut transport,
        )
        .expect("captured endpoint turn");
    assert_eq!(response.turn.model, "requested-model");
    assert_eq!(transport.calls.len(), 1);
    let (base_url, path, body, credential) = &transport.calls[0];
    assert_eq!(base_url, "http://127.0.0.1:9443");
    assert_eq!(path, "/v1/chat/completions");
    assert_eq!(credential.as_deref(), Some(secret));
    assert!(!body.to_string().contains(secret));

    let mut injected = InjectedEndpointProvider {
        provider: &mut provider,
        transport: &mut transport,
    };
    let mut tools = FinalToolExecutor;
    let mut sink = RecordingEventSink::default();
    run_cogitate(
        &mut injected,
        &mut tools,
        request().to_run_input(),
        &mut sink,
    );
    assert!(!serialized_events(sink).contains(secret));

    let malformed =
        CogitateRequest::from_value(&json!({"credential": secret})).expect_err("bad request");
    assert!(!format!("{malformed:?}").contains(secret));
    let validation = validate_event(&json!({
        "event": "text_delta",
        "ts": 1,
        "correlation_id": "corr-1",
        "delta": "chunk",
        "model": "model",
        "credential": secret
    }))
    .expect_err("undeclared credential is rejected");
    assert!(!format!("{validation:?}").contains(secret));
}

#[test]
fn endpoint_failure_redacts_credential_and_untrusted_response_body() {
    let secret = "deterministic-wire-secret";
    let mut provider = endpoint_provider(secret, false);
    let mut transport = CapturingEndpointTransport {
        response: Ok(HttpResponse {
            status: 500,
            body: format!("upstream echoed {secret}"),
        }),
        calls: Vec::new(),
    };
    let failure = provider
        .converse_endpoint_with_transport(
            "requested-model",
            None,
            &[ConverseMessage::User {
                text: "hello".to_owned(),
            }],
            &[],
            std::time::Duration::from_secs(1),
            &mut transport,
        )
        .expect_err("500 is normalized");
    assert_eq!(transport.calls[0].3.as_deref(), Some(secret));
    assert_eq!(failure.reason_code, "provider_response_invalid");
    assert!(!format!("{failure:?}").contains(secret));

    let mut injected = InjectedEndpointProvider {
        provider: &mut provider,
        transport: &mut transport,
    };
    let mut tools = FinalToolExecutor;
    let mut sink = RecordingEventSink::default();
    run_cogitate(
        &mut injected,
        &mut tools,
        request().to_run_input(),
        &mut sink,
    );
    let events = serialized_events(sink);
    assert!(events.contains("provider_response_invalid"));
    assert!(!events.contains(secret));
}

struct OrderedAttestedChannel {
    response: std::io::Cursor<Vec<u8>>,
    order: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
    endpoint_recorded: bool,
}

impl std::io::Read for OrderedAttestedChannel {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut self.response, buffer)
    }
}

impl std::io::Write for OrderedAttestedChannel {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if !self.endpoint_recorded {
            self.order.borrow_mut().push("endpoint");
            self.endpoint_recorded = true;
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl solstone_core_spp_ratls::AttestedIo for OrderedAttestedChannel {
    fn set_io_timeout(&mut self, _: Option<std::time::Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

fn attestation_verdict() -> solstone_core_spp_ratls::CompositeVerdict {
    use solstone_core_spp_attest::{
        nvgpu::claims::GpuAppraisal,
        snp::{CpuAppraisal, CpuTcb, TcbVersion},
    };
    let tcb = TcbVersion {
        boot_loader: None,
        tee: None,
        snp: None,
        microcode: None,
        fmc: None,
    };
    solstone_core_spp_ratls::CompositeVerdict {
        verified: true,
        legs: ["cpu", "gpu"],
        substrate: String::new(),
        checked_at: std::time::UNIX_EPOCH,
        cpu: CpuAppraisal {
            steps: Vec::new(),
            hcla_version: 0,
            report_version: 0,
            cpuid_family: None,
            cpuid_model: None,
            cpuid_step: None,
            tcb: CpuTcb {
                current: tcb.clone(),
                reported: tcb.clone(),
                committed: tcb.clone(),
                launch: tcb,
            },
            pcr_sha256: String::new(),
            host_data_hex: String::new(),
            measurement_hex: String::new(),
            chip_id_hex: String::new(),
        },
        gpu: GpuAppraisal {
            steps: Vec::new(),
            driver_version: String::new(),
            vbios_version: String::new(),
            hwmodel: String::new(),
            ueid: String::new(),
            oemid: String::new(),
            eat_nonce: String::new(),
            claims_version: String::new(),
            arch: String::new(),
            envelope_gpu_uuid: String::new(),
        },
    }
}

fn framed_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[test]
fn confidential_dispatch_orders_readiness_channel_then_endpoint_exactly_once() {
    let secret = "confidential-secret";
    let order = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut provider = endpoint_provider(secret, true);
    let readiness_order = std::rc::Rc::clone(&order);
    let establish_order = std::rc::Rc::clone(&order);
    let channel_order = std::rc::Rc::clone(&order);
    provider
        .converse_confidential_with_controls(
            "requested-model",
            None,
            &[ConverseMessage::User {
                text: "hello".to_owned(),
            }],
            &[],
            std::time::Duration::from_secs(1),
            std::time::UNIX_EPOCH,
            move |_| {
                readiness_order.borrow_mut().push("readiness");
                solstone_core_spp_ratls::NvattestEnsureStatus::AlreadyInstalled
            },
            move |_, _| {
                establish_order.borrow_mut().push("channel");
                Ok((
                    attestation_verdict(),
                    Box::new(OrderedAttestedChannel {
                        response: std::io::Cursor::new(framed_response(&final_turn_response())),
                        order: channel_order,
                        endpoint_recorded: false,
                    }) as Box<dyn solstone_core_spp_ratls::AttestedIo>,
                ))
            },
        )
        .expect("confidential dispatch succeeds");
    assert_eq!(&*order.borrow(), &["readiness", "channel", "endpoint"]);
}

#[test]
fn confidential_dispatch_stops_after_failed_readiness() {
    let order = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut provider = endpoint_provider("unused", true);
    let readiness_order = std::rc::Rc::clone(&order);
    let failure = provider
        .converse_confidential_with_controls(
            "requested-model",
            None,
            &[ConverseMessage::User {
                text: "hello".to_owned(),
            }],
            &[],
            std::time::Duration::from_secs(1),
            std::time::UNIX_EPOCH,
            move |_| {
                readiness_order.borrow_mut().push("readiness");
                solstone_core_spp_ratls::NvattestEnsureStatus::InstallFailed
            },
            |_, _| panic!("channel establishment must not run after failed readiness"),
        )
        .expect_err("failed readiness refuses dispatch");
    assert_eq!(failure.reason_code, "attestation_not_yet_verified");
    assert_eq!(&*order.borrow(), &["readiness"]);
}

#[test]
fn confidential_dispatch_ready_negative_attempts_one_channel_and_zero_endpoints() {
    let order = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut provider = endpoint_provider("unused", true);
    let readiness_order = std::rc::Rc::clone(&order);
    let establish_order = std::rc::Rc::clone(&order);
    let failure = provider
        .converse_confidential_with_controls(
            "requested-model",
            None,
            &[ConverseMessage::User {
                text: "hello".to_owned(),
            }],
            &[],
            std::time::Duration::from_secs(1),
            std::time::UNIX_EPOCH,
            move |_| {
                readiness_order.borrow_mut().push("readiness");
                solstone_core_spp_ratls::NvattestEnsureStatus::AlreadyInstalled
            },
            move |_, _| {
                establish_order.borrow_mut().push("channel");
                Err("tls_handshake_failed")
            },
        )
        .expect_err("failed channel establishment refuses dispatch");
    assert_eq!(failure.reason_code, "attestation_failed");
    assert_eq!(&*order.borrow(), &["readiness", "channel"]);
}

#[test]
fn completion_budget_is_a_validated_request_value() {
    let mut value = request_value();
    value["max_output_tokens"] = json!(4096);
    let parsed = CogitateRequest::from_value(&value).expect("explicit completion budget");
    assert_eq!(parsed.to_value()["max_output_tokens"], 4096);
    for invalid in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("4096"),
        json!(u64::from(u32::MAX) + 1),
    ] {
        value["max_output_tokens"] = invalid;
        assert!(CogitateRequest::from_value(&value).is_err());
    }
}

#[test]
fn completion_budget_reaches_the_actual_endpoint_request() {
    for (window, explicit, expected) in [
        (None, None, 8192),
        (Some(4096), None, 1024),
        (Some(32768), Some(6000), 6000),
        (Some(32768), Some(8), 8),
    ] {
        let mut value = request_value();
        value["context_window"] = json!(window);
        value["max_output_tokens"] = json!(explicit);
        let request = CogitateRequest::from_value(&value).unwrap();
        let mut provider = endpoint_provider_for(&request, "", false);
        let mut transport = CapturingEndpointTransport {
            response: Ok(HttpResponse {
                status: 200,
                body: final_turn_response(),
            }),
            calls: Vec::new(),
        };
        provider
            .converse_endpoint_with_transport(
                "model",
                None,
                &[ConverseMessage::User {
                    text: "hello".to_owned(),
                }],
                &[],
                std::time::Duration::from_secs(1),
                &mut transport,
            )
            .unwrap();
        assert_eq!(transport.calls[0].2["max_tokens"], expected);
    }
}
