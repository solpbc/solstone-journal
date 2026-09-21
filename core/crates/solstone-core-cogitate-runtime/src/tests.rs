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
    execute_delay: Option<Duration>,
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
        if let Some(delay) = self.execute_delay {
            std::thread::sleep(delay);
        }
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
    turn_with_id("response-1", text, calls, usage)
}

fn turn_with_id(
    response_id: &str,
    text: &str,
    calls: Vec<ConverseToolCall>,
    usage: Value,
) -> ProviderResponse {
    ProviderResponse {
        turn: ConverseTurn {
            text: text.to_owned(),
            tool_calls: calls,
            finish_reason: "stop".to_owned(),
            usage,
            model: "test".to_owned(),
            thinking: None,
        },
        response_id: response_id.to_owned(),
    }
}

struct TestLogger;
static LOGGER: TestLogger = TestLogger;
static LOGGER_INIT: std::sync::Once = std::sync::Once::new();
static LOGS: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();

impl log::Log for TestLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            LOGS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
                .lock()
                .expect("warn capture lock")
                .push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

static WARN_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn install_warn_capture() -> std::sync::MutexGuard<'static, ()> {
    let guard = WARN_TEST_MUTEX.lock().unwrap();
    LOGGER_INIT.call_once(|| {
        log::set_logger(&LOGGER).expect("warn capture logger installs");
        log::set_max_level(log::LevelFilter::Warn);
    });
    LOGS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .expect("warn capture lock")
        .clear();
    guard
}

fn captured_warns() -> Vec<String> {
    LOGS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .expect("warn capture lock")
        .clone()
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
                output: "contents".to_owned(),
                is_error: false,
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
fn two_calls_in_one_response_advance_resource_ladder_once() {
    let _guard = install_warn_capture();
    let mut config = RunConfig {
        context_window: Some(1),
        ..RunConfig::default()
    };
    config.max_turns = 10;
    let response1 = turn_with_id(
        "response-1",
        "partial",
        vec![
            call("read_file", json!({"path":"a"})),
            call("read_file", json!({"path":"b"})),
        ],
        json!({"input_tokens": 1}),
    );
    let response2 = turn_with_id(
        "response-2",
        "followup",
        vec![final_call(false, "done")],
        json!({}),
    );
    let mut provider = ScriptedProvider::new([Ok(response1), Ok(response2)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert_eq!(tools.calls, vec!["read_file", "read_file"]);
    let resource_events: Vec<_> = sink
        .events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::BudgetEscalation {
                ladder: BudgetLadder::Resource,
                stage,
                message,
                ..
            } => Some((*stage, message.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(resource_events.len(), 1);
    assert_eq!(resource_events[0].0, BudgetStage::FinalTurn);
    assert!(resource_events[0].1.is_some());
    assert!(!sink.events.iter().any(|event| matches!(
        event,
        RuntimeEvent::BudgetEscalation {
            stage: BudgetStage::ForceStopped,
            ..
        }
    )));
    let next = &provider.seen_messages[1];
    assert_results_follow_calls(next);
    let [.., assistant, first, second, nudge] = next.as_slice() else {
        panic!("expected assistant, two results, resource nudge");
    };
    assert!(
        matches!(assistant, ConverseMessage::Assistant { tool_calls, .. } if tool_calls.len() == 2)
    );
    assert!(matches!(first, ConverseMessage::ToolResult { .. }));
    assert!(matches!(second, ConverseMessage::ToolResult { .. }));
    assert!(matches!(nudge, ConverseMessage::User { text } if text.contains("Resource budget")));
    let logs = captured_warns();
    assert!(
        logs.iter()
            .any(|line| line.contains("nudged cid=cid ladder=resource stage=final_turn"))
    );
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
            "wall_clock_exceeded: cogitate run exceeded its wall-clock deadline after producing the model didn't answer the request",
        ),
        (
            state(false, true, false, false, "I cannot do that."),
            "token_budget_exceeded: cogitate run reached its per-run resource budget after producing the model didn't answer the request",
        ),
        (
            state(false, false, false, true, "I cannot do that."),
            "agent_stuck: cogitate run was interrupted/stuck after producing the model didn't answer the request",
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
    // Three monologues warn once; three more end the run.
    let mut provider = ScriptedProvider::new([
        Ok(turn("one", vec![], json!({}))),
        Ok(turn("two", vec![], json!({}))),
        Ok(turn("three", vec![], json!({}))),
        Ok(turn("four", vec![], json!({}))),
        Ok(turn("five", vec![], json!({}))),
        Ok(turn("six", vec![], json!({}))),
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
    assert_eq!(provider.seen_messages.len(), 6);
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
    let response1 = turn_with_id(
        "response-1",
        "partial",
        vec![
            call("read_file", json!({"path":"x"})),
            call("read_file", json!({"path":"y"})),
        ],
        json!({"input_tokens": 1}),
    );
    let response2 = turn_with_id(
        "response-2",
        "followup",
        vec![final_call(false, "done")],
        json!({}),
    );
    let mut provider = ScriptedProvider::new([Ok(response1), Ok(response2)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert_eq!(
        sink.events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolStart { .. }))
            .count(),
        2
    );
    assert_eq!(
        sink.events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolEnd { .. }))
            .count(),
        2
    );
    let last_tool_end = sink
        .events
        .iter()
        .rposition(|event| matches!(event, RuntimeEvent::ToolEnd { .. }))
        .expect("tool end");
    let first_escalation = sink
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                RuntimeEvent::BudgetEscalation {
                    stage: BudgetStage::FinalTurn,
                    message: Some(_),
                    ..
                }
            )
        })
        .expect("held final-turn publication");
    assert!(first_escalation > last_tool_end);
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

// --- the first stuck trip in a run warns; the second ends it ---------------

fn same_call() -> ConverseToolCall {
    call("read_file", json!({"path":"note.txt"}))
}

fn repeat_turns(count: usize) -> Vec<Result<ProviderResponse, ConverseFailure>> {
    (0..count)
        .map(|_| Ok(turn("", vec![same_call()], json!({}))))
        .collect()
}

fn errored() -> ToolExecution {
    ToolExecution {
        output: "denied".to_owned(),
        is_error: true,
        sol_budget_exhausted: None,
        slot_reacquire_error: None,
    }
}

/// User messages other than the run's initial prompt.
fn warnings(messages: &[ConverseMessage]) -> Vec<&str> {
    messages
        .iter()
        .filter_map(|message| match message {
            ConverseMessage::User { text } if text != "do work" => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn run(
    responses: Vec<Result<ProviderResponse, ConverseFailure>>,
    tools: &mut ScriptedTools,
    config: RunConfig,
) -> (crate::RunOutcome, ScriptedProvider) {
    let mut provider = ScriptedProvider::new(responses);
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, tools, input(config), &mut sink);
    (outcome, provider)
}

fn call_with_id(id: &str, base: ConverseToolCall) -> ConverseToolCall {
    ConverseToolCall {
        id: id.to_owned(),
        ..base
    }
}

/// In a request every assistant message with tool calls is followed, directly and in order,
/// by one result per call.
fn assert_results_follow_calls(request: &[ConverseMessage]) {
    let mut index = 0;
    while index < request.len() {
        if let ConverseMessage::Assistant { tool_calls, .. } = &request[index] {
            for (offset, call) in tool_calls.iter().enumerate() {
                match request.get(index + 1 + offset) {
                    Some(ConverseMessage::ToolResult { tool_call_id, .. }) => {
                        assert_eq!(tool_call_id, &call.id)
                    }
                    other => panic!("call {} has no adjacent result: {other:?}", call.id),
                }
            }
            index += 1 + tool_calls.len();
        } else {
            index += 1;
        }
    }
}

fn run_with_events(
    responses: Vec<Result<ProviderResponse, ConverseFailure>>,
    tools: &mut ScriptedTools,
) -> (crate::RunOutcome, ScriptedProvider, Vec<RuntimeEvent>) {
    let mut provider = ScriptedProvider::new(responses);
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, tools, input(RunConfig::default()), &mut sink);
    (outcome, provider, sink.events)
}

#[test]
fn first_repeat_trip_warns_after_the_result_and_a_changed_turn_can_finish() {
    let mut responses = repeat_turns(4);
    responses.push(Ok(turn("", vec![final_call(false, "done")], json!({}))));
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(responses, &mut tools, RunConfig::default());
    assert_eq!(outcome.reason_code, None);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert_eq!(provider.seen_messages.len(), 5);
    assert_eq!(tools.calls.len(), 4);
    for earlier in &provider.seen_messages[..4] {
        assert!(warnings(earlier).is_empty(), "no warning before the trip");
    }
    let last = provider.seen_messages.last().unwrap();
    assert_eq!(warnings(last).len(), 1);
    assert_results_follow_calls(last);
    let [.., assistant, result, warning] = last.as_slice() else {
        panic!("request too short");
    };
    assert!(matches!(assistant, ConverseMessage::Assistant { .. }));
    assert!(matches!(result, ConverseMessage::ToolResult { .. }));
    assert!(matches!(warning, ConverseMessage::User { .. }));
}

#[test]
fn a_second_repeat_trip_after_the_warning_ends_agent_stuck() {
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(repeat_turns(8), &mut tools, RunConfig::default());
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(provider.seen_messages.len(), 8);
    assert_eq!(tools.calls.len(), 8);
    assert_eq!(warnings(provider.seen_messages.last().unwrap()).len(), 1);
    assert_results_follow_calls(provider.seen_messages.last().unwrap());
}

#[test]
fn three_monologues_warn_once_and_three_more_end_agent_stuck() {
    let responses = (0..6)
        .map(|_| Ok(turn("still thinking", vec![], json!({}))))
        .collect();
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(responses, &mut tools, RunConfig::default());
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(
        outcome.result, None,
        "monologue text is never kept as a result"
    );
    assert_eq!(
        outcome.error_text.as_deref(),
        Some("agent_stuck: cogitate run was interrupted/stuck before emitting a final result")
    );
    assert_eq!(provider.seen_messages.len(), 6);
    assert_eq!(warnings(&provider.seen_messages[2]).len(), 0);
    let after_warning = &provider.seen_messages[3];
    assert_eq!(warnings(after_warning).len(), 1);
    assert!(matches!(
        after_warning.last().unwrap(),
        ConverseMessage::User { .. }
    ));
    assert_eq!(warnings(provider.seen_messages.last().unwrap()).len(), 1);
}

#[test]
fn a_trip_on_the_last_call_of_a_multi_call_turn_warns_after_every_result() {
    // Two single-call turns, then a turn of two identical calls: the fourth action trips on
    // the turn's last call, so both calls run and both results precede the warning.
    let mut responses = repeat_turns(2);
    responses.push(Ok(turn(
        "",
        vec![
            call_with_id("x-0", same_call()),
            call_with_id("x-1", same_call()),
        ],
        json!({}),
    )));
    responses.push(Ok(turn("", vec![final_call(false, "done")], json!({}))));
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(responses, &mut tools, RunConfig::default());
    assert_eq!(outcome.reason_code, None);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert_eq!(tools.calls, vec!["read_file"; 4]);
    let last = provider.seen_messages.last().unwrap();
    assert_results_follow_calls(last);
    let [.., assistant, first, second, warning] = last.as_slice() else {
        panic!("request too short");
    };
    assert!(
        matches!(assistant, ConverseMessage::Assistant { tool_calls, .. } if tool_calls.len() == 2)
    );
    assert!(
        matches!(first, ConverseMessage::ToolResult { tool_call_id, .. } if tool_call_id == "x-0")
    );
    assert!(
        matches!(second, ConverseMessage::ToolResult { tool_call_id, .. } if tool_call_id == "x-1")
    );
    assert!(matches!(warning, ConverseMessage::User { .. }));
}

#[test]
fn a_first_trip_before_the_last_call_of_a_turn_ends_agent_stuck_as_before() {
    // Four identical calls trip the detector at the fourth, which is not the turn's last
    // call: the run ends at once, the calls after the trip (the terminal one included)
    // never run, and no warning is sent.
    let mut calls: Vec<_> = (0..4)
        .map(|index| call_with_id(&format!("x-{index}"), same_call()))
        .collect();
    calls.push(final_call(false, "claims work that never ran"));
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(
        vec![Ok(turn("partial", calls, json!({})))],
        &mut tools,
        RunConfig::default(),
    );
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(outcome.result.as_deref(), Some("partial"));
    assert_eq!(tools.calls, vec!["read_file"; 4]);
    assert_eq!(provider.seen_messages.len(), 1);
    assert!(warnings(&provider.seen_messages[0]).is_empty());
}

#[test]
fn a_second_trip_keeps_the_partial_text_of_the_tripping_turn() {
    let mut responses = repeat_turns(4);
    responses.extend(repeat_turns(3));
    responses.push(Ok(turn("partial", vec![same_call()], json!({}))));
    let mut tools = ScriptedTools::default();
    let (outcome, _) = run(responses, &mut tools, RunConfig::default());
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(outcome.result.as_deref(), Some("partial"));
    assert_eq!(
        outcome.error_text.as_deref(),
        Some("agent_stuck: cogitate run was interrupted/stuck with a partial result preserved")
    );
}

#[test]
fn the_warning_is_per_run_and_shared_by_both_branches() {
    // A differing call after the warning does not re-arm it.
    let mut responses = repeat_turns(4);
    responses.push(Ok(turn("", vec![call("list_dir", json!({}))], json!({}))));
    responses.extend(repeat_turns(4));
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(responses, &mut tools, RunConfig::default());
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(provider.seen_messages.len(), 9);
    assert_eq!(warnings(provider.seen_messages.last().unwrap()).len(), 1);

    // A tool-result warning then three text-only turns.
    let mut responses = repeat_turns(4);
    responses.extend((0..3).map(|_| Ok(turn("thinking", vec![], json!({})))));
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(responses, &mut tools, RunConfig::default());
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(provider.seen_messages.len(), 7);
    assert_eq!(warnings(provider.seen_messages.last().unwrap()).len(), 1);

    // A text-only warning then four identical calls.
    let mut responses: Vec<_> = (0..3)
        .map(|_| Ok(turn("thinking", vec![], json!({}))))
        .collect();
    responses.extend(repeat_turns(4));
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(responses, &mut tools, RunConfig::default());
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(provider.seen_messages.len(), 7);
    assert_eq!(warnings(provider.seen_messages.last().unwrap()).len(), 1);
}

#[test]
fn the_warning_emits_no_events_of_its_own_and_the_warned_run_goes_on_to_finish() {
    // Four identical calls trip and warn; the run then takes one different step and finishes.
    let mut responses = repeat_turns(4);
    responses.push(Ok(turn("", vec![call("list_dir", json!({}))], json!({}))));
    responses.push(Ok(turn("", vec![final_call(false, "done")], json!({}))));
    let mut tools = ScriptedTools::default();
    let (outcome, _, events) = run_with_events(responses, &mut tools);
    assert_eq!(outcome.reason_code, None);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    let starts = events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolStart { .. }))
        .count();
    let ends = events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ToolEnd { .. }))
        .count();
    assert_eq!(
        (starts, ends),
        (5, 5),
        "one pair per dispatched call, including the step taken after the warning"
    );
    // Nothing else: the warning is not a budget event and emits none of its own.
    assert_eq!(events.len(), starts + ends + 1, "{events:?}");
    assert!(matches!(events.last(), Some(RuntimeEvent::Terminal { .. })));
}

#[test]
fn six_alternating_calls_trip_the_alternation_rule_and_warn_once() {
    // A/B/A/B/A/B, each call always returning its own output, is the alternation rule.
    let mut responses: Vec<_> = (0..3)
        .flat_map(|_| {
            [
                Ok(turn("", vec![same_call()], json!({}))),
                Ok(turn("", vec![call("list_dir", json!({}))], json!({}))),
            ]
        })
        .collect();
    responses.push(Ok(turn("", vec![final_call(false, "done")], json!({}))));
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(responses, &mut tools, RunConfig::default());
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert_eq!(provider.seen_messages.len(), 7);
    for earlier in &provider.seen_messages[..6] {
        assert!(warnings(earlier).is_empty());
    }
    assert_eq!(warnings(provider.seen_messages.last().unwrap()).len(), 1);
}

#[test]
fn three_identical_errors_warn_and_three_more_end_agent_stuck() {
    let mut tools = ScriptedTools::default();
    tools.executions.extend((0..6).map(|_| errored()));
    let (outcome, provider) = run(repeat_turns(6), &mut tools, RunConfig::default());
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(provider.seen_messages.len(), 6);
    let after_warning = &provider.seen_messages[3];
    assert_eq!(warnings(after_warning).len(), 1);
    assert!(matches!(
        after_warning.last().unwrap(),
        ConverseMessage::User { .. }
    ));
    assert_eq!(warnings(provider.seen_messages.last().unwrap()).len(), 1);
}

#[test]
fn three_identical_ok_calls_then_a_different_call_never_warn() {
    let mut responses = repeat_turns(3);
    responses.push(Ok(turn("", vec![call("list_dir", json!({}))], json!({}))));
    responses.push(Ok(turn("", vec![final_call(false, "done")], json!({}))));
    let mut tools = ScriptedTools::default();
    let (outcome, provider) = run(responses, &mut tools, RunConfig::default());
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert!(
        provider
            .seen_messages
            .iter()
            .all(|request| warnings(request).is_empty())
    );
}

#[test]
fn each_warning_names_the_terminal_tool_the_run_has_and_the_two_branches_differ() {
    // (expects_emit_final, the terminal tool that must be named, the other tool that must not be told to run)
    for (expects_emit_final, present, wrong_call) in [
        (true, "emit_final", "call finish"),
        (false, "finish", "call emit_final"),
    ] {
        let config = RunConfig {
            expects_emit_final,
            ..RunConfig::default()
        };
        // Tool-result branch.
        let mut responses = repeat_turns(4);
        responses.push(Ok(turn(
            "",
            vec![final_call(expects_emit_final, "done")],
            json!({}),
        )));
        let mut tools = ScriptedTools::default();
        let (_, provider) = run(responses, &mut tools, config.clone());
        let repeat_text = warnings(provider.seen_messages.last().unwrap())[0].to_owned();
        assert!(
            repeat_text.contains(&format!("call {present} now")),
            "{repeat_text}"
        );
        assert!(!repeat_text.contains(wrong_call), "{repeat_text}");
        // Text-only branch.
        let mut responses: Vec<_> = (0..3)
            .map(|_| Ok(turn("thinking", vec![], json!({}))))
            .collect();
        responses.push(Ok(turn(
            "",
            vec![final_call(expects_emit_final, "done")],
            json!({}),
        )));
        let mut tools = ScriptedTools::default();
        let (_, provider) = run(responses, &mut tools, config);
        let text_only = warnings(provider.seen_messages.last().unwrap())[0].to_owned();
        assert!(
            text_only.contains(&format!("call {present} now")),
            "{text_only}"
        );
        assert!(!text_only.contains(wrong_call), "{text_only}");
        assert_ne!(repeat_text, text_only, "each branch says what it saw");
    }
}

#[test]
fn tool_execution_is_error_propagates_to_converse_tool_result_and_runtime_event() {
    let mut provider = ScriptedProvider::new([
        Ok(turn(
            "",
            vec![call("read_file", json!({"path":"secret.txt"}))],
            json!({}),
        )),
        Ok(turn("", vec![final_call(false, "done")], json!({}))),
    ]);
    let mut tools = ScriptedTools::default();
    tools.executions.push_back(crate::tools::ToolExecution {
        output: "permission denied".to_owned(),
        is_error: true,
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
        provider.seen_messages[1][2],
        ConverseMessage::ToolResult {
            tool_call_id: "read_file-id".to_owned(),
            tool_name: "read_file".to_owned(),
            output: "permission denied".to_owned(),
            is_error: true,
        }
    );
    assert!(sink.events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolEnd {
            tool,
            result,
            is_error: true,
            ..
        } if tool == "read_file" && result == "permission denied"
    )));
}

#[test]
fn held_budget_nudge_appended_after_all_tool_results_of_turn() {
    let _guard = install_warn_capture();
    let config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    let response1 = turn_with_id(
        "response-1",
        "working",
        vec![
            call("read_file", json!({"path":"a"})),
            call("read_file", json!({"path":"b"})),
        ],
        json!({"input_tokens": 72}),
    );
    let response2 = turn_with_id(
        "response-2",
        "finishing",
        vec![final_call(false, "done")],
        json!({}),
    );
    let mut provider = ScriptedProvider::new([Ok(response1), Ok(response2)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.result.as_deref(), Some("done"));

    let msgs = &provider.seen_messages[1];
    assert_eq!(msgs.len(), 5);
    assert!(matches!(msgs[0], ConverseMessage::User { .. }));
    assert!(matches!(msgs[1], ConverseMessage::Assistant { .. }));
    assert!(matches!(
        &msgs[2],
        ConverseMessage::ToolResult {
            is_error: false,
            ..
        }
    ));
    assert!(matches!(
        &msgs[3],
        ConverseMessage::ToolResult {
            is_error: false,
            ..
        }
    ));
    assert!(matches!(&msgs[4], ConverseMessage::User { text } if text.contains("Resource budget")));

    let tool_end_indices: Vec<usize> = sink
        .events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| matches!(e, RuntimeEvent::ToolEnd { .. }).then_some(i))
        .collect();
    let escalation_index = sink
        .events
        .iter()
        .position(|e| {
            matches!(
                e,
                RuntimeEvent::BudgetEscalation {
                    stage: BudgetStage::Warning,
                    ..
                }
            )
        })
        .expect("warning escalation event");
    assert_eq!(tool_end_indices.len(), 2);
    assert!(
        escalation_index > tool_end_indices[1],
        "escalation must be emitted after all tool ends"
    );

    let logs = captured_warns();
    assert!(
        logs.iter()
            .any(|l| l.contains("nudged cid=cid ladder=resource stage=warning"))
    );
}

#[test]
fn held_nudge_dropped_if_turn_exits_early() {
    let _guard = install_warn_capture();
    let config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    let response1 = turn_with_id(
        "response-1",
        "working",
        vec![
            call("read_file", json!({"path":"a"})),
            call("read_file", json!({"path":"b"})),
        ],
        json!({"input_tokens": 72}),
    );
    let mut provider = ScriptedProvider::new([Ok(response1)]);
    let mut tools = ScriptedTools::default();
    tools.executions.push_back(crate::tools::ToolExecution {
        output: "err".to_owned(),
        is_error: true,
        sol_budget_exhausted: None,
        slot_reacquire_error: Some("slot broken".to_owned()),
    });
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("sol_slot_reacquire_failed")
    );

    assert!(
        !sink
            .events
            .iter()
            .any(|e| matches!(e, RuntimeEvent::BudgetEscalation { .. }))
    );
    let logs = captured_warns();
    assert!(!logs.iter().any(|l| l.contains("nudged cid=")));
}

#[test]
fn moment_one_force_stop_emits_escalation_without_message_or_log() {
    let _guard = install_warn_capture();
    let config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    let response1 = turn_with_id(
        "response-1",
        "working",
        vec![call("read_file", json!({"path":"a"}))],
        json!({"input_tokens": 80}),
    );
    let response2 = turn_with_id(
        "response-2",
        "still working",
        vec![call("read_file", json!({"path":"b"}))],
        json!({"input_tokens": 80}),
    );
    let mut provider = ScriptedProvider::new([Ok(response1), Ok(response2)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("token_budget_exceeded")
    );

    assert_eq!(tools.calls.len(), 1);

    let force_stopped_event = sink.events.iter().find(|e| {
        matches!(
            e,
            RuntimeEvent::BudgetEscalation {
                ladder: BudgetLadder::Resource,
                stage: BudgetStage::ForceStopped,
                message: None,
                ..
            }
        )
    });
    assert!(force_stopped_event.is_some());

    let logs = captured_warns();
    assert!(
        logs.iter()
            .any(|l| l.contains("nudged cid=cid ladder=resource stage=final_turn"))
    );
    assert!(!logs.iter().any(|l| l.contains("stage=force_stopped")));
}

fn tool_result_after_one_call(is_error: bool, output: &str) -> ConverseMessage {
    let mut provider = ScriptedProvider::new([
        Ok(turn(
            "",
            vec![call("read_file", json!({"path": "note.txt"}))],
            json!({}),
        )),
        Ok(turn("", vec![final_call(false, "done")], json!({}))),
    ]);
    let mut tools = ScriptedTools::default();
    tools.executions.push_back(ToolExecution {
        output: output.to_owned(),
        is_error,
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
    provider.seen_messages[1]
        .iter()
        .find(|message| matches!(message, ConverseMessage::ToolResult { .. }))
        .cloned()
        .expect("tool result carried into the next request")
}

#[test]
fn successful_and_failed_tool_results_differ_only_by_output_and_is_error() {
    let success = tool_result_after_one_call(false, "contents");
    let failure = tool_result_after_one_call(true, "permission denied");
    let ConverseMessage::ToolResult {
        tool_call_id: success_id,
        tool_name: success_name,
        output: success_output,
        is_error: success_error,
    } = success
    else {
        panic!("success twin");
    };
    let ConverseMessage::ToolResult {
        tool_call_id: failure_id,
        tool_name: failure_name,
        output: failure_output,
        is_error: failure_error,
    } = failure
    else {
        panic!("failure twin");
    };
    assert_eq!(success_id, failure_id);
    assert_eq!(success_name, failure_name);
    assert_eq!(success_output, "contents");
    assert_eq!(failure_output, "permission denied");
    assert!(!success_error);
    assert!(failure_error);
    assert_ne!(success_error, failure_error);
}

#[test]
fn turn_threshold_nudge_follows_every_result_of_a_multi_call_turn() {
    let _guard = install_warn_capture();
    let config = RunConfig {
        max_turns: 4,
        ..Default::default()
    };
    let warmup = turn_with_id(
        "r1",
        "",
        vec![call("read_file", json!({"path": "a"}))],
        json!({}),
    );
    let crossing = turn_with_id(
        "r2",
        "",
        vec![
            call("read_file", json!({"path": "b"})),
            call("read_file", json!({"path": "c"})),
        ],
        json!({}),
    );
    let finish = turn_with_id("r3", "", vec![final_call(false, "done")], json!({}));
    let mut provider = ScriptedProvider::new([Ok(warmup), Ok(crossing), Ok(finish)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    let next = &provider.seen_messages[2];
    assert_results_follow_calls(next);
    let [.., assistant, first, second, nudge] = next.as_slice() else {
        panic!("expected assistant, two results, turn nudge");
    };
    assert!(
        matches!(assistant, ConverseMessage::Assistant { tool_calls, .. } if tool_calls.len() == 2)
    );
    assert!(matches!(first, ConverseMessage::ToolResult { .. }));
    assert!(matches!(second, ConverseMessage::ToolResult { .. }));
    assert!(matches!(nudge, ConverseMessage::User { text } if text.contains("Turn budget")));
    let logs = captured_warns();
    assert!(
        logs.iter()
            .any(|line| line.contains("nudged cid=cid ladder=turn stage=warning"))
    );
}

#[test]
fn double_ladder_publishes_resource_then_turn_after_every_result() {
    let _guard = install_warn_capture();
    let mut config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    config.max_turns = 4;
    let warmup = turn_with_id(
        "r1",
        "",
        vec![call("read_file", json!({"path": "a"}))],
        json!({"input_tokens": 1}),
    );
    let crossing = turn_with_id(
        "r2",
        "",
        vec![
            call("read_file", json!({"path": "b"})),
            call("read_file", json!({"path": "c"})),
        ],
        json!({"input_tokens": 72}),
    );
    let finish = turn_with_id("r3", "", vec![final_call(false, "done")], json!({}));
    let mut provider = ScriptedProvider::new([Ok(warmup), Ok(crossing), Ok(finish)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    let next = &provider.seen_messages[2];
    assert_results_follow_calls(next);
    let [.., assistant, first, second, resource, turn_nudge] = next.as_slice() else {
        panic!("expected assistant, two results, resource nudge, turn nudge");
    };
    assert!(
        matches!(assistant, ConverseMessage::Assistant { tool_calls, .. } if tool_calls.len() == 2)
    );
    assert!(matches!(first, ConverseMessage::ToolResult { .. }));
    assert!(matches!(second, ConverseMessage::ToolResult { .. }));
    assert!(matches!(resource, ConverseMessage::User { text } if text.contains("Resource budget")));
    assert!(matches!(turn_nudge, ConverseMessage::User { text } if text.contains("Turn budget")));
    let escalations: Vec<_> = sink
        .events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::BudgetEscalation {
                ladder,
                stage,
                message: Some(_),
                ..
            } => Some((*ladder, *stage)),
            _ => None,
        })
        .collect();
    assert_eq!(
        escalations,
        vec![
            (BudgetLadder::Resource, BudgetStage::Warning),
            (BudgetLadder::Turn, BudgetStage::Warning),
        ]
    );
    let logs = captured_warns();
    let resource_log = logs
        .iter()
        .position(|line| line.contains("nudged cid=cid ladder=resource stage=warning"))
        .expect("resource marker");
    let turn_log = logs
        .iter()
        .position(|line| line.contains("nudged cid=cid ladder=turn stage=warning"))
        .expect("turn marker");
    assert!(resource_log < turn_log);
    assert_eq!(
        logs.iter()
            .filter(|line| line.contains("nudged cid="))
            .count(),
        2
    );
}

#[test]
fn armed_nonterminal_then_finish_force_stops_without_dispatch() {
    let _guard = install_warn_capture();
    let config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    let arm = turn_with_id(
        "r1",
        "",
        vec![call("read_file", json!({"path": "a"}))],
        json!({"input_tokens": 80}),
    );
    let mixed = turn_with_id(
        "r2",
        "",
        vec![
            call("read_file", json!({"path": "b"})),
            final_call(false, "should not finish"),
        ],
        json!({"input_tokens": 80}),
    );
    let mut provider = ScriptedProvider::new([Ok(arm), Ok(mixed)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("token_budget_exceeded")
    );
    assert_eq!(tools.calls, vec!["read_file"]);
    assert_eq!(
        sink.events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolStart { .. }))
            .count(),
        1
    );
    assert!(sink.events.iter().any(|event| matches!(
        event,
        RuntimeEvent::BudgetEscalation {
            ladder: BudgetLadder::Resource,
            stage: BudgetStage::ForceStopped,
            message: None,
            ..
        }
    )));
    let logs = captured_warns();
    assert!(!logs.iter().any(|line| line.contains("stage=force_stopped")));
}

#[test]
fn armed_finish_then_nonterminal_finalizes_without_dispatching_either() {
    let config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    let arm = turn_with_id(
        "r1",
        "",
        vec![call("read_file", json!({"path": "a"}))],
        json!({"input_tokens": 80}),
    );
    let mixed = turn_with_id(
        "r2",
        "",
        vec![
            final_call(false, "done"),
            call("read_file", json!({"path": "b"})),
        ],
        json!({"input_tokens": 80}),
    );
    let mut provider = ScriptedProvider::new([Ok(arm), Ok(mixed)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.reason_code, None);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert_eq!(tools.calls, vec!["read_file"]);
    assert!(!sink.events.iter().any(|event| matches!(
        event,
        RuntimeEvent::BudgetEscalation {
            stage: BudgetStage::ForceStopped,
            ..
        }
    )));
}

#[test]
fn both_armed_ladders_emit_message_less_force_stops_in_resource_then_turn_order() {
    let _guard = install_warn_capture();
    let mut config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    config.max_turns = 2;
    let arm = turn_with_id(
        "r1",
        "",
        vec![
            call("read_file", json!({"path": "a"})),
            call("read_file", json!({"path": "b"})),
        ],
        json!({"input_tokens": 80}),
    );
    let next = turn_with_id(
        "r2",
        "",
        vec![call("read_file", json!({"path": "c"}))],
        json!({"input_tokens": 80}),
    );
    let mut provider = ScriptedProvider::new([Ok(arm), Ok(next)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some("token_budget_exceeded")
    );
    assert_eq!(tools.calls, vec!["read_file", "read_file"]);
    assert_eq!(
        sink.events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolStart { .. }))
            .count(),
        2
    );
    let force_stops: Vec<_> = sink
        .events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::BudgetEscalation {
                ladder,
                stage: BudgetStage::ForceStopped,
                message: None,
                ..
            } => Some(*ladder),
            _ => None,
        })
        .collect();
    assert_eq!(
        force_stops,
        vec![BudgetLadder::Resource, BudgetLadder::Turn]
    );
    let logs = captured_warns();
    assert!(!logs.iter().any(|line| line.contains("stage=force_stopped")));
}

#[test]
fn coincident_budget_nudge_then_stuck_warning_and_second_trip_ends() {
    let _guard = install_warn_capture();
    let config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    let mut responses = vec![
        Ok(turn_with_id(
            "r1",
            "",
            vec![same_call()],
            json!({"input_tokens": 1}),
        )),
        Ok(turn_with_id(
            "r2",
            "",
            vec![same_call()],
            json!({"input_tokens": 1}),
        )),
        Ok(turn_with_id(
            "r3",
            "",
            vec![
                call_with_id("x-0", same_call()),
                call_with_id("x-1", same_call()),
            ],
            json!({"input_tokens": 72}),
        )),
    ];
    responses.extend((4..8).map(|index| {
        Ok(turn_with_id(
            &format!("r{index}"),
            "",
            vec![same_call()],
            json!({"input_tokens": 1}),
        ))
    }));
    let mut provider = ScriptedProvider::new(responses);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    let after_first_trip = &provider.seen_messages[3];
    assert_results_follow_calls(after_first_trip);
    let [.., assistant, first, second, budget, stuck] = after_first_trip.as_slice() else {
        panic!("expected assistant, results, budget nudge, stuck warning");
    };
    assert!(
        matches!(assistant, ConverseMessage::Assistant { tool_calls, .. } if tool_calls.len() == 2)
    );
    assert!(
        matches!(first, ConverseMessage::ToolResult { tool_call_id, .. } if tool_call_id == "x-0")
    );
    assert!(
        matches!(second, ConverseMessage::ToolResult { tool_call_id, .. } if tool_call_id == "x-1")
    );
    assert!(matches!(budget, ConverseMessage::User { text } if text.contains("Resource budget")));
    assert!(matches!(stuck, ConverseMessage::User { text } if text.contains("repeating steps")));
    assert_eq!(
        warnings(after_first_trip)
            .iter()
            .filter(|text| text.contains("Resource budget"))
            .count(),
        1
    );
    assert_eq!(warnings(provider.seen_messages.last().unwrap()).len(), 2);
    let logs = captured_warns();
    assert!(
        logs.iter()
            .any(|line| line.contains("nudged cid=cid ladder=resource stage=warning"))
    );
}

#[test]
fn threshold_crossing_nonterminal_then_finish_drops_held_nudge() {
    let _guard = install_warn_capture();
    let config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    let mixed = turn_with_id(
        "r1",
        "",
        vec![
            call("read_file", json!({"path": "a"})),
            final_call(false, "done"),
        ],
        json!({"input_tokens": 72}),
    );
    let mut provider = ScriptedProvider::new([Ok(mixed)]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.result.as_deref(), Some("done"));
    assert_eq!(tools.calls, vec!["read_file"]);
    assert!(
        !sink
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::BudgetEscalation { .. }))
    );
    assert!(!captured_warns().iter().any(|line| line.contains("nudged")));
}

#[test]
fn deadline_after_first_result_drops_held_nudge() {
    let _guard = install_warn_capture();
    let mut config = RunConfig {
        context_window: Some(100),
        timeout: Duration::from_millis(200),
        ..RunConfig::default()
    };
    config.max_turns = 10;
    let crossing = turn_with_id(
        "r1",
        "",
        vec![
            call("read_file", json!({"path": "a"})),
            call("read_file", json!({"path": "b"})),
        ],
        json!({"input_tokens": 72}),
    );
    let mut provider = ScriptedProvider::new([Ok(crossing)]);
    let mut tools = ScriptedTools {
        execute_delay: Some(Duration::from_millis(150)),
        ..ScriptedTools::default()
    };
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.reason_code.as_deref(), Some("wall_clock_exceeded"));
    assert_eq!(tools.calls, vec!["read_file"]);
    assert!(
        !sink
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::BudgetEscalation { .. }))
    );
    assert!(!captured_warns().iter().any(|line| line.contains("nudged")));
}

#[test]
fn first_stuck_trip_before_last_call_drops_held_nudge() {
    let _guard = install_warn_capture();
    let config = RunConfig {
        context_window: Some(100),
        ..RunConfig::default()
    };
    let mut calls: Vec<_> = (0..4)
        .map(|index| call_with_id(&format!("x-{index}"), same_call()))
        .collect();
    calls.push(final_call(false, "claims work that never ran"));
    let mut provider = ScriptedProvider::new([Ok(turn_with_id(
        "r1",
        "partial",
        calls,
        json!({"input_tokens": 72}),
    ))]);
    let mut tools = ScriptedTools::default();
    let mut sink = RecordingEventSink::default();
    let outcome = run_cogitate(&mut provider, &mut tools, input(config), &mut sink);
    assert_eq!(outcome.reason_code.as_deref(), Some("agent_stuck"));
    assert_eq!(tools.calls, vec!["read_file"; 4]);
    assert!(
        !sink
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::BudgetEscalation { .. }))
    );
    assert!(!captured_warns().iter().any(|line| line.contains("nudged")));
}
