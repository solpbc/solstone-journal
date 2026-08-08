// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoldRead<T> {
    pub value: T,
    pub malformed_line_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TerminalUnit {
    pub mode: String,
    pub name: String,
    pub facet: Option<String>,
    pub stream: Option<String>,
    pub segment: Option<String>,
    pub activity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalState {
    pub latest_event: TerminalEvent,
    pub latest_ts: i64,
    pub last_real_complete_ts: Option<i64>,
    pub trailing_fail_count: usize,
    pub deterministic_fail_count: usize,
    pub last_fail_ts: Option<i64>,
    pub use_id: Option<String>,
    pub state: Option<String>,
    pub reason_code: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub oldest_trailing_fail_ts: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEvent {
    Complete,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompletedUnit {
    pub mode: String,
    pub name: String,
    pub facet: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionsSince {
    pub segments: Vec<CompletionSegment>,
    pub activities: Vec<CompletionActivity>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompletionSegment {
    pub stream: Option<String>,
    pub segment: String,
    pub ts: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompletionActivity {
    pub facet: Option<String>,
    pub activity: String,
    pub ts: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DailyUnit {
    pub name: String,
    pub facet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeterministicFailure {
    pub count: usize,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SegmentIdentity {
    pub stream: Option<String>,
    pub segment: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentProgress {
    pub sensed: bool,
    pub density: Option<String>,
    pub change_class: Option<String>,
    pub dispatched: BTreeSet<String>,
    pub completed: BTreeSet<String>,
    pub unconfigured: BTreeSet<String>,
    pub capped_by_skip: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataStateMap(pub BTreeMap<String, String>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentInput {
    pub key: String,
    pub stream: String,
    pub data_state: DataStateMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThoughtVerdict {
    Complete,
    NoSenseComplete,
    Floor(String),
    Dispatched(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentCompletion {
    pub blockers: Vec<SegmentBlocker>,
    pub not_sensed: usize,
    pub not_thought: usize,
    pub total: usize,
    pub capped: usize,
    pub exhausted: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentBlocker {
    pub segment: String,
    pub dimension: SegmentBlockerDimension,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentBlockerDimension {
    NotSensed,
    NotThought,
}
