// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use solstone_core_cogitate_runtime::events::{BudgetLadder, BudgetStage};
use solstone_core_cogitate_runtime::{
    ConverseProvider, EventSink, RunInput, RunOutcome, RuntimeEvent, ToolExecutor, run_cogitate,
};

use crate::{CogitateRequest, ValidationError, validate_event};

#[derive(Clone, Copy)]
enum NativeEventKind {
    TextDelta,
    Thinking,
    ToolStart,
    ToolEnd,
    ToolBudgetExhausted,
    BudgetEscalation,
    Finish,
    Error,
    DryRun,
}

impl NativeEventKind {
    const ALL: [Self; 9] = [
        Self::TextDelta,
        Self::Thinking,
        Self::ToolStart,
        Self::ToolEnd,
        Self::ToolBudgetExhausted,
        Self::BudgetEscalation,
        Self::Finish,
        Self::Error,
        Self::DryRun,
    ];

    const fn wire_kind(self) -> &'static str {
        match self {
            Self::TextDelta => "text_delta",
            Self::Thinking => "thinking",
            Self::ToolStart => "tool_start",
            Self::ToolEnd => "tool_end",
            Self::ToolBudgetExhausted => "tool_budget_exhausted",
            Self::BudgetEscalation => "budget_escalation",
            Self::Finish => "finish",
            Self::Error => "error",
            Self::DryRun => "dry_run",
        }
    }
}

/// The registered Cortex kinds actually emitted by this native wire adapter.
///
/// The list is derived from the same mapping enum used by the serializer; the
/// Callosum conformance test imports this function rather than maintaining a
/// second hand-copied vocabulary.
pub fn native_producible_kinds() -> &'static [&'static str] {
    static KINDS: OnceLock<Box<[&str]>> = OnceLock::new();
    KINDS.get_or_init(|| {
        NativeEventKind::ALL
            .iter()
            .map(|kind| kind.wire_kind())
            .collect()
    })
}

/// Convert one runtime event into its native Cortex-compatible wire object.
///
/// This is deliberately pure with respect to output transport: callers own
/// stdout framing and flushing. The timestamp is stamped here, at the adapter
/// boundary, rather than in the provider-independent runtime.
pub fn serialize_event(event: RuntimeEvent) -> Value {
    match event {
        RuntimeEvent::TextDelta {
            delta,
            model,
            correlation_id,
        } => event_value(
            NativeEventKind::TextDelta,
            correlation_id,
            [
                ("delta", Value::String(delta)),
                ("model", Value::String(model)),
            ],
        ),
        RuntimeEvent::Reasoning {
            summary,
            payload,
            model,
            correlation_id,
        } => {
            let mut value = event_value(
                NativeEventKind::Thinking,
                correlation_id,
                [
                    ("summary", Value::String(summary)),
                    ("model", Value::String(model)),
                ],
            );
            if let Some(payload) = payload {
                value["payload"] = payload;
            }
            value
        }
        RuntimeEvent::ToolStart {
            call_id,
            tool,
            arguments,
            correlation_id,
        } => event_value(
            NativeEventKind::ToolStart,
            correlation_id,
            [
                ("call_id", Value::String(call_id)),
                ("tool", Value::String(tool)),
                ("args", arguments),
            ],
        ),
        RuntimeEvent::ToolEnd {
            call_id,
            tool,
            arguments,
            result,
            is_error,
            correlation_id,
        } => event_value(
            NativeEventKind::ToolEnd,
            correlation_id,
            [
                ("call_id", Value::String(call_id)),
                ("tool", Value::String(tool)),
                ("args", arguments),
                ("result", Value::String(result)),
                ("is_error", Value::Bool(is_error)),
            ],
        ),
        RuntimeEvent::SolBudgetExhausted {
            budget,
            count,
            correlation_id,
        } => event_value(
            NativeEventKind::ToolBudgetExhausted,
            correlation_id,
            [
                ("tool", Value::String("solstone".to_owned())),
                ("budget", json!(budget)),
                ("count", json!(count)),
            ],
        ),
        RuntimeEvent::BudgetEscalation {
            ladder,
            stage,
            message,
            correlation_id,
        } => event_value(
            NativeEventKind::BudgetEscalation,
            correlation_id,
            [
                ("ladder", Value::String(ladder_name(ladder).to_owned())),
                ("stage", Value::String(stage_name(stage).to_owned())),
                ("message", message.map_or(Value::Null, Value::String)),
            ],
        ),
        RuntimeEvent::Terminal { outcome } => terminal_event(outcome),
    }
}

/// Serialize and validate an event before a stdout adapter writes its NDJSON
/// line. This is the normal emission path; [`serialize_event`] remains public
/// for pure mapping and deliberate malformed-value validator tests.
pub fn serialize_event_validated(event: RuntimeEvent) -> Result<Value, ValidationError> {
    let value = serialize_event(event);
    validate_event(&value)?;
    Ok(value)
}

/// Produce the native dry-run terminal event without constructing a provider.
pub fn serialize_dry_run(request: &CogitateRequest) -> Result<Value, ValidationError> {
    let input = request.to_run_input();
    let value = event_value(
        NativeEventKind::DryRun,
        request.correlation_id.clone(),
        [
            ("dry_run", Value::Bool(true)),
            ("terminal", Value::Bool(true)),
            (
                "rendered_prompt",
                json!({
                    "initial_prompt": input.initial_prompt,
                    "system_instruction": input.system_instruction,
                }),
            ),
            (
                "expects_emit_final",
                Value::Bool(input.config.expects_emit_final),
            ),
        ],
    );
    validate_event(&value)?;
    Ok(value)
}

/// Result of selecting the native dry-run shortcut or executing the runtime.
pub enum NativeRun {
    DryRun(Value),
    Completed(Box<RunOutcome>),
}

/// Execute a prepared request, unless its dry-run marker selects the one-line
/// native shortcut. The latter does not invoke the supplied provider.
///
/// The CLI must still call [`serialize_dry_run`] before constructing its
/// endpoint provider, so dry-run has no endpoint setup side effects at all.
pub fn run_or_dry_run(
    request: &CogitateRequest,
    provider: &mut dyn ConverseProvider,
    tools: &mut dyn ToolExecutor,
    sink: &mut dyn EventSink,
) -> Result<NativeRun, ValidationError> {
    if request.dry_run {
        return serialize_dry_run(request).map(NativeRun::DryRun);
    }
    let input: RunInput = request.to_run_input();
    Ok(NativeRun::Completed(Box::new(run_cogitate(
        provider, tools, input, sink,
    ))))
}

fn event_value<const N: usize>(
    kind: NativeEventKind,
    correlation_id: String,
    fields: [(&str, Value); N],
) -> Value {
    let mut object = Map::from_iter([
        (
            "event".to_owned(),
            Value::String(kind.wire_kind().to_owned()),
        ),
        ("ts".to_owned(), json!(now_ms())),
        ("correlation_id".to_owned(), Value::String(correlation_id)),
    ]);
    object.extend(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value)),
    );
    Value::Object(object)
}

fn terminal_event(outcome: RunOutcome) -> Value {
    let failed = outcome.reason_code.is_some()
        || outcome.error_text.is_some()
        || outcome.provider_failure.is_some();
    let kind = if failed {
        NativeEventKind::Error
    } else {
        NativeEventKind::Finish
    };
    let mut value = event_value(
        kind,
        outcome.correlation_id.clone(),
        [("terminal", Value::Bool(outcome.terminal))],
    );
    let object = value
        .as_object_mut()
        .expect("event_value always returns an object");
    object.insert("usage".to_owned(), outcome.usage.to_wire_value());
    if let Some(result) = outcome.result {
        object.insert("result".to_owned(), Value::String(result));
    }
    if let Some(raw) = outcome.raw_payload {
        object.insert("raw".to_owned(), raw);
    }
    if failed {
        let reason_code = outcome.reason_code.or_else(|| {
            outcome
                .provider_failure
                .as_ref()
                .map(|failure| failure.reason_code.clone())
        });
        let error = outcome
            .error_text
            .or_else(|| reason_code.clone())
            .unwrap_or_else(|| "cogitate run failed".to_owned());
        object.insert("error".to_owned(), Value::String(error));
        if let Some(reason_code) = reason_code {
            object.insert("reason_code".to_owned(), Value::String(reason_code));
        }
        if let Some(failure) = outcome.provider_failure {
            object.insert(
                "provider_failure".to_owned(),
                json!({
                    "reason_code": failure.reason_code,
                    "retryable": failure.retryable,
                    "blocking": failure.blocking,
                }),
            );
        }
    }
    value
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

const fn ladder_name(ladder: BudgetLadder) -> &'static str {
    match ladder {
        BudgetLadder::Resource => "resource",
        BudgetLadder::Turn => "turn",
    }
}

const fn stage_name(stage: BudgetStage) -> &'static str {
    match stage {
        BudgetStage::Warning => "warning",
        BudgetStage::FinalTurn => "final_turn",
        BudgetStage::ForceStopped => "force_stopped",
    }
}
