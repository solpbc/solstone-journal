// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only projections of the shared catchup-state envelope.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::{
    BackoffSummary, SEGMENT_REPAIR_STATUS_DEGRADED, SEGMENT_REPAIR_STATUS_PROGRESSING,
    SEGMENT_REPAIR_STATUS_STUCK, SEGMENT_REPAIR_STATUS_UNKNOWN, SegmentRepairSummary,
};

const KIND_DAILY_CATCHUP: &str = "daily-catchup";
const KIND_SEGMENT_REPAIR: &str = "segment-repair";

/// Read the daily-catchup backoff projection, failing open on state-read errors.
pub fn read_backoff_summary(journal: &Path, day: &str) -> Option<BackoffSummary> {
    let record = shared_record(journal, day, KIND_DAILY_CATCHUP)?;
    record
        .get("entered_backoff_at")
        .filter(|value| !value.is_null())?;
    Some(BackoffSummary {
        backoff_stuck: true,
        attempts: json_usize(record.get("attempts")),
        consecutive_non_completion: json_usize(record.get("consecutive_non_completion")),
        last_outcome: json_string(record.get("last_outcome")).unwrap_or_default(),
        next_retry_at: json_f64(record.get("next_retry_at")),
    })
}

/// Return whether the segment-repair state records at least one attempt.
pub fn read_segment_repair_attempted(journal: &Path, day: &str) -> bool {
    shared_record(journal, day, KIND_SEGMENT_REPAIR)
        .map(|record| json_usize(record.get("attempts")) > 0)
        .unwrap_or(false)
}

/// Read the segment-repair projection.
///
/// This deliberately performs a separate state-file read from the shared
/// fail-open helpers above: unreadable state is surfaced as an unknown repair
/// status, while a fingerprint mismatch (or fingerprint-read error) suppresses
/// the record entirely.
pub fn read_segment_repair_summary(journal: &Path, day: &str) -> Option<SegmentRepairSummary> {
    let value = match fs::read(state_path(journal)) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => value,
            Err(_) => return Some(unknown_summary()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return Some(unknown_summary()),
    };
    let entries = normalized_entries(&value);
    let record = entries
        .get(&key(day, KIND_SEGMENT_REPAIR))
        .and_then(Value::as_object)?;
    let fingerprint = record.get("fingerprint").and_then(Value::as_str)?;
    let Ok(current_fingerprint) =
        solstone_core_system::catchup::read_raw_input_fingerprint(journal, day)
    else {
        return None;
    };
    if current_fingerprint != fingerprint {
        return None;
    }

    let consecutive = json_usize(record.get("consecutive_non_completion"));
    let last_outcome = json_string(record.get("last_outcome"));
    if last_outcome.as_deref() == Some(SEGMENT_REPAIR_STATUS_PROGRESSING) {
        return Some(SegmentRepairSummary {
            status: SEGMENT_REPAIR_STATUS_PROGRESSING.to_owned(),
            attempts: json_usize(record.get("attempts")),
            consecutive_non_completion: 0,
            last_outcome,
            next_retry_at: Some(json_f64(record.get("next_retry_at"))),
            repair_reason_code: json_string(record.get("exit_reason"))
                .filter(|value| !value.is_empty())
                .or_else(|| json_string(record.get("reason_code"))),
            timeout_seconds: json_i64(record.get("timeout_seconds")),
            bounded: record.get("bounded").and_then(Value::as_bool),
            cleared: non_null_value(record.get("cleared")),
            remaining: non_null_value(record.get("remaining")),
        });
    }
    if consecutive == 0 {
        return None;
    }
    let status = if record
        .get("entered_backoff_at")
        .is_some_and(|value| !value.is_null())
    {
        SEGMENT_REPAIR_STATUS_STUCK
    } else {
        SEGMENT_REPAIR_STATUS_DEGRADED
    };
    Some(SegmentRepairSummary {
        status: status.to_owned(),
        attempts: json_usize(record.get("attempts")),
        consecutive_non_completion: consecutive,
        last_outcome,
        next_retry_at: Some(json_f64(record.get("next_retry_at"))),
        repair_reason_code: json_string(record.get("reason_code")),
        timeout_seconds: json_i64(record.get("timeout_seconds")),
        bounded: record.get("bounded").and_then(Value::as_bool),
        cleared: None,
        remaining: None,
    })
}

fn unknown_summary() -> SegmentRepairSummary {
    SegmentRepairSummary {
        status: SEGMENT_REPAIR_STATUS_UNKNOWN.to_owned(),
        attempts: 0,
        consecutive_non_completion: 0,
        last_outcome: None,
        next_retry_at: None,
        repair_reason_code: None,
        timeout_seconds: None,
        bounded: None,
        cleared: None,
        remaining: None,
    }
}

fn shared_record(journal: &Path, day: &str, kind: &str) -> Option<Map<String, Value>> {
    let bytes = fs::read(state_path(journal)).ok()?;
    let value = serde_json::from_slice::<Value>(&bytes).ok()?;
    normalized_entries(&value)
        .remove(&key(day, kind))
        .and_then(|value| value.as_object().cloned())
}

fn state_path(journal: &Path) -> std::path::PathBuf {
    journal.join("health/catchup-state.json")
}

fn key(day: &str, kind: &str) -> String {
    format!("{day}:{kind}")
}

fn normalized_entries(value: &Value) -> Map<String, Value> {
    value
        .as_object()
        .and_then(|value| value.get("entries"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn json_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_bool().map(i64::from))
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn json_usize(value: Option<&Value>) -> usize {
    json_i64(value)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0)
}

fn json_f64(value: Option<&Value>) -> f64 {
    value
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_bool().map(|value| if value { 1.0 } else { 0.0 }))
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .unwrap_or(0.0)
}

fn non_null_value(value: Option<&Value>) -> Option<Value> {
    value.filter(|value| !value.is_null()).cloned()
}
