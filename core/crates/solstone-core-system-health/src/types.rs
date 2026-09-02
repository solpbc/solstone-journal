// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::Value;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklogUnit {
    pub mode: String,
    pub name: String,
    pub facet: Option<String>,
    pub stream: Option<String>,
    pub segment: Option<String>,
    pub why: String,
    pub reason_code: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub trailing_fail_count: usize,
    pub last_fail_ts: Option<i64>,
    pub stuck: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklogError {
    pub day: String,
    pub stage: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackoffSummary {
    pub backoff_stuck: bool,
    pub attempts: usize,
    pub consecutive_non_completion: usize,
    pub last_outcome: String,
    pub next_retry_at: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentRepairSummary {
    pub status: String,
    pub attempts: usize,
    pub consecutive_non_completion: usize,
    pub last_outcome: Option<String>,
    pub next_retry_at: Option<f64>,
    pub repair_reason_code: Option<String>,
    pub timeout_seconds: Option<i64>,
    pub bounded: Option<bool>,
    /// Preserves source-record presence, including JSON false and zero.
    pub cleared: Option<Value>,
    /// Preserves source-record presence, including JSON false and zero.
    pub remaining: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CappedDailyUnit {
    pub name: String,
    pub facet: Option<String>,
    pub reason_code: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CappedDailySummary {
    pub count: usize,
    pub unit: CappedDailyUnit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BacklogDay {
    pub day: String,
    pub state: String,
    pub segments: usize,
    pub units: usize,
    /// Age-gated count of segments with a not-yet-sensed modality past `MODALITY_INPUT_AGED_MS`, not a raw un-aged count.
    pub not_sensed: usize,
    pub why: Vec<BacklogUnit>,
    pub reason: Option<String>,
    pub reason_code: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub error: Option<BacklogError>,
    pub backoff: Option<BackoffSummary>,
    pub segment_repair: Option<SegmentRepairSummary>,
    pub capped_daily: Option<CappedDailySummary>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BacklogView {
    pub window: usize,
    pub days: Vec<BacklogDay>,
    pub pending_days: usize,
    pub stuck_days: usize,
    pub oldest_pending_day: Option<String>,
    pub errors: Vec<BacklogError>,
    pub degraded: bool,
    pub malformed_line_count: usize,
}

fn nonempty(value: &Option<String>) -> Option<&String> {
    value.as_ref().filter(|value| !value.is_empty())
}

impl Serialize for BacklogUnit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("mode", &self.mode)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("facet", &self.facet)?;
        map.serialize_entry("stream", &self.stream)?;
        map.serialize_entry("segment", &self.segment)?;
        map.serialize_entry("why", &self.why)?;
        if let Some(reason_code) = nonempty(&self.reason_code) {
            map.serialize_entry("reason_code", reason_code)?;
        }
        if let Some(provider) = nonempty(&self.provider) {
            map.serialize_entry("provider", provider)?;
        }
        if let Some(model) = nonempty(&self.model) {
            map.serialize_entry("model", model)?;
        }
        map.serialize_entry("trailing_fail_count", &self.trailing_fail_count)?;
        map.serialize_entry("last_fail_ts", &self.last_fail_ts)?;
        map.serialize_entry("stuck", &self.stuck)?;
        map.end()
    }
}

impl Serialize for BacklogError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("day", &self.day)?;
        map.serialize_entry("stage", &self.stage)?;
        map.serialize_entry("message", &self.message)?;
        map.end()
    }
}

impl Serialize for CappedDailyUnit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("facet", &self.facet)?;
        map.serialize_entry("reason_code", &self.reason_code)?;
        map.serialize_entry("count", &self.count)?;
        map.end()
    }
}

impl Serialize for BacklogDay {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("day", &self.day)?;
        map.serialize_entry("state", &self.state)?;
        map.serialize_entry("segments", &self.segments)?;
        map.serialize_entry("units", &self.units)?;
        map.serialize_entry("not_sensed", &self.not_sensed)?;
        map.serialize_entry("reason", &self.reason)?;
        map.serialize_entry("why", &self.why)?;
        map.serialize_entry("error", &self.error)?;
        if let Some(reason_code) = nonempty(&self.reason_code) {
            map.serialize_entry("reason_code", reason_code)?;
        }
        if let Some(provider) = nonempty(&self.provider) {
            map.serialize_entry("provider", provider)?;
        }
        if let Some(model) = nonempty(&self.model) {
            map.serialize_entry("model", model)?;
        }
        if let Some(backoff) = &self.backoff {
            map.serialize_entry("backoff_stuck", &backoff.backoff_stuck)?;
            map.serialize_entry("backoff_attempts", &backoff.attempts)?;
            map.serialize_entry(
                "backoff_consecutive_non_completion",
                &backoff.consecutive_non_completion,
            )?;
            map.serialize_entry("backoff_last_outcome", &backoff.last_outcome)?;
            map.serialize_entry("backoff_next_retry_at", &backoff.next_retry_at)?;
        }
        if let Some(repair) = &self.segment_repair {
            map.serialize_entry("segment_repair_status", &repair.status)?;
            map.serialize_entry("segment_repair_attempts", &repair.attempts)?;
            map.serialize_entry(
                "segment_repair_consecutive_non_completion",
                &repair.consecutive_non_completion,
            )?;
            let repair_last_outcome = repair
                .last_outcome
                .as_ref()
                .filter(|value| !value.is_empty());
            map.serialize_entry("segment_repair_last_outcome", &repair_last_outcome)?;
            map.serialize_entry("segment_repair_next_retry_at", &repair.next_retry_at)?;
            map.serialize_entry("segment_repair_reason_code", &repair.repair_reason_code)?;
            map.serialize_entry("segment_repair_timeout_seconds", &repair.timeout_seconds)?;
            map.serialize_entry("segment_repair_bounded", &repair.bounded)?;
            if let Some(cleared) = &repair.cleared {
                map.serialize_entry("segment_repair_cleared", cleared)?;
            }
            if let Some(remaining) = &repair.remaining {
                map.serialize_entry("segment_repair_remaining", remaining)?;
            }
        }
        if let Some(capped) = &self.capped_daily {
            map.serialize_entry("capped_daily_unit_count", &capped.count)?;
            map.serialize_entry("capped_daily_unit", &capped.unit)?;
        }
        map.end()
    }
}

impl Serialize for BacklogView {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(8))?;
        map.serialize_entry("window", &self.window)?;
        map.serialize_entry("days", &self.days)?;
        map.serialize_entry("pending_days", &self.pending_days)?;
        map.serialize_entry("stuck_days", &self.stuck_days)?;
        map.serialize_entry("oldest_pending_day", &self.oldest_pending_day)?;
        map.serialize_entry("errors", &self.errors)?;
        map.serialize_entry("degraded", &self.degraded)?;
        map.serialize_entry("malformed_line_count", &self.malformed_line_count)?;
        map.end()
    }
}
