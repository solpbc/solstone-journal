// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_cogitate_tools::{SlotLease, SlotReacquireError};
use solstone_core_generate_wire::{
    ConverseFailure, ConverseMessage, ConverseToolCall, ConverseToolSpec, ConverseTurn,
};

use crate::config::{RunConfig, RunInput};
use crate::events::{BudgetLadder, BudgetStage, RecordingEventSink, RuntimeEvent};
use crate::ladders::{ResourceLadder, TurnLadder};
use crate::outcome::{RunOutcome, TailState, compose_tail};
use crate::provider::{ConverseProvider, ProviderResponse};
use crate::runtime::run_cogitate;
use crate::stuck::{HistoryEntry, StuckDetector};
use crate::tools::{CogitateToolExecutor, ToolExecution, ToolExecutor};
use crate::{TOOL_BINDING_SETUP_FAILED, Usage};

#[derive(Default)]
struct ScriptedProvider {
    responses: VecDeque<Result<ProviderResponse, ConverseFailure>>,
    seen_messages: Vec<Vec<ConverseMessage>>,
}

impl ScriptedProvider {
    fn new(responses: impl IntoIterator<Item = Result<ProviderResponse, ConverseFailure>>) -> Self {
        Self {
            responses: responses.into_iter().collect(),
            seen_messages: Vec::new(),
        }
    }
}

impl ConverseProvider for ScriptedProvider {
    fn converse(
        &mut self,
        _model: &str,
        _system: Option<&str>,
        messages: &[ConverseMessage],
        _tools: &[ConverseToolSpec],
        _deadline: Duration,
    ) -> Result<ProviderResponse, ConverseFailure> {
        self.seen_messages.push(messages.to_vec());
        self.responses.pop_front().expect("script has a response")
    }
}

#[derive(Default)]
struct ScriptedTools {
    executions: VecDeque<ToolExecution>,
    calls: Vec<String>,
}

struct FailingLease;

struct FailingSetupTools;

impl SlotLease for FailingLease {
    fn yield_slot(&mut self) {}
    fn reacquire(&mut self) -> Result<(), SlotReacquireError> {
        Err(SlotReacquireError::Other("slot disappeared".to_owned()))
    }
    fn cancel_pending_reacquire(&mut self) {}
}

impl ToolExecutor for ScriptedTools {
    fn offered_tools(&self, _config: &RunConfig) -> Result<Vec<ConverseToolSpec>, String> {
        Ok(Vec::new())
    }
    fn execute(&mut self, _config: &RunConfig, call: &ConverseToolCall) -> ToolExecution {
        self.calls.push(call.name.clone());
        self.executions
            .pop_front()
            .unwrap_or_else(|| ToolExecution {
                output: "ok".to_owned(),
                is_error: false,
                sol_budget_exhausted: None,
                slot_reacquire_error: None,
            })
    }
}

impl ToolExecutor for FailingSetupTools {
    fn offered_tools(&self, _config: &RunConfig) -> Result<Vec<ConverseToolSpec>, String> {
        Err("unknown access_tier: invalid".to_owned())
    }

    fn execute(&mut self, _config: &RunConfig, _call: &ConverseToolCall) -> ToolExecution {
        unreachable!("setup failure prevents dispatch")
    }
}

fn input(mut config: RunConfig) -> RunInput {
    config.correlation_id = "cid".to_owned();
    RunInput {
        config,
        initial_prompt: "do work".to_owned(),
        system_instruction: None,
        journal_root: PathBuf::from("."),
    }
}

fn turn(text: &str, calls: Vec<ConverseToolCall>, usage: Value) -> ProviderResponse {
    ProviderResponse {
        turn: ConverseTurn {
            text: text.to_owned(),
            tool_calls: calls,
            finish_reason: "stop".to_owned(),
            usage,
            model: "test".to_owned(),
            thinking: None,
        },
        response_id: "response-1".to_owned(),
    }
}

fn call(name: &str, arguments: Value) -> ConverseToolCall {
    ConverseToolCall {
        id: format!("{name}-id"),
        name: name.to_owned(),
        arguments,
        not_offered: false,
        thought_signature: None,
    }
}

fn final_call(expects_emit_final: bool, text: &str) -> ConverseToolCall {
    if expects_emit_final {
        call("emit_final", json!({"content": text}))
    } else {
        call("finish", json!({"message": text}))
    }
}

#[test]
fn explicit_final_tool_ends_without_dispatching_a_tool() {
    let config = RunConfig::default();
    let mut provider = ScriptedProvider::new([Ok(turn(
        "ignored",
        vec![final_call(false, "done")],
        json!({}),
    ))]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert_eq!(outcome.reason_code, None);
    assert!(tools.calls.is_empty());
    assert_eq!(provider.seen_messages.len(), 1);
}

#[test]
fn emit_final_ends_an_expects_final_run_without_dispatching_a_tool() {
    let config = RunConfig {
        expects_emit_final: true,
        ..RunConfig::default()
    };
    let mut provider = ScriptedProvider::new([Ok(turn(
        "ignored",
        vec![final_call(true, "artifact")],
        json!({}),
    ))]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.result.as_deref(), Some("artifact"));
    assert_eq!(outcome.reason_code, None);
    assert!(tools.calls.is_empty());
}

#[test]
fn tool_observation_is_carried_into_the_next_provider_turn() {
    let mut provider = ScriptedProvider::new([
        Ok(turn(
            "",
            vec![call("read_file", json!({"path":"note.txt"}))],
            json!({"input_tokens": 2}),
        )),
        Ok(turn("", vec![final_call(false, "done")], json!({}))),
    ]);
    let mut tools = ScriptedTools::default();
    tools.executions.push_back(ToolExecution {
        output: "contents".to_owned(),
        is_error: false,
        sol_budget_exhausted: None,
        slot_reacquire_error: None,
    });
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(
        &mut provider,
        &mut tools,
        input(RunConfig::default()),
        &mut sink,
    );
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert_eq!(
        provider.seen_messages[1],
        vec![
            ConverseMessage::User {
                text: "do work".to_owned()
            },
            ConverseMessage::Assistant {
                text: String::new(),
                tool_calls: vec![call("read_file", json!({"path":"note.txt"}))]
            },
            ConverseMessage::ToolResult {
                tool_call_id: "read_file-id".to_owned(),
                tool_name: "read_file".to_owned(),
                output: "contents".to_owned()
            },
        ]
    );
}

#[test]
fn real_bound_read_observation_is_carried_into_the_next_provider_turn() {
    let root = temp_journal();
    fs::write(root.join("note.txt"), "real contents").unwrap();
    let mut provider = ScriptedProvider::new([
        Ok(turn(
            "",
            vec![call("read_file", json!({"path":"note.txt"}))],
            json!({}),
        )),
        Ok(turn("", vec![final_call(false, "done")], json!({}))),
    ]);
    let mut slot = solstone_core_cogitate_tools::NoopSlotLease;
    let mut tools = CogitateToolExecutor::new(&root, 200, &mut slot);
    let mut sink = RecordingEventSink::default();
    assert_eq!(
        run_cogitate(
            &mut provider,
            &mut tools,
            input(RunConfig::default()),
            &mut sink
        )
        .result
        .as_deref(),
        Some("done")
    );
    assert!(matches!(
        &provider.seen_messages[1][2],
        ConverseMessage::ToolResult { output, .. } if output == "real contents"
    ));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn final_tool_bypasses_armed_ladders() {
    let mut config = RunConfig {
        context_window: Some(1),
        ..RunConfig::default()
    };
    config.expects_emit_final = true;
    let mut provider = ScriptedProvider::new([Ok(turn(
        "",
        vec![final_call(true, "done")],
        json!({"input_tokens": 1}),
    ))]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.reason_code, None);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert!(
        !sink
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::BudgetEscalation { .. }))
    );
}

#[test]
fn unbound_dispatch_refusal_is_byte_exact_and_bound_read_executes() {
    let refusal = solstone_core_cogitate_tools::REFUSAL_TOOL_NOT_BOUND;
    let root = temp_journal();
    fs::write(root.join("note.txt"), "bound contents").unwrap();
    let mut slot = solstone_core_cogitate_tools::NoopSlotLease;
    let mut executor = CogitateToolExecutor::new(&root, 200, &mut slot);
    let diagnostic = RunConfig {
        access_tier: "diagnostic".to_owned(),
        ..RunConfig::default()
    };
    assert!(
        !executor
            .offered_tools(&diagnostic)
            .unwrap()
            .iter()
            .any(|tool| tool.name == "read_file")
    );
    let denied = executor.execute(&diagnostic, &call("read_file", json!({"path":"x"})));
    assert_eq!(denied.output, refusal);
    assert!(denied.is_error);
    let normal = RunConfig::default();
    assert!(
        executor
            .offered_tools(&normal)
            .unwrap()
            .iter()
            .any(|tool| tool.name == "read_file")
    );
    let allowed = executor.execute(&normal, &call("read_file", json!({"path":"note.txt"})));
    assert_eq!(allowed.output, "bound contents");
    assert!(!allowed.is_error);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn offered_schemas_follow_every_tool_argument_spec() {
    let root = temp_journal();
    let mut slot = solstone_core_cogitate_tools::NoopSlotLease;
    let executor = CogitateToolExecutor::new(&root, 200, &mut slot);
    let config = RunConfig::default();
    let mut schemas = executor.offered_tools(&config).unwrap();
    schemas.extend(
        executor
            .offered_tools(&RunConfig {
                expects_emit_final: true,
                ..RunConfig::default()
            })
            .unwrap(),
    );
    for metadata in [
        solstone_core_cogitate_tools::sol_tool(),
        &solstone_core_cogitate_tools::READ_FILE_TOOL,
        &solstone_core_cogitate_tools::LIST_DIRECTORY_TOOL,
        &solstone_core_cogitate_tools::GLOB_TOOL,
        &solstone_core_cogitate_tools::GREP_SEARCH_TOOL,
        &solstone_core_cogitate_tools::EMIT_FINAL_TOOL,
        &solstone_core_cogitate_tools::FINISH_TOOL,
    ] {
        let schema = schemas
            .iter()
            .find(|schema| schema.name == metadata.name)
            .unwrap();
        assert_eq!(schema.parameters["additionalProperties"], false);
        let properties = schema.parameters["properties"].as_object().unwrap();
        let required = schema.parameters["required"].as_array().unwrap();
        let expected_required = metadata
            .arguments
            .iter()
            .filter(|argument| argument.required)
            .map(|argument| argument.name)
            .collect::<Vec<_>>();
        assert_eq!(
            required
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
            expected_required,
            "{} required names",
            metadata.name
        );
        for argument in metadata.arguments {
            assert!(
                properties.contains_key(argument.name),
                "{}:{}",
                metadata.name,
                argument.name
            );
        }
    }
    let glob = schemas.iter().find(|schema| schema.name == "glob").unwrap();
    assert!(glob.parameters["properties"].get("root").is_some());
    let grep = schemas
        .iter()
        .find(|schema| schema.name == "grep_search")
        .unwrap();
    assert!(grep.parameters["properties"].get("path").is_some());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn read_limits_notices_and_grep_context_reach_the_model() {
    let root = temp_journal();
    fs::create_dir_all(root.join("narrow")).unwrap();
    fs::write(
        root.join("lines.txt"),
        (1..=2_001)
            .map(|line| format!("line {line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    fs::write(root.join("narrow/note.txt"), "before\nneedle\nafter\n").unwrap();
    fs::write(root.join("other.txt"), "needle elsewhere\n").unwrap();
    let mut slot = solstone_core_cogitate_tools::NoopSlotLease;
    let mut executor = CogitateToolExecutor::new(&root, 200, &mut slot);
    let config = RunConfig::default();
    let read = executor.execute(
        &config,
        &call("read_file", json!({"path":"lines.txt", "max_lines":1})),
    );
    assert!(
        read.output.contains("line 2"),
        "unadvertised max_lines must not lower the default cap"
    );
    assert!(
        read.output
            .contains(solstone_core_cogitate_tools::NOTICE_READ_FILE_TRUNCATED)
    );
    let grep = executor.execute(
        &config,
        &call(
            "grep_search",
            json!({"pattern":"needle", "path":"narrow", "context_lines":1}),
        ),
    );
    assert!(grep.output.contains("narrow/note.txt:1:before"));
    assert!(grep.output.contains("narrow/note.txt:2:needle"));
    assert!(grep.output.contains("narrow/note.txt:3:after"));
    assert!(
        !grep.output.contains("other.txt"),
        "path must narrow the search"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn tool_binding_setup_failure_is_not_a_slot_failure() {
    let mut provider = ScriptedProvider::default();
    let mut tools = FailingSetupTools;
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(
        &mut provider,
        &mut tools,
        input(RunConfig::default()),
        &mut sink,
    );
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some(TOOL_BINDING_SETUP_FAILED)
    );
    assert_ne!(
        outcome.reason_code.as_deref(),
        Some(crate::SOL_SLOT_REACQUIRE_FAILED)
    );
}

fn temp_journal() -> PathBuf {
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("solstone-runtime-test-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn resource_ladder_uses_frozen_oracle_templates_and_latches_warning() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/cogitate_oracle.json")).unwrap();
    let messages = fixture["budget_escalation"]["messages"].as_array().unwrap();
    let mut ladder = ResourceLadder::default();
    let warning = ladder.check(Some(0.70), "finish").unwrap();
    assert_eq!(
        warning.message.as_deref(),
        Some(
            messages[1]["text"]
                .as_str()
                .unwrap()
                .replace("{finish_tool}", "finish")
                .as_str()
        )
    );
    assert!(ladder.check(Some(0.75), "finish").is_none());
    let final_turn = ladder.check(Some(0.78), "finish").unwrap();
    assert_eq!(
        final_turn.message.as_deref(),
        Some(
            messages[0]["text"]
                .as_str()
                .unwrap()
                .replace("{finish_tool}", "finish")
                .as_str()
        )
    );
    assert_eq!(
        ladder.check(Some(0.78), "finish").unwrap().stage,
        BudgetStage::ForceStopped
    );
}

#[test]
fn turn_ladder_counts_off_by_one_and_dedupes_before_armed_check() {
    let mut ladder = TurnLadder::default();
    for index in 0..58 {
        let _ = ladder.check(&format!("r{index}"), 60, "finish");
    }
    let armed = ladder.check("request-58", 60, "finish").unwrap();
    assert_eq!(armed.stage, BudgetStage::FinalTurn);
    assert_eq!(ladder.observed_turns, 59);
    // Duplicate response ids are a total no-op before arming.
    assert!(ladder.check("request-58", 60, "finish").is_none());
    assert_eq!(ladder.observed_turns, 59);
    assert!(!ladder.force_stopped);
    assert_eq!(
        ladder.check("request-59", 60, "finish").unwrap().stage,
        BudgetStage::ForceStopped
    );
    assert_eq!(ladder.observed_turns, 59);
}

#[test]
fn two_calls_in_one_response_show_resource_turn_dedupe_asymmetry() {
    let mut config = RunConfig {
        context_window: Some(1),
        ..RunConfig::default()
    };
    config.max_turns = 2;
    let response = turn(
        "partial",
        vec![
            call("read_file", json!({"path":"a"})),
            call("read_file", json!({"path":"b"})),
        ],
        json!({"input_tokens": 1}),
    );
    let mut provider = ScriptedProvider::new([Ok(response)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    // Resource checks have no response-id dedupe, while turn checks do.
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("token_budget_exceeded")
    );
    assert_eq!(tools.calls, vec!["read_file"]);
    assert!(sink.events.iter().any(|event| matches!(
        event,
        RuntimeEvent::BudgetEscalation {
            ladder: BudgetLadder::Resource,
            stage: BudgetStage::ForceStopped,
            ..
        }
    )));
    assert!(!sink.events.iter().any(|event| matches!(
        event,
        RuntimeEvent::BudgetEscalation {
            ladder: BudgetLadder::Turn,
            stage: BudgetStage::ForceStopped,
            ..
        }
    )));
}

#[test]
fn turn_warnings_latch_and_ultimatum_suppresses_later_warnings() {
    let mut ladder = TurnLadder::default();
    let mut percentages = Vec::new();
    for index in 0..54 {
        if let Some(event) = ladder.check(&format!("r{index}"), 60, "finish")
            && event.stage == BudgetStage::Warning
        {
            percentages.push(event.message.unwrap());
        }
    }
    assert_eq!(percentages.len(), 3);
    assert!(percentages.iter().any(|message| message.contains("50%")));
    assert!(percentages.iter().any(|message| message.contains("75%")));
    assert!(percentages.iter().any(|message| message.contains("90%")));
    let mut small = TurnLadder::default();
    assert_eq!(
        small.check("one", 2, "finish").unwrap().stage,
        BudgetStage::FinalTurn
    );
    assert!(small.warnings_fired.is_empty());
}

#[test]
fn cumulative_usage_does_not_apply_a_monetary_limit() {
    let mut ladder = ResourceLadder::default();
    let config = RunConfig {
        context_window: Some(100_000),
        ..RunConfig::default()
    };
    let per_turn = Usage {
        input_tokens: 4_000,
        output_tokens: 100_000,
        ..Usage::default()
    };
    let mut accumulated = Usage::default();
    for _ in 0..1_000 {
        accumulated.add_assign(&per_turn);
        assert!(
            ladder
                .check(
                    crate::runtime::context_fraction(&config, &per_turn),
                    "finish"
                )
                .is_none()
        );
    }
    assert_eq!(accumulated.input_tokens, 4_000_000);
    assert!(!ladder.force_stopped);
    assert!(ResourceLadder::default().check(None, "finish").is_none());
}

#[test]
fn stuck_detector_matches_four_live_patterns_and_user_boundary() {
    let mut action_observation = StuckDetector::default();
    for _ in 0..4 {
        action_observation.push(HistoryEntry::Action {
            tool: "x".to_owned(),
            arguments: json!({"a":1}),
        });
        action_observation.push(HistoryEntry::Observation {
            tool: "x".to_owned(),
            output: "ok".to_owned(),
            is_error: false,
        });
    }
    assert!(action_observation.is_stuck());
    let mut action_error = StuckDetector::default();
    for _ in 0..3 {
        action_error.push(HistoryEntry::Action {
            tool: "x".to_owned(),
            arguments: json!({}),
        });
        action_error.push(HistoryEntry::Observation {
            tool: "x".to_owned(),
            output: "no".to_owned(),
            is_error: true,
        });
    }
    assert!(action_error.is_stuck());
    let mut monologue = StuckDetector::default();
    for _ in 0..3 {
        monologue.push(HistoryEntry::AssistantText("same".to_owned()));
    }
    assert!(monologue.is_stuck());
    let mut alternating = StuckDetector::default();
    for index in 0_usize..6 {
        alternating.push(HistoryEntry::Action {
            tool: if index % 2 == 0 { "a" } else { "b" }.to_owned(),
            arguments: json!({}),
        });
        alternating.push(HistoryEntry::Observation {
            tool: if index % 2 == 0 { "a" } else { "b" }.to_owned(),
            output: (index % 2).to_string(),
            is_error: false,
        });
    }
    assert!(alternating.is_stuck());
    action_error.push(HistoryEntry::User);
    assert!(!action_error.is_stuck());
}

#[test]
fn tail_precedence_and_non_responsive_composition_are_preserved() {
    let state = |wall, context, turns, stuck, text: &str| TailState {
        wall_clock_exceeded: wall,
        context_force_stopped: context,
        max_turns_exhausted: turns,
        stuck_or_paused: stuck,
        expects_emit_final: false,
        final_text: Some(text.to_owned()),
        usage: Usage::default(),
        correlation_id: "cid".to_owned(),
    };
    let wall = compose_tail(state(true, true, true, true, "partial"));
    assert_eq!(wall.reason_code.as_deref(), Some("wall_clock_exceeded"));
    assert_eq!(
        wall.error_text.as_deref(),
        Some(
            "wall_clock_exceeded: cogitate run exceeded its wall-clock deadline and was force-finished with a partial result preserved"
        )
    );
    let context = compose_tail(state(false, true, true, true, "partial"));
    assert_eq!(
        context.reason_code.as_deref(),
        Some("token_budget_exceeded")
    );
    assert_eq!(
        context.error_text.as_deref(),
        Some(
            "token_budget_exceeded: cogitate run reached its per-run resource budget and was force-finished with a partial result preserved"
        )
    );
    let turns = compose_tail(state(false, false, true, true, "partial"));
    assert_eq!(turns.reason_code.as_deref(), Some("max_turns_exhausted"));
    assert_eq!(
        turns.error_text.as_deref(),
        Some(
            "max_turns_exhausted: cogitate run reached its turn budget and was force-finished with a partial result preserved"
        )
    );
    let stuck = compose_tail(state(false, false, false, true, "partial"));
    assert_eq!(stuck.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(
        stuck.error_text.as_deref(),
        Some("agent_stuck: cogitate run was interrupted/stuck with a partial result preserved")
    );
    for (tail_state, expected) in [
        (
            state(true, false, false, false, "I cannot do that."),
            "wall_clock_exceeded: cogitate run exceeded its wall-clock deadline after producing the thinking engine didn't answer the request",
        ),
        (
            state(false, true, false, false, "I cannot do that."),
            "token_budget_exceeded: cogitate run reached its per-run resource budget after producing the thinking engine didn't answer the request",
        ),
        (
            state(false, false, false, true, "I cannot do that."),
            "agent_stuck: cogitate run was interrupted/stuck after producing the thinking engine didn't answer the request",
        ),
    ] {
        let outcome = compose_tail(tail_state);
        assert_eq!(outcome.result, None);
        assert_eq!(outcome.error_text.as_deref(), Some(expected));
        assert!(outcome.raw_payload.is_some());
    }
}

#[test]
fn monologues_trip_stuck_and_provider_failures_are_terminal_passthroughs() {
    let mut provider = ScriptedProvider::new([
        Ok(turn("one", vec![], json!({}))),
        Ok(turn("two", vec![], json!({}))),
        Ok(turn("three", vec![], json!({}))),
    ]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    assert_eq!(
        run_cogitate(
            &mut provider,
            &mut tools,
            input(RunConfig::default()),
            &mut sink
        )
        .reason_code
        .as_deref(),
        Some("agent_stuck")
    );
    let failure = ConverseFailure {
        reason_code: "provider_quota_exceeded".to_owned(),
        retryable: true,
        blocking: false,
        detail: None,
    };
    let mut provider = ScriptedProvider::new([Err(failure.clone())]);
    let outcome = run_cogitate(
        &mut provider,
        &mut tools,
        input(RunConfig::default()),
        &mut sink,
    );
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("provider_quota_exceeded")
    );
    assert_eq!(outcome.provider_failure, Some(failure));
    assert!(outcome.terminal);
}

#[test]
fn provider_failure_error_text_prefers_detail_then_reason_code() {
    let with_detail = ConverseFailure {
        reason_code: "provider_unavailable".to_owned(),
        retryable: true,
        blocking: false,
        detail: Some("upstream said distinctive-detail".to_owned()),
    };
    let outcome = RunOutcome::provider_failure(with_detail, Usage::default(), "corr".to_owned());
    assert_eq!(
        outcome.error_text.as_deref(),
        Some("upstream said distinctive-detail")
    );
    assert_eq!(outcome.reason_code.as_deref(), Some("provider_unavailable"));

    let without_detail = ConverseFailure {
        reason_code: "provider_unavailable".to_owned(),
        retryable: true,
        blocking: false,
        detail: None,
    };
    let outcome = RunOutcome::provider_failure(without_detail, Usage::default(), "corr".to_owned());
    assert_eq!(outcome.error_text.as_deref(), Some("provider_unavailable"));
}

#[test]
fn slot_reacquire_other_is_a_distinct_terminal_runtime_outcome() {
    let root = temp_journal();
    let mut lease = FailingLease;
    let mut tools = CogitateToolExecutor::new(&root, 200, &mut lease);
    let mut provider = ScriptedProvider::new([Ok(turn(
        "",
        vec![call(
            "solstone",
            json!({"command":"solstone --runtime-test-invalid"}),
        )],
        json!({}),
    ))]);
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(
        &mut provider,
        &mut tools,
        input(RunConfig::default()),
        &mut sink,
    );
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some(crate::SOL_SLOT_REACQUIRE_FAILED)
    );
    assert_eq!(outcome.error_text.as_deref(), Some("slot disappeared"));
    assert!(outcome.provider_failure.is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn expects_final_no_output_and_usage_shapes_are_normalized() {
    let empty = compose_tail(TailState {
        wall_clock_exceeded: false,
        context_force_stopped: false,
        max_turns_exhausted: false,
        stuck_or_paused: false,
        expects_emit_final: true,
        final_text: None,
        usage: Usage::default(),
        correlation_id: "cid".to_owned(),
    });
    assert_eq!(empty.reason_code.as_deref(), Some("no_output"));
    // Post-parse arm shapes: anthropic.rs:987-991, openai.rs:1141-1144, google.rs:1140-1143.
    let values = [
        json!({"input_tokens":2,"output_tokens":3,"cache_creation_tokens":5,"cached_input_tokens":7,"reasoning_tokens":11}),
        json!({"input_tokens":2,"output_tokens":3,"total_tokens":5,"reasoning_tokens":1,"model_version":"gpt"}),
        json!({"input_tokens":2,"output_tokens":3,"total_tokens":5,"reasoning_tokens":1,"model_version":"gemini"}),
    ];
    let normalized = values.map(|value| Usage::from_turn(&value));
    assert_eq!(normalized[0].cached_tokens, 7);
    assert_eq!(normalized[1].cached_tokens, 0);
    assert_eq!(normalized[2].total_tokens(), 5);
    assert_eq!(normalized[1].input_tokens, normalized[2].input_tokens);
}

#[test]
fn events_include_tool_ladder_and_terminal() {
    let config = RunConfig {
        context_window: Some(1),
        ..RunConfig::default()
    };
    let mut provider = ScriptedProvider::new([Ok(turn(
        "partial",
        vec![
            call("read_file", json!({"path":"x"})),
            call("read_file", json!({"path":"y"})),
        ],
        json!({"input_tokens":1}),
    ))]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let _ = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert!(
        sink.events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ToolStart { .. }))
    );
    assert!(
        sink.events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ToolEnd { .. }))
    );
    assert!(
        sink.events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::BudgetEscalation { .. }))
    );
    assert!(matches!(
        sink.events.last(),
        Some(RuntimeEvent::Terminal { .. })
    ));
}

#[test]
fn truncated_turn_stops_before_tools_or_repeated_completion() {
    for calls in [
        vec![],
        vec![final_call(false, "partial")],
        vec![call(
            "solstone",
            json!({"command":"journal identity partner"}),
        )],
    ] {
        let mut response = turn(
            "<tool_call>incomplete",
            calls,
            json!({"output_tokens":1024}),
        );
        response.turn.finish_reason = "max_tokens".to_owned();
        let mut provider = ScriptedProvider::new([Ok(response)]);
        let mut tools = ScriptedTools::default();
        let mut sink = RecordingEventSink::default();
        let outcome = run_cogitate(
            &mut provider,
            &mut tools,
            input(RunConfig::default()),
            &mut sink,
        );
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some("token_budget_exceeded")
        );
        assert!(outcome.result.is_none());
        assert!(tools.calls.is_empty());
        assert_eq!(provider.seen_messages.len(), 1);
    }
}
