// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only projections of the shared catchup-state envelope.

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_system::catchup::{
    KIND_DAILY_CATCHUP, KIND_SEGMENT_REPAIR, catchup_state_key, catchup_state_path,
    normalized_catchup_entries,
};

use crate::{
    BackoffSummary, SEGMENT_REPAIR_STATUS_DEGRADED, SEGMENT_REPAIR_STATUS_PROGRESSING,
    SEGMENT_REPAIR_STATUS_STUCK, SEGMENT_REPAIR_STATUS_UNKNOWN, SegmentRepairSummary,
};

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
    let value = match std::fs::read(catchup_state_path(journal)) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => value,
            Err(_) => return Some(unknown_summary()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return Some(unknown_summary()),
    };
    let entries = normalized_entries(&value);
    let record = entries
        .get(&catchup_state_key(day, KIND_SEGMENT_REPAIR))
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
    let bytes = std::fs::read(catchup_state_path(journal)).ok()?;
    let value = serde_json::from_slice::<Value>(&bytes).ok()?;
    normalized_entries(&value)
        .remove(&catchup_state_key(day, kind))
        .and_then(|value| value.as_object().cloned())
}

fn normalized_entries(value: &Value) -> Map<String, Value> {
    normalized_catchup_entries(value)
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    use solstone_core_system::catchup::{
        CatchupKind, SegmentRepairOutcome, record_daily_catchup_progress,
        record_segment_repair_attempt, record_segment_repair_outcome,
    };

    use super::{read_segment_repair_attempted, read_segment_repair_summary};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn writers_populate_shipped_repair_and_drain_projections() {
        let root = std::env::temp_dir().join(format!(
            "wave-two-projection-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let segment = root.join("chronicle/20260101/120000_60");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("audio.json"), b"raw").unwrap();
        record_daily_catchup_progress(&root, "20260101", 1, 2);
        record_segment_repair_attempt(&root, "20260101", 1.0);
        assert!(read_segment_repair_attempted(&root, "20260101"));
        record_segment_repair_outcome(
            &root,
            "20260101",
            SegmentRepairOutcome {
                success: false,
                timed_out: true,
                timeout_seconds: Some(3.0),
                ended_at: 4.0,
                cleared: Some(1),
                remaining: Some(2),
            },
        );
        assert_eq!(
            read_segment_repair_summary(&root, "20260101")
                .unwrap()
                .status,
            "progressing"
        );
        assert!(
            !solstone_core_system::catchup::day_eligible_to_drain(
                &root,
                "20260101",
                CatchupKind::SegmentRepair,
                SystemTime::UNIX_EPOCH
            )
            .unwrap()
        );
        let _ = fs::remove_dir_all(root);
    }
}
