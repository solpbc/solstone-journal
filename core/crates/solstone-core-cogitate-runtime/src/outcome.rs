// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_generate_wire::{
    ConverseFailure, NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS, classify_output_responsiveness,
};

use crate::Usage;

/// Slot-reacquire failures are local admission errors, outside both the closed
/// cogitate deterministic catalogue and generate-wire's provider vocabulary.
pub const SOL_SLOT_REACQUIRE_FAILED: &str = "sol_slot_reacquire_failed";
/// Tool binding fails before a conversation starts, distinct from a mid-run
/// local slot-reacquire failure.
pub const TOOL_BINDING_SETUP_FAILED: &str = "tool_binding_setup_failed";
pub const NON_RESPONSIVE_REASON_CODE: &str = "non_responsive";

#[derive(Clone, Debug, PartialEq)]
pub struct RunOutcome {
    pub reason_code: Option<String>,
    pub error_text: Option<String>,
    pub result: Option<String>,
    pub usage: Usage,
    pub raw_payload: Option<Value>,
    pub terminal: bool,
    pub correlation_id: String,
    pub provider_failure: Option<ConverseFailure>,
}

impl RunOutcome {
    pub fn clean(result: Option<String>, usage: Usage, correlation_id: String) -> Self {
        Self {
            reason_code: None,
            error_text: None,
            result,
            usage,
            raw_payload: None,
            terminal: true,
            correlation_id,
            provider_failure: None,
        }
    }

    pub fn provider_failure(
        failure: ConverseFailure,
        usage: Usage,
        correlation_id: String,
    ) -> Self {
        Self {
            error_text: Some(
                failure
                    .detail
                    .clone()
                    .unwrap_or_else(|| failure.reason_code.clone()),
            ),
            reason_code: Some(failure.reason_code.clone()),
            result: None,
            usage,
            raw_payload: None,
            terminal: true,
            correlation_id,
            provider_failure: Some(failure),
        }
    }
}

pub(crate) fn non_responsive_payload(output: &str) -> Option<Value> {
    let verdict = classify_output_responsiveness(output);
    verdict.non_responsive.then(|| {
        let mut payload = json!({
            "reason_code": NON_RESPONSIVE_REASON_CODE,
            "non_responsive_output": output.chars().take(NON_RESPONSIVE_RAW_OUTPUT_CAP_CHARS).collect::<String>(),
        });
        if let Some(signal) = verdict.matched_signal {
            payload["non_responsive_matched_signal"] = Value::String(signal.as_log_value().to_owned());
        }
        Value::Array(vec![payload])
    })
}

pub(crate) struct TailState {
    pub wall_clock_exceeded: bool,
    pub context_force_stopped: bool,
    pub max_turns_exhausted: bool,
    pub stuck_or_paused: bool,
    pub expects_emit_final: bool,
    pub final_text: Option<String>,
    pub usage: Usage,
    pub correlation_id: String,
}

/// The terminal ordering preserves the retired runtime's precedence.
/// Classify once, then let each higher-precedence branch retain the same
/// non-responsive result-nullification behavior as the reference.
pub(crate) fn compose_tail(state: TailState) -> RunOutcome {
    let output = state.final_text.unwrap_or_default();
    let raw_payload = (!output.trim().is_empty())
        .then(|| non_responsive_payload(&output))
        .flatten();
    let non_responsive = raw_payload.is_some();
    let partial = !output.trim().is_empty();
    let result = (!non_responsive && partial).then_some(output.clone());
    let (reason_code, error_text) = if state.wall_clock_exceeded {
        (
            "wall_clock_exceeded",
            if non_responsive {
                "wall_clock_exceeded: cogitate run exceeded its wall-clock deadline after producing the thinking engine didn't answer the request"
            } else if partial {
                "wall_clock_exceeded: cogitate run exceeded its wall-clock deadline and was force-finished with a partial result preserved"
            } else {
                "wall_clock_exceeded: cogitate run exceeded its wall-clock deadline and was force-finished before emitting a final result"
            },
        )
    } else if state.context_force_stopped || state.max_turns_exhausted {
        if state.context_force_stopped {
            (
                "token_budget_exceeded",
                if non_responsive {
                    "token_budget_exceeded: cogitate run reached its per-run resource budget after producing the thinking engine didn't answer the request"
                } else if partial {
                    "token_budget_exceeded: cogitate run reached its per-run resource budget and was force-finished with a partial result preserved"
                } else {
                    "token_budget_exceeded: cogitate run reached its per-run resource budget and was force-finished before emitting a final result"
                },
            )
        } else {
            (
                "max_turns_exhausted",
                if non_responsive {
                    "max_turns_exhausted: cogitate run reached its turn budget after producing the thinking engine didn't answer the request"
                } else if partial {
                    "max_turns_exhausted: cogitate run reached its turn budget and was force-finished with a partial result preserved"
                } else {
                    "max_turns_exhausted: cogitate run reached its turn budget and was force-finished before emitting a final result"
                },
            )
        }
    } else if state.stuck_or_paused {
        (
            "agent_stuck",
            if non_responsive {
                "agent_stuck: cogitate run was interrupted/stuck after producing the thinking engine didn't answer the request"
            } else if partial {
                "agent_stuck: cogitate run was interrupted/stuck with a partial result preserved"
            } else {
                "agent_stuck: cogitate run was interrupted/stuck before emitting a final result"
            },
        )
    } else if non_responsive {
        (
            NON_RESPONSIVE_REASON_CODE,
            "non_responsive: cogitate run produced the thinking engine didn't answer the request",
        )
    } else if state.expects_emit_final && !partial {
        (
            "no_output",
            "no_output: expects-final cogitate run finished without emitting a final result",
        )
    } else {
        return RunOutcome::clean(result, state.usage, state.correlation_id);
    };
    RunOutcome {
        reason_code: Some(reason_code.to_owned()),
        error_text: Some(error_text.to_owned()),
        result,
        usage: state.usage,
        raw_payload,
        terminal: true,
        correlation_id: state.correlation_id,
        provider_failure: None,
    }
}
