// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Instant;

use solstone_core_generate_wire::{ConverseMessage, ConverseToolCall};

use crate::config::RunInput;
use crate::events::{EventSink, RuntimeEvent};
use crate::ladders::{LadderEvent, ResourceLadder, TurnLadder};
use crate::outcome::{
    RunOutcome, SOL_SLOT_REACQUIRE_FAILED, TOOL_BINDING_SETUP_FAILED, TailState, compose_tail,
};
use crate::provider::ConverseProvider;
use crate::stuck::{HistoryEntry, StuckDetector};
use crate::tools::ToolExecutor;
use crate::usage::Usage;

/// Run a prepared cogitate conversation until it reaches a terminal outcome.
///
/// The native scope has no async provider task to race with a deadline. It
/// consequently checks the deadline cooperatively between turns and tool
/// dispatches; this changes interruption mechanics, not the deadline
/// calculation or terminal meaning.
pub fn run_cogitate(
    provider: &mut dyn ConverseProvider,
    tools: &mut dyn ToolExecutor,
    input: RunInput,
    sink: &mut dyn EventSink,
) -> RunOutcome {
    let config = input.config;
    let offered_tools = match tools.offered_tools(&config) {
        Ok(tools) => tools,
        Err(error) => {
            return terminal(
                sink,
                setup_failure(error, Usage::default(), config.correlation_id),
            );
        }
    };
    // Conversation compaction is intentionally outside this runtime. Messages
    // are never summarized or dropped, so this history accumulates for the full
    // run; only MAX_TURNS (default 60) and the context ladder's stage-3
    // force-stop bound it. This is a deliberate scope cut, to revisit when
    // or later if long runs make that accumulation a practical context
    // problem.
    let mut messages = vec![ConverseMessage::User {
        text: input.initial_prompt.clone(),
    }];
    let mut stuck = StuckDetector::default();
    stuck.push(HistoryEntry::User);
    let mut usage = Usage::default();
    let mut resources = ResourceLadder::default();
    let mut turns = TurnLadder::default();
    let mut final_text = None;
    let started = Instant::now();
    let deadline = config.wall_clock_deadline();

    loop {
        if started.elapsed() >= deadline {
            return terminal(
                sink,
                tail(&config, usage, final_text, true, &resources, &turns, false),
            );
        }
        let remaining = deadline.saturating_sub(started.elapsed());
        let response = match provider.converse(
            &config.model,
            input.system_instruction.as_deref(),
            &messages,
            &offered_tools,
            remaining,
        ) {
            Ok(turn) => turn,
            Err(failure) => {
                // Provider failures are terminal regardless of retryability in this
                // single-run API; retain flags verbatim for a caller to decide retry.
                let outcome =
                    RunOutcome::provider_failure(failure, usage, config.correlation_id.clone());
                return terminal(sink, outcome);
            }
        };
        let turn = response.turn;
        let turn_usage = Usage::from_turn(&turn.usage);
        usage.add_assign(&turn_usage);
        if !turn.text.is_empty() {
            sink.emit(RuntimeEvent::TextDelta {
                delta: turn.text.clone(),
                model: turn.model.clone(),
                correlation_id: config.correlation_id.clone(),
            });
        }
        if let Some(thinking) = &turn.thinking {
            sink.emit(RuntimeEvent::Reasoning {
                summary: thinking.to_string(),
                payload: Some(thinking.clone()),
                model: turn.model.clone(),
                correlation_id: config.correlation_id.clone(),
            });
        }
        // A truncated provider turn is not a complete action or final result.
        // Replaying it as ordinary assistant text repeats the same cut-off
        // submission and conceals the actual resource failure as agent_stuck.
        if turn.finish_reason == "max_tokens" {
            return terminal(sink, RunOutcome {
                reason_code: Some("token_budget_exceeded".to_owned()),
                error_text: Some("token_budget_exceeded: provider exhausted the response token budget before completing its turn".to_owned()),
                result: None,
                usage,
                raw_payload: None,
                terminal: true,
                correlation_id: config.correlation_id.clone(),
                provider_failure: None,
            });
        }
        messages.push(ConverseMessage::Assistant {
            text: turn.text.clone(),
            tool_calls: turn.tool_calls.clone(),
        });
        if turn.tool_calls.is_empty() {
            stuck.push(HistoryEntry::AssistantText(turn.text));
            if stuck.is_stuck() {
                return terminal(
                    sink,
                    tail(&config, usage, final_text, false, &resources, &turns, true),
                );
            }
            // MAX_TURNS_HEADROOM is an SDK iteration-cap backstop. It has no
            // native counterpart because three consecutive pure monologues
            // trip the stuck detector, a tighter bound than max_turns + 2.
            continue;
        }
        for call in &turn.tool_calls {
            if is_final_tool(call, config.expects_emit_final) {
                final_text = Some(final_tool_text(call));
                return terminal(
                    sink,
                    tail(&config, usage, final_text, false, &resources, &turns, false),
                );
            }
            if started.elapsed() >= deadline {
                return terminal(
                    sink,
                    tail(
                        &config,
                        usage,
                        final_text.or(Some(turn.text.clone())),
                        true,
                        &resources,
                        &turns,
                        false,
                    ),
                );
            }
            if let Some(event) = resources.check(
                context_fraction(&config, &turn_usage),
                finish_tool(config.expects_emit_final),
            ) {
                send_ladder_event(
                    &mut messages,
                    &mut stuck,
                    sink,
                    &config.correlation_id,
                    event,
                );
            }
            if let Some(event) = turns.check(
                &response.response_id,
                config.max_turns,
                finish_tool(config.expects_emit_final),
            ) {
                send_ladder_event(
                    &mut messages,
                    &mut stuck,
                    sink,
                    &config.correlation_id,
                    event,
                );
            }
            if resources.force_stopped || turns.force_stopped {
                return terminal(
                    sink,
                    tail(
                        &config,
                        usage,
                        final_text.or(Some(turn.text.clone())),
                        false,
                        &resources,
                        &turns,
                        false,
                    ),
                );
            }
            sink.emit(RuntimeEvent::ToolStart {
                call_id: call.id.clone(),
                tool: call.name.clone(),
                arguments: call.arguments.clone(),
                correlation_id: config.correlation_id.clone(),
            });
            stuck.push(HistoryEntry::Action {
                tool: call.name.clone(),
                arguments: call.arguments.clone(),
            });
            let execution = tools.execute(&config, call);
            sink.emit(RuntimeEvent::ToolEnd {
                call_id: call.id.clone(),
                tool: call.name.clone(),
                arguments: call.arguments.clone(),
                result: execution.output.clone(),
                is_error: execution.is_error,
                correlation_id: config.correlation_id.clone(),
            });
            if let Some((budget, count)) = execution.sol_budget_exhausted {
                sink.emit(RuntimeEvent::SolBudgetExhausted {
                    budget,
                    count,
                    correlation_id: config.correlation_id.clone(),
                });
            }
            messages.push(ConverseMessage::ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                output: execution.output.clone(),
            });
            stuck.push(HistoryEntry::Observation {
                tool: call.name.clone(),
                output: execution.output,
                is_error: execution.is_error,
            });
            if let Some(error) = execution.slot_reacquire_error {
                return terminal(sink, local_failure(error, usage, config.correlation_id));
            }
            if started.elapsed() >= deadline {
                return terminal(
                    sink,
                    tail(
                        &config,
                        usage,
                        final_text.or(Some(turn.text.clone())),
                        true,
                        &resources,
                        &turns,
                        false,
                    ),
                );
            }
            if stuck.is_stuck() {
                return terminal(
                    sink,
                    tail(
                        &config,
                        usage,
                        final_text.or(Some(turn.text.clone())),
                        false,
                        &resources,
                        &turns,
                        true,
                    ),
                );
            }
        }
        if resources.force_stopped || turns.force_stopped {
            return terminal(
                sink,
                tail(
                    &config,
                    usage,
                    final_text.or(Some(turn.text)),
                    false,
                    &resources,
                    &turns,
                    false,
                ),
            );
        }
    }
}

fn send_ladder_event(
    messages: &mut Vec<ConverseMessage>,
    stuck: &mut StuckDetector,
    sink: &mut dyn EventSink,
    correlation_id: &str,
    event: LadderEvent,
) {
    sink.emit(RuntimeEvent::BudgetEscalation {
        ladder: event.ladder,
        stage: event.stage,
        message: event.message.clone(),
        correlation_id: correlation_id.to_owned(),
    });
    if let Some(message) = event.message {
        messages.push(ConverseMessage::User { text: message });
        stuck.push(HistoryEntry::User);
    }
}

fn finish_tool(expects_emit_final: bool) -> &'static str {
    if expects_emit_final {
        "emit_final"
    } else {
        "finish"
    }
}
pub(crate) fn context_fraction(config: &crate::RunConfig, turn_usage: &Usage) -> Option<f64> {
    config
        .context_window
        .filter(|window| *window > 0)
        .map(|window| turn_usage.input_tokens as f64 / window as f64)
}
fn is_final_tool(call: &ConverseToolCall, expects_emit_final: bool) -> bool {
    call.name == finish_tool(expects_emit_final)
}
fn final_tool_text(call: &ConverseToolCall) -> String {
    call.arguments
        .get(if call.name == "emit_final" {
            "content"
        } else {
            "message"
        })
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn tail(
    config: &crate::RunConfig,
    usage: Usage,
    final_text: Option<String>,
    wall_clock_exceeded: bool,
    resources: &ResourceLadder,
    turns: &TurnLadder,
    stuck_or_paused: bool,
) -> RunOutcome {
    compose_tail(TailState {
        wall_clock_exceeded,
        context_force_stopped: resources.force_stopped,
        max_turns_exhausted: turns.force_stopped,
        stuck_or_paused,
        expects_emit_final: config.expects_emit_final,
        final_text,
        usage,
        correlation_id: config.correlation_id.clone(),
    })
}
fn local_failure(error_text: String, usage: Usage, correlation_id: String) -> RunOutcome {
    RunOutcome {
        reason_code: Some(SOL_SLOT_REACQUIRE_FAILED.to_owned()),
        error_text: Some(error_text),
        result: None,
        usage,
        raw_payload: None,
        terminal: true,
        correlation_id,
        provider_failure: None,
    }
}
fn setup_failure(error_text: String, usage: Usage, correlation_id: String) -> RunOutcome {
    RunOutcome {
        reason_code: Some(TOOL_BINDING_SETUP_FAILED.to_owned()),
        error_text: Some(error_text),
        result: None,
        usage,
        raw_payload: None,
        terminal: true,
        correlation_id,
        provider_failure: None,
    }
}
fn terminal(sink: &mut dyn EventSink, outcome: RunOutcome) -> RunOutcome {
    sink.emit(RuntimeEvent::Terminal {
        outcome: outcome.clone(),
    });
    outcome
}
