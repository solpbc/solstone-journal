// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::Instant;

use solstone_core_generate_wire::{ConverseMessage, ConverseToolCall};

use crate::config::RunInput;
use crate::events::{BudgetLadder, BudgetStage, EventSink, RuntimeEvent};
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
    // The first stuck trip in a run that lands on a text-only turn or on the last
    // call of a turn is answered with a warning; a second trip ends the run. Once
    // per run, not per episode: after being warned a run may repeat consecutively
    // for one more detector window, but it is not bounded in total by this.
    let mut stuck_warned = false;
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
                if stuck_warned {
                    return terminal(
                        sink,
                        tail(&config, usage, final_text, false, &resources, &turns, true),
                    );
                }
                stuck_warned = true;
                push_stuck_warning(
                    &mut messages,
                    &mut stuck,
                    StuckWarning::TextOnly,
                    finish_tool(config.expects_emit_final),
                );
            }
            // MAX_TURNS_HEADROOM is an SDK iteration-cap backstop. It has no
            // native counterpart because six consecutive pure monologues (three,
            // the one warning, three more) end the run through the stuck
            // detector, a tighter bound than max_turns + 2 for any realistic
            // max_turns.
            continue;
        }
        if is_final_tool(&turn.tool_calls[0], config.expects_emit_final) {
            final_text = Some(final_tool_text(&turn.tool_calls[0]));
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
        if resources.final_turn_armed
            && let Some(event) = resources.check(
                context_fraction(&config, &turn_usage),
                finish_tool(config.expects_emit_final),
            )
        {
            sink.emit(RuntimeEvent::BudgetEscalation {
                ladder: event.ladder,
                stage: event.stage,
                message: None,
                correlation_id: config.correlation_id.clone(),
            });
        }
        if turns.final_turn_armed
            && let Some(event) = turns.check(
                &response.response_id,
                config.max_turns,
                finish_tool(config.expects_emit_final),
            )
        {
            sink.emit(RuntimeEvent::BudgetEscalation {
                ladder: event.ladder,
                stage: event.stage,
                message: None,
                correlation_id: config.correlation_id.clone(),
            });
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

        let mut held_ladder_events = Vec::new();
        if let Some(event) = resources.check(
            context_fraction(&config, &turn_usage),
            finish_tool(config.expects_emit_final),
        ) {
            held_ladder_events.push(event);
        }
        if let Some(event) = turns.check(
            &response.response_id,
            config.max_turns,
            finish_tool(config.expects_emit_final),
        ) {
            held_ladder_events.push(event);
        }

        let mut tripped = false;
        let last_call = turn.tool_calls.len() - 1;
        for (index, call) in turn.tool_calls.iter().enumerate() {
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
                is_error: execution.is_error,
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
                if stuck_warned || index != last_call {
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
                tripped = true;
            }
        }
        for event in held_ladder_events {
            publish_ladder_nudge(
                &mut messages,
                &mut stuck,
                sink,
                &config.correlation_id,
                event,
            );
        }
        if tripped {
            stuck_warned = true;
            push_stuck_warning(
                &mut messages,
                &mut stuck,
                StuckWarning::Repeating,
                finish_tool(config.expects_emit_final),
            );
        }
    }
}

fn ladder_str(ladder: BudgetLadder) -> &'static str {
    match ladder {
        BudgetLadder::Resource => "resource",
        BudgetLadder::Turn => "turn",
    }
}

fn stage_str(stage: BudgetStage) -> &'static str {
    match stage {
        BudgetStage::Warning => "warning",
        BudgetStage::FinalTurn => "final_turn",
        BudgetStage::ForceStopped => "force_stopped",
    }
}

fn publish_ladder_nudge(
    messages: &mut Vec<ConverseMessage>,
    stuck: &mut StuckDetector,
    sink: &mut dyn EventSink,
    correlation_id: &str,
    event: LadderEvent,
) {
    let Some(message) = event.message else {
        return;
    };
    messages.push(ConverseMessage::User {
        text: message.clone(),
    });
    stuck.push(HistoryEntry::User);
    sink.emit(RuntimeEvent::BudgetEscalation {
        ladder: event.ladder,
        stage: event.stage,
        message: Some(message),
        correlation_id: correlation_id.to_owned(),
    });
    log::warn!(
        "nudged cid={} ladder={} stage={}",
        correlation_id,
        ladder_str(event.ladder),
        stage_str(event.stage),
    );
}

fn finish_tool(expects_emit_final: bool) -> &'static str {
    if expects_emit_final {
        "emit_final"
    } else {
        "finish"
    }
}

/// Which kind of stuck trip is being warned about.
enum StuckWarning {
    /// Steps repeated.
    Repeating,
    /// Consecutive replies with no tool call.
    TextOnly,
}

/// The one model-facing warning sent on a run's first stuck trip. Like a
/// ladder message it is a user message and resets the detector window; unlike a
/// ladder message it is not a budget event and emits none.
fn push_stuck_warning(
    messages: &mut Vec<ConverseMessage>,
    stuck: &mut StuckDetector,
    warning: StuckWarning,
    finish_tool: &str,
) {
    let text = match warning {
        StuckWarning::Repeating => format!(
            "You are repeating steps without new progress. Do not repeat them. Change the arguments, try a different approach, or call {finish_tool} now with what you have and say what is missing."
        ),
        StuckWarning::TextOnly => format!(
            "You have replied with text several times without taking a step, and plain text is not accepted as a result. Take a step with a tool, or call {finish_tool} now with what you have and say what is missing."
        ),
    };
    messages.push(ConverseMessage::User { text });
    stuck.push(HistoryEntry::User);
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
