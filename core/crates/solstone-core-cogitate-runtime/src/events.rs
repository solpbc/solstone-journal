// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use crate::RunOutcome;

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeEvent {
    TextDelta {
        delta: String,
        model: String,
        correlation_id: String,
    },
    Reasoning {
        summary: String,
        payload: Option<Value>,
        model: String,
        correlation_id: String,
    },
    ToolStart {
        call_id: String,
        tool: String,
        arguments: Value,
        correlation_id: String,
    },
    ToolEnd {
        call_id: String,
        tool: String,
        arguments: Value,
        result: String,
        is_error: bool,
        correlation_id: String,
    },
    SolBudgetExhausted {
        budget: i64,
        count: i64,
        correlation_id: String,
    },
    BudgetEscalation {
        ladder: BudgetLadder,
        stage: BudgetStage,
        message: Option<String>,
        correlation_id: String,
    },
    Terminal {
        outcome: RunOutcome,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetLadder {
    Resource,
    Turn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetStage {
    Warning,
    FinalTurn,
    ForceStopped,
}

pub trait EventSink {
    fn emit(&mut self, event: RuntimeEvent);
}

#[derive(Default)]
pub struct NoopEventSink;
impl EventSink for NoopEventSink {
    fn emit(&mut self, _event: RuntimeEvent) {}
}

#[derive(Default)]
pub struct RecordingEventSink {
    pub events: Vec<RuntimeEvent>,
}
impl EventSink for RecordingEventSink {
    fn emit(&mut self, event: RuntimeEvent) {
        self.events.push(event);
    }
}
