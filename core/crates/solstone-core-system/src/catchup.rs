// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Catchup-state reads and writer-owned progress records shared by native tasks.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use solstone_core_journal_io::{
    HealthMarkerError, HealthMarkerKind, HealthMarkerState, day_marker_pair_status,
    read_health_marker,
};
use solstone_core_journal_io::{
    JsonWriteOptions, LockOptions, PathError, PathOrDay, day_dirs, hold_lock, iter_segments,
    write_json,
};
use solstone_core_processing_record::{expected_handler, read_processing_record_header, vocab};
use thiserror::Error;

pub const MAX_UPDATED_CATCHUP: usize = 4;
pub const CATCHUP_STATE_VERSION: u64 = 1;
pub const KIND_DAILY_CATCHUP: &str = "daily-catchup";
pub const KIND_SEGMENT_REPAIR: &str = "segment-repair";
const SEGMENT_REPAIR_AT_ADMISSION: &str = "segment_repair_at_admission";

const RAW_HASHED_NAMES: [&str; 5] = [
    "audio.json",
    "audio.jsonl",
    "chat.jsonl",
    "screen.jsonl",
    "conversation_transcript.jsonl",
];
const RAW_HASHED_SUFFIXES: [&str; 3] = ["_audio.jsonl", "_screen.jsonl", "_transcript.md"];
const MEDIA_EXTENSIONS: [&str; 17] = [
    ".flac", ".opus", ".ogg", ".m4a", ".mp3", ".wav", ".webm", ".mp4", ".mov", ".png", ".jpg",
    ".jpeg", ".heic", ".heif", ".gif", ".webp", ".tiff",
];
const PDF_EXTENSIONS: [&str; 1] = [".pdf"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchupKind {
    DailyCatchup,
    SegmentRepair,
}

impl CatchupKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DailyCatchup => KIND_DAILY_CATCHUP,
            Self::SegmentRepair => KIND_SEGMENT_REPAIR,
        }
    }
}

/// The shared catchup-state envelope is a versioned `entries` object keyed by
/// `"{day}:{kind}"`.  Readers intentionally retain their field-by-field
/// leniency; this definition supplies only the container, keys, and writers.
pub fn catchup_state_path(journal: &Path) -> PathBuf {
    journal.join("health/catchup-state.json")
}

pub fn catchup_state_key(day: &str, kind: &str) -> String {
    format!("{day}:{kind}")
}

pub fn normalized_catchup_entries(value: &Value) -> Map<String, Value> {
    catchup_entries(value).cloned().unwrap_or_default()
}

fn catchup_entries(value: &Value) -> Option<&Map<String, Value>> {
    value
        .as_object()
        .and_then(|object| object.get("entries"))
        .and_then(Value::as_object)
}

fn strict_catchup_entries(value: &Value) -> Result<Map<String, Value>, CatchupError> {
    catchup_entries(value)
        .cloned()
        .ok_or_else(|| CatchupError::State("entries is not an object".to_owned()))
}

fn empty_catchup_state() -> Value {
    json!({"version": CATCHUP_STATE_VERSION, "entries": {}})
}

fn read_catchup_state(journal: &Path) -> Value {
    let path = catchup_state_path(journal);
    match fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => {
                json!({"version": CATCHUP_STATE_VERSION, "entries": normalized_catchup_entries(&value)})
            }
            Err(error) => {
                eprintln!("failed to read catchup state: {error}");
                empty_catchup_state()
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => empty_catchup_state(),
        Err(error) => {
            eprintln!("failed to read catchup state {}: {error}", path.display());
            empty_catchup_state()
        }
    }
}

fn record_from_entry(entry: Option<&Value>, day: &str, kind: &str) -> Map<String, Value> {
    let mut record = Map::from_iter([
        ("day".to_owned(), json!(day)),
        ("command_kind".to_owned(), json!(kind)),
        ("attempts".to_owned(), json!(0)),
        ("consecutive_non_completion".to_owned(), json!(0)),
        ("last_attempt_at".to_owned(), json!(0)),
        ("last_outcome".to_owned(), json!("")),
        ("next_retry_at".to_owned(), json!(0)),
        ("entered_backoff_at".to_owned(), Value::Null),
        ("notified_at".to_owned(), Value::Null),
        ("fingerprint".to_owned(), Value::Null),
        ("active".to_owned(), Value::Null),
        ("reason_code".to_owned(), Value::Null),
        ("timeout_seconds".to_owned(), Value::Null),
        ("bounded".to_owned(), Value::Null),
        ("cleared".to_owned(), Value::Null),
        ("remaining".to_owned(), Value::Null),
        ("exit_reason".to_owned(), Value::Null),
        ("daily_progress".to_owned(), Value::Null),
    ]);
    if let Some(existing) = entry.and_then(Value::as_object) {
        record.extend(existing.clone());
    }
    record.insert("day".to_owned(), json!(day));
    record.insert("command_kind".to_owned(), json!(kind));
    record
}

fn as_usize(value: Option<&Value>) -> usize {
    value.and_then(Value::as_u64).unwrap_or(0) as usize
}

fn reset_after_fingerprint_change(record: &mut Map<String, Value>, reset_attempts: bool) {
    for field in ["consecutive_non_completion", "next_retry_at"] {
        record.insert(field.to_owned(), json!(0));
    }
    if reset_attempts {
        record.insert("attempts".to_owned(), json!(0));
    }
    for field in [
        "entered_backoff_at",
        "notified_at",
        "reason_code",
        "timeout_seconds",
        "bounded",
        "cleared",
        "remaining",
        "exit_reason",
        "daily_progress",
    ] {
        record.insert(field.to_owned(), Value::Null);
    }
    record.insert("last_outcome".to_owned(), json!(""));
}

fn apply_no_progress_backoff(record: &mut Map<String, Value>, ended_at: f64) {
    record.insert("cleared".to_owned(), Value::Null);
    record.insert("remaining".to_owned(), Value::Null);
    record.insert("exit_reason".to_owned(), Value::Null);
    let consecutive = as_usize(record.get("consecutive_non_completion")) + 1;
    record.insert("consecutive_non_completion".to_owned(), json!(consecutive));
    record.insert(
        "next_retry_at".to_owned(),
        json!(
            ended_at
                + (600_u64
                    .saturating_mul(2_u64.saturating_pow((consecutive.saturating_sub(1)) as u32))
                    .min(86_400) as f64)
        ),
    );
    if consecutive >= 3 && record.get("entered_backoff_at").is_none_or(Value::is_null) {
        record.insert("entered_backoff_at".to_owned(), json!(ended_at));
        record.insert("notified_at".to_owned(), json!(ended_at));
    }
}

#[cfg(unix)]
fn marker_generation(journal: &Path, day: &str, kind: HealthMarkerKind) -> u64 {
    versioned_generation(journal, day, kind).unwrap_or(0)
}

#[cfg(unix)]
fn versioned_generation(journal: &Path, day: &str, kind: HealthMarkerKind) -> Option<u64> {
    match read_health_marker(journal, day, kind) {
        Ok(HealthMarkerState::Versioned { marker, .. }) => Some(marker.generation),
        Ok(
            HealthMarkerState::Absent
            | HealthMarkerState::LegacyEmpty { .. }
            | HealthMarkerState::MalformedNonEmpty { .. },
        ) => None,
        Err(error) => {
            eprintln!("failed to read {kind:?} health marker for {day}: {error}");
            None
        }
    }
}

#[cfg(unix)]
fn daily_marker_proves_attempt(
    journal: &Path,
    day: &str,
    admitted_generation: u64,
    fingerprint: &str,
) -> bool {
    matches!(
        read_health_marker(journal, day, HealthMarkerKind::Daily),
        Ok(HealthMarkerState::Versioned { marker, .. })
            if marker.generation == admitted_generation
                && marker.fingerprint.as_deref() == Some(fingerprint)
    )
}

fn prune(entries: &mut Map<String, Value>, journal: &Path) {
    let days = match day_dirs(journal) {
        Ok(days) => days,
        Err(error) => {
            eprintln!("failed to prune catchup state: {error}");
            return;
        }
    };
    let Some(newest) = days.keys().max() else {
        return;
    };
    let Ok(date) = chrono::NaiveDate::parse_from_str(newest, "%Y%m%d") else {
        return;
    };
    let cutoff = (date - chrono::Duration::days(30))
        .format("%Y%m%d")
        .to_string();
    entries.retain(|_, value| {
        let Some(record) = value.as_object() else {
            return true;
        };
        if record
            .get("day")
            .and_then(Value::as_str)
            .unwrap_or_default()
            >= cutoff.as_str()
            || record.get("active").is_some_and(|value| !value.is_null())
        {
            return true;
        }
        let completed = record.get("last_outcome").and_then(Value::as_str) == Some("completed");
        let cleared = as_usize(record.get("consecutive_non_completion")) == 0
            && record
                .get("next_retry_at")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                == 0.0
            && record.get("entered_backoff_at").is_none_or(Value::is_null);
        !(completed || cleared)
    });
}

fn update_catchup_state(
    journal: &Path,
    prune_records: bool,
    update: impl FnOnce(&mut Map<String, Value>) -> bool,
) {
    // The journal path is a parameter; the reference's ambient journal-root
    // creation remains at the caller.
    let path = catchup_state_path(journal);
    let Ok(_lock) = hold_lock(&path, LockOptions::default()) else {
        eprintln!("failed to lock catchup state");
        return;
    };
    let mut state = read_catchup_state(journal);
    let Some(entries) = state.get_mut("entries").and_then(Value::as_object_mut) else {
        return;
    };
    if !update(entries) {
        return;
    }
    if prune_records {
        prune(entries, journal);
    }
    if write_json(path, &state, JsonWriteOptions::default()).is_err() {
        eprintln!("failed to write catchup state");
    }
}

/// Record daily progress.  Unlike repair records, this deliberately never prunes.
pub fn record_daily_catchup_progress(journal: &Path, day: &str, cleared: usize, remaining: usize) {
    update_catchup_state(journal, false, |entries| {
        let key = catchup_state_key(day, KIND_DAILY_CATCHUP);
        let mut record = record_from_entry(entries.get(&key), day, KIND_DAILY_CATCHUP);
        record.insert(
            "daily_progress".to_owned(),
            json!({"cleared": cleared, "remaining": remaining}),
        );
        entries.insert(key, Value::Object(record));
        true
    });
}

/// Record an automatic whole-day catchup dispatch before its worker can start.
pub fn record_daily_catchup_attempt(
    journal: &Path,
    day: &str,
    reference: &str,
    started_at: f64,
    admitted_generation: u64,
    fingerprint: &str,
) {
    update_catchup_state(journal, false, |entries| {
        let key = catchup_state_key(day, KIND_DAILY_CATCHUP);
        let mut record = record_from_entry(entries.get(&key), day, KIND_DAILY_CATCHUP);
        if record.get("fingerprint").and_then(Value::as_str) != Some(fingerprint)
            || record.get("admitted_generation").and_then(Value::as_u64)
                != Some(admitted_generation)
        {
            reset_after_fingerprint_change(&mut record, true);
        }
        let attempts = as_usize(record.get("attempts")) + 1;
        record.insert("fingerprint".to_owned(), json!(fingerprint));
        record.insert("attempts".to_owned(), json!(attempts));
        record.insert("last_attempt_at".to_owned(), json!(started_at));
        record.insert("admitted_generation".to_owned(), json!(admitted_generation));
        // Progress belongs to one primary attempt.  A later task must not
        // inherit a previous task's measured output merely because its raw
        // input fingerprint did not change.
        record.insert("daily_progress".to_owned(), Value::Null);
        record.insert("exit_code".to_owned(), Value::Null);
        record.insert("exit_status".to_owned(), Value::Null);
        // Only an inactive repair already present at admission can be settled
        // by this daily attempt. Comparing its complete record at completion
        // preserves repairs that overlap or begin while the daily task runs.
        let prior_repair = entries
            .get(&catchup_state_key(day, KIND_SEGMENT_REPAIR))
            .filter(|repair| {
                repair.get("active").is_some_and(Value::is_null)
                    && repair.get("fingerprint").and_then(Value::as_str) == Some(fingerprint)
            })
            .cloned()
            .unwrap_or(Value::Null);
        record.insert(SEGMENT_REPAIR_AT_ADMISSION.to_owned(), prior_repair);
        record.insert(
            "active".to_owned(),
            json!({"ref": reference, "started_at": started_at}),
        );
        entries.insert(key, Value::Object(record));
        true
    });
}

#[cfg(unix)]
fn settled_segment_repair_key(
    entries: &Map<String, Value>,
    day: &str,
    daily: &Map<String, Value>,
) -> Option<String> {
    let prior = daily
        .get(SEGMENT_REPAIR_AT_ADMISSION)
        .filter(|value| !value.is_null())?;
    let key = catchup_state_key(day, KIND_SEGMENT_REPAIR);
    (entries.get(&key) == Some(prior)).then_some(key)
}

/// The stream/raw tuple sampled when a catchup task becomes the queue primary.
#[derive(Debug, Clone)]
pub struct DailyCatchupAdmission {
    pub generation: u64,
    pub fingerprint: String,
}

/// Sample and persist one catchup admission synchronously at primary dispatch.
pub fn admit_daily_catchup(
    journal: &Path,
    day: &str,
    reference: &str,
    started_at: f64,
) -> Result<DailyCatchupAdmission, CatchupError> {
    admit_daily_catchup_with_capability(
        journal,
        day,
        reference,
        started_at,
        catchup_marker_capability,
    )
}

pub(crate) fn admit_daily_catchup_with_capability<Capability>(
    journal: &Path,
    day: &str,
    reference: &str,
    started_at: f64,
    capability: Capability,
) -> Result<DailyCatchupAdmission, CatchupError>
where
    Capability: FnOnce() -> Result<(), CatchupError>,
{
    capability()?;
    #[cfg(not(unix))]
    {
        let _ = (journal, day, reference, started_at);
        return Err(CatchupError::CapabilityUnavailable);
    }
    #[cfg(unix)]
    {
        let generation = match read_health_marker(journal, day, HealthMarkerKind::Stream)? {
            HealthMarkerState::Versioned { marker, .. } => marker.generation,
            HealthMarkerState::Absent | HealthMarkerState::LegacyEmpty { .. } => 0,
            HealthMarkerState::MalformedNonEmpty { .. } => {
                return Err(CatchupError::State(format!(
                    "stream health marker for {day} is malformed"
                )));
            }
        };
        let fingerprint = read_raw_input_fingerprint(journal, day)?;
        record_daily_catchup_attempt(
            journal,
            day,
            reference,
            started_at,
            generation,
            &fingerprint,
        );
        Ok(DailyCatchupAdmission {
            generation,
            fingerprint,
        })
    }
}

/// Persist a fail-safe terminal result when primary admission cannot be sampled.
pub fn record_daily_catchup_admission_failure(journal: &Path, day: &str, started_at: f64) {
    update_catchup_state(journal, false, |entries| {
        let key = catchup_state_key(day, KIND_DAILY_CATCHUP);
        let mut record = record_from_entry(entries.get(&key), day, KIND_DAILY_CATCHUP);
        let attempts = as_usize(record.get("attempts")) + 1;
        record.insert("attempts".to_owned(), json!(attempts));
        record.insert("last_attempt_at".to_owned(), json!(started_at));
        record.insert("active".to_owned(), Value::Null);
        record.insert("admitted_generation".to_owned(), Value::Null);
        record.insert("daily_progress".to_owned(), Value::Null);
        record.remove(SEGMENT_REPAIR_AT_ADMISSION);
        record.insert("exit_code".to_owned(), json!(-1));
        record.insert("exit_status".to_owned(), json!("error"));
        record.insert("last_outcome".to_owned(), json!("error"));
        record.insert("reason_code".to_owned(), json!("admission_unreadable"));
        record.insert("timeout_seconds".to_owned(), Value::Null);
        record.insert("bounded".to_owned(), json!(false));
        apply_no_progress_backoff(&mut record, started_at);
        entries.insert(key, Value::Object(record));
        true
    });
}

/// The process-level terminal data associated with one daily-catchup dispatch.
#[derive(Debug, Clone)]
pub struct DailyCatchupOutcome {
    pub success: bool,
    pub timed_out: bool,
    pub timeout_seconds: Option<f64>,
    pub ended_at: f64,
    pub exit_code: i32,
    pub exit_status: String,
}

/// Record a daily-catchup completion using marker generation as durable proof.
pub fn record_daily_catchup_outcome(
    journal: &Path,
    day: &str,
    reference: &str,
    admitted_generation: u64,
    fingerprint: &str,
    outcome: DailyCatchupOutcome,
) -> Result<bool, CatchupError> {
    record_daily_catchup_outcome_with_capability(
        journal,
        day,
        reference,
        admitted_generation,
        fingerprint,
        outcome,
        catchup_marker_capability,
    )
}

pub(crate) fn record_daily_catchup_outcome_with_capability<Capability>(
    journal: &Path,
    day: &str,
    reference: &str,
    admitted_generation: u64,
    fingerprint: &str,
    outcome: DailyCatchupOutcome,
    capability: Capability,
) -> Result<bool, CatchupError>
where
    Capability: FnOnce() -> Result<(), CatchupError>,
{
    capability()?;
    #[cfg(not(unix))]
    {
        let _ = (
            journal,
            day,
            reference,
            admitted_generation,
            fingerprint,
            outcome,
        );
        return Err(CatchupError::CapabilityUnavailable);
    }
    #[cfg(unix)]
    {
        let stream_generation = marker_generation(journal, day, HealthMarkerKind::Stream);
        let current_fingerprint = read_raw_input_fingerprint(journal, day);
        let daily_completed = current_fingerprint.as_ref().is_ok_and(|current| {
            current == fingerprint
                && daily_marker_proves_attempt(journal, day, admitted_generation, fingerprint)
        });
        let mut recorded = false;
        update_catchup_state(journal, false, |entries| {
            let key = catchup_state_key(day, KIND_DAILY_CATCHUP);
            let mut record = record_from_entry(entries.get(&key), day, KIND_DAILY_CATCHUP);
            if record.get("fingerprint").and_then(Value::as_str) != Some(fingerprint)
                || record.get("admitted_generation").and_then(Value::as_u64)
                    != Some(admitted_generation)
                || record
                    .get("active")
                    .and_then(Value::as_object)
                    .and_then(|active| active.get("ref"))
                    .and_then(Value::as_str)
                    != Some(reference)
            {
                return false;
            }
            record.insert("active".to_owned(), Value::Null);
            record.insert("admitted_generation".to_owned(), json!(admitted_generation));
            record.insert("exit_code".to_owned(), json!(outcome.exit_code));
            record.insert("exit_status".to_owned(), json!(outcome.exit_status));

            if daily_completed {
                record.insert("attempts".to_owned(), json!(0));
                record.insert("consecutive_non_completion".to_owned(), json!(0));
                record.insert("next_retry_at".to_owned(), json!(0));
                record.insert("entered_backoff_at".to_owned(), Value::Null);
                record.insert("notified_at".to_owned(), Value::Null);
                record.insert("reason_code".to_owned(), Value::Null);
                record.insert("timeout_seconds".to_owned(), Value::Null);
                record.insert("bounded".to_owned(), json!(false));
                record.insert("last_outcome".to_owned(), json!("completed"));
            } else if current_fingerprint.is_err() {
                record.insert("last_outcome".to_owned(), json!("error"));
                record.insert("reason_code".to_owned(), json!("fingerprint_unreadable"));
                record.insert("timeout_seconds".to_owned(), Value::Null);
                record.insert("bounded".to_owned(), json!(false));
                apply_no_progress_backoff(&mut record, outcome.ended_at);
            } else if stream_generation > admitted_generation
                || current_fingerprint
                    .as_ref()
                    .is_ok_and(|current| current != fingerprint)
            {
                reset_after_fingerprint_change(&mut record, true);
                record.insert(
                    "reason_code".to_owned(),
                    json!(if stream_generation > admitted_generation {
                        "stream_advanced"
                    } else {
                        "fingerprint_changed"
                    }),
                );
                record.insert("timeout_seconds".to_owned(), Value::Null);
                record.insert("bounded".to_owned(), json!(false));
                record.insert("last_outcome".to_owned(), json!("superseded"));
            } else {
                let reason = if outcome.timed_out {
                    "wall_clock_exceeded"
                } else {
                    "daily_catchup_failed"
                };
                record.insert(
                    "last_outcome".to_owned(),
                    json!(if outcome.timed_out {
                        "timeout"
                    } else {
                        "error"
                    }),
                );
                record.insert("reason_code".to_owned(), json!(reason));
                record.insert(
                    "timeout_seconds".to_owned(),
                    if outcome.timed_out {
                        outcome
                            .timeout_seconds
                            .map_or(Value::Null, |value| json!(value))
                    } else {
                        Value::Null
                    },
                );
                record.insert("bounded".to_owned(), json!(outcome.timed_out));
                let progress = record
                    .get("daily_progress")
                    .and_then(Value::as_object)
                    .and_then(|value| value.get("cleared"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if progress > 0 {
                    record.insert("last_outcome".to_owned(), json!("progressing"));
                    record.insert("consecutive_non_completion".to_owned(), json!(0));
                    record.insert("next_retry_at".to_owned(), json!(outcome.ended_at + 600.0));
                    record.insert("entered_backoff_at".to_owned(), Value::Null);
                    record.insert("notified_at".to_owned(), Value::Null);
                } else {
                    apply_no_progress_backoff(&mut record, outcome.ended_at);
                }
            }
            if daily_completed
                && let Some(repair_key) = settled_segment_repair_key(entries, day, &record)
            {
                entries.remove(&repair_key);
            }
            record.remove(SEGMENT_REPAIR_AT_ADMISSION);
            entries.insert(key, Value::Object(record));
            recorded = true;
            true
        });
        Ok(recorded)
    }
}

/// Reconcile catchup attempts that survived a supervisor crash.  Completion
/// proof wins; newer input supersedes an old attempt; only unchanged work is
/// charged an interrupted retry.
pub fn reconcile_stale_catchup_attempts(
    journal: &Path,
    now: SystemTime,
) -> Result<(), CatchupError> {
    reconcile_stale_catchup_attempts_with_capability(journal, now, catchup_marker_capability)
}

pub(crate) fn reconcile_stale_catchup_attempts_with_capability<Capability>(
    journal: &Path,
    now: SystemTime,
    capability: Capability,
) -> Result<(), CatchupError>
where
    Capability: FnOnce() -> Result<(), CatchupError>,
{
    capability()?;
    #[cfg(not(unix))]
    {
        let _ = (journal, now);
        return Err(CatchupError::CapabilityUnavailable);
    }
    #[cfg(unix)]
    {
        let ended_at = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        update_catchup_state(journal, false, |entries| {
            let keys = entries.keys().cloned().collect::<Vec<_>>();
            let mut changed = false;
            for key in keys {
                let Some(existing) = entries.get(&key) else {
                    continue;
                };
                let Some(kind) = existing.get("command_kind").and_then(Value::as_str) else {
                    continue;
                };
                if !matches!(kind, KIND_DAILY_CATCHUP | KIND_SEGMENT_REPAIR)
                    || !existing.get("active").is_some_and(json_truthy)
                {
                    continue;
                }
                let Some(day) = existing.get("day").and_then(Value::as_str) else {
                    continue;
                };
                let mut record = record_from_entry(Some(existing), day, kind);
                let current_fingerprint = read_raw_input_fingerprint(journal, day);
                let fingerprint_unreadable = current_fingerprint.is_err();
                let fingerprint_changed = match (
                    record.get("fingerprint").and_then(Value::as_str),
                    current_fingerprint.as_ref(),
                ) {
                    (Some(old), Ok(current)) => old != current,
                    _ => true,
                };
                record.insert("active".to_owned(), Value::Null);
                if kind == KIND_DAILY_CATCHUP {
                    let admitted = record
                        .get("admitted_generation")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let fingerprint = record.get("fingerprint").and_then(Value::as_str);
                    let daily_completed = fingerprint.is_some_and(|fingerprint| {
                        current_fingerprint.as_ref().is_ok_and(|current| {
                            current == fingerprint
                                && daily_marker_proves_attempt(journal, day, admitted, fingerprint)
                        })
                    });
                    let stream_advanced =
                        marker_generation(journal, day, HealthMarkerKind::Stream) > admitted;
                    if daily_completed {
                        record.insert("attempts".to_owned(), json!(0));
                        record.insert("consecutive_non_completion".to_owned(), json!(0));
                        record.insert("next_retry_at".to_owned(), json!(0));
                        record.insert("entered_backoff_at".to_owned(), Value::Null);
                        record.insert("notified_at".to_owned(), Value::Null);
                        record.insert("reason_code".to_owned(), Value::Null);
                        record.insert("timeout_seconds".to_owned(), Value::Null);
                        record.insert("bounded".to_owned(), json!(false));
                        record.insert("last_outcome".to_owned(), json!("completed"));
                    } else if fingerprint_unreadable {
                        record.insert("last_outcome".to_owned(), json!("error"));
                        record.insert("reason_code".to_owned(), json!("fingerprint_unreadable"));
                        apply_no_progress_backoff(&mut record, ended_at);
                    } else if stream_advanced || fingerprint_changed {
                        reset_after_fingerprint_change(&mut record, true);
                        record.insert("last_outcome".to_owned(), json!("superseded"));
                        record.insert(
                            "reason_code".to_owned(),
                            json!(if stream_advanced {
                                "stream_advanced"
                            } else {
                                "fingerprint_changed"
                            }),
                        );
                    } else {
                        record.insert("last_outcome".to_owned(), json!("interrupted"));
                        record.insert("reason_code".to_owned(), json!("interrupted"));
                        apply_no_progress_backoff(&mut record, ended_at);
                    }
                } else if fingerprint_changed {
                    reset_after_fingerprint_change(&mut record, false);
                    record.insert("last_outcome".to_owned(), json!("superseded"));
                    record.insert("reason_code".to_owned(), json!("fingerprint_changed"));
                } else {
                    record.insert("last_outcome".to_owned(), json!("interrupted"));
                    record.insert("reason_code".to_owned(), json!("interrupted"));
                    apply_no_progress_backoff(&mut record, ended_at);
                }
                let repair_to_settle = if kind == KIND_DAILY_CATCHUP
                    && record.get("last_outcome").and_then(Value::as_str) == Some("completed")
                {
                    settled_segment_repair_key(entries, day, &record)
                } else {
                    None
                };
                record.remove(SEGMENT_REPAIR_AT_ADMISSION);
                entries.insert(key, Value::Object(record));
                if let Some(repair_key) = repair_to_settle {
                    entries.remove(&repair_key);
                }
                changed = true;
            }
            changed
        });
        Ok(())
    }
}

/// Record the beginning of a segment repair and its raw-input fingerprint.
pub fn record_segment_repair_attempt(journal: &Path, day: &str, started_at: f64) {
    let Ok(fingerprint) = read_raw_input_fingerprint(journal, day) else {
        eprintln!("failed to fingerprint segment repair {day}");
        return;
    };
    update_catchup_state(journal, true, |entries| {
        let key = catchup_state_key(day, KIND_SEGMENT_REPAIR);
        let mut record = record_from_entry(entries.get(&key), day, KIND_SEGMENT_REPAIR);
        if record.get("fingerprint").and_then(Value::as_str) != Some(fingerprint.as_str()) {
            reset_after_fingerprint_change(&mut record, false);
        }
        let attempts = as_usize(record.get("attempts")) + 1;
        record.insert("fingerprint".to_owned(), json!(fingerprint));
        record.insert("attempts".to_owned(), json!(attempts));
        record.insert("last_attempt_at".to_owned(), json!(started_at));
        record.insert(
            "active".to_owned(),
            json!({"ref": "segment-repair", "started_at": started_at}),
        );
        entries.insert(key, Value::Object(record));
        true
    });
}

/// Record a segment-repair result. Failures are logged and swallowed so callers
/// cannot accidentally turn bookkeeping into a failed repair.
#[derive(Debug, Clone, Copy)]
pub struct SegmentRepairOutcome {
    pub success: bool,
    pub timed_out: bool,
    pub timeout_seconds: Option<f64>,
    pub ended_at: f64,
    pub cleared: Option<usize>,
    pub remaining: Option<usize>,
}

pub fn record_segment_repair_outcome(journal: &Path, day: &str, outcome: SegmentRepairOutcome) {
    update_catchup_state(journal, true, |entries| {
        let key = catchup_state_key(day, KIND_SEGMENT_REPAIR);
        if outcome.success {
            entries.remove(&key);
            return true;
        }
        let mut record = record_from_entry(entries.get(&key), day, KIND_SEGMENT_REPAIR);
        let reason = if outcome.timed_out {
            "wall_clock_exceeded"
        } else {
            "repair_failed"
        };
        record.insert("active".to_owned(), Value::Null);
        record.insert(
            "last_outcome".to_owned(),
            json!(if outcome.timed_out {
                "timeout"
            } else {
                "error"
            }),
        );
        record.insert("reason_code".to_owned(), json!(reason));
        record.insert(
            "timeout_seconds".to_owned(),
            if outcome.timed_out {
                outcome
                    .timeout_seconds
                    .map_or(Value::Null, |value| json!(value))
            } else {
                Value::Null
            },
        );
        record.insert("bounded".to_owned(), json!(outcome.timed_out));
        if outcome.cleared.is_some_and(|value| value > 0) {
            record.insert("last_outcome".to_owned(), json!("progressing"));
            record.insert("consecutive_non_completion".to_owned(), json!(0));
            record.insert("entered_backoff_at".to_owned(), Value::Null);
            record.insert("notified_at".to_owned(), Value::Null);
            record.insert("next_retry_at".to_owned(), json!(outcome.ended_at + 600.0));
            record.insert("cleared".to_owned(), json!(outcome.cleared));
            record.insert("remaining".to_owned(), json!(outcome.remaining));
            record.insert("exit_reason".to_owned(), json!(reason));
        } else {
            apply_no_progress_backoff(&mut record, outcome.ended_at);
        }
        entries.insert(key, Value::Object(record));
        true
    });
}

#[derive(Debug, Error)]
pub enum CatchupError {
    #[error("catchup capability unavailable on this platform")]
    CapabilityUnavailable,
    #[error("catchup journal path error: {0}")]
    Path(#[from] PathError),
    #[error("catchup I/O failed at {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("catchup state is malformed: {0}")]
    State(String),
    #[cfg(unix)]
    #[error("catchup marker error: {0}")]
    Marker(#[from] HealthMarkerError),
}

pub(crate) fn catchup_marker_capability() -> Result<(), CatchupError> {
    #[cfg(unix)]
    {
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err(CatchupError::CapabilityUnavailable)
    }
}

/// Return ascending day keys whose stream marker has not been completed.
pub fn updated_days(
    journal: &Path,
    exclude: &BTreeSet<String>,
) -> Result<Vec<String>, CatchupError> {
    catchup_marker_capability()?;
    #[cfg(not(unix))]
    {
        let _ = (journal, exclude);
        return Err(CatchupError::CapabilityUnavailable);
    }
    #[cfg(unix)]
    {
        let days = day_dirs(journal)?;
        let mut updated = Vec::new();
        for (day, _) in days {
            if exclude.contains(&day) {
                continue;
            }
            if !day_marker_pair_status(journal, &day)?.is_complete() {
                updated.push(day);
            }
        }
        updated.sort();
        Ok(updated)
    }
}

/// Return whether one day/kind record may be drained now.
pub fn day_eligible_to_drain(
    journal: &Path,
    day: &str,
    kind: CatchupKind,
    now: SystemTime,
) -> Result<bool, CatchupError> {
    let entries = read_entries(journal)?;
    let key = format!("{day}:{}", kind.as_str());
    let Some(entry) = entries.get(&key) else {
        return Ok(true);
    };
    let entry = entry
        .as_object()
        .ok_or_else(|| CatchupError::State(format!("entry {key} is not an object")))?;
    if entry.get("active").is_some_and(json_truthy) {
        return Ok(false);
    }
    let retry_at = entry.get("next_retry_at").map_or(Ok(0.0), json_number)?;
    let now = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    if now >= retry_at {
        return Ok(true);
    }
    if retry_at > 0.0
        && entry.get("reason_code").and_then(Value::as_str) == Some("admission_unreadable")
    {
        return Ok(false);
    }
    let fingerprint = entry
        .get("fingerprint")
        .and_then(Value::as_str)
        .ok_or_else(|| CatchupError::State(format!("entry {key} has no string fingerprint")))?;
    match read_raw_input_fingerprint(journal, day) {
        Ok(current) => Ok(current != fingerprint),
        // A terminal/restart fingerprint failure has already been charged a
        // retry boundary. Repeating the same unreadable scan must not let the
        // automatic fail-open wrapper resubmit it before that boundary.
        Err(_)
            if retry_at > 0.0
                && entry.get("reason_code").and_then(Value::as_str)
                    == Some("fingerprint_unreadable") =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

/// Return the Python-compatible raw-input fingerprint for a chronicle day.
pub fn read_raw_input_fingerprint(journal: &Path, day: &str) -> Result<String, CatchupError> {
    let day_dir = journal.join("chronicle").join(day);
    let mut entries = Vec::new();
    for segment in iter_segments(journal, PathOrDay::Day(day))? {
        for entry in read_dir(segment.path())? {
            let entry = entry.map_err(|source| CatchupError::Io {
                path: segment.path().to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                return Err(CatchupError::State(format!(
                    "non-UTF-8 input name under {}",
                    segment.path().display()
                )));
            };
            let marker = if is_raw_hashed(&name) && !is_processing_projection(&path) {
                sha256_file(&path)?
            } else if is_sized_media(&path) {
                format!("size:{}", metadata(&path)?.len())
            } else {
                continue;
            };
            let relative = path
                .strip_prefix(&day_dir)
                .map_err(|_| {
                    CatchupError::State(format!("segment path escaped day: {}", path.display()))
                })?
                .to_str()
                .ok_or_else(|| {
                    CatchupError::State(format!(
                        "non-UTF-8 relative path under {}",
                        day_dir.display()
                    ))
                })?
                .replace('\\', "/");
            entries.push((relative, marker));
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(hex_digest(Sha256::digest(
        compact_ascii_entries(&entries).as_bytes(),
    )))
}

/// Select newest-first natural days, followed by deduplicated forced days.
pub fn eligible_catchup_days(
    journal: &Path,
    force_days: &[String],
    exclude: &BTreeSet<String>,
    now: SystemTime,
) -> Result<Vec<String>, CatchupError> {
    let natural = updated_days(journal, exclude)?;
    let eligible_natural = natural
        .into_iter()
        .filter(|day| eligible_or_fail_open(journal, day, false, now))
        .collect::<Vec<_>>();
    let mut selected = eligible_natural
        .into_iter()
        .rev()
        .take(MAX_UPDATED_CATCHUP)
        .collect::<Vec<_>>();
    let mut seen = selected.iter().cloned().collect::<BTreeSet<_>>();
    for day in force_days {
        if eligible_or_fail_open(journal, day, true, now) && seen.insert(day.clone()) {
            selected.push(day.clone());
        }
    }
    Ok(selected)
}

fn eligible_or_fail_open(journal: &Path, day: &str, force: bool, now: SystemTime) -> bool {
    if force {
        return true;
    }
    match (|| {
        Ok::<_, CatchupError>(
            day_eligible_to_drain(journal, day, CatchupKind::DailyCatchup, now)?
                && day_eligible_to_drain(journal, day, CatchupKind::SegmentRepair, now)?,
        )
    })() {
        Ok(eligible) => eligible,
        Err(error) => {
            eprintln!(
                "supervisor: catchup eligibility check failed for {day}; treating as eligible: {error}"
            );
            true
        }
    }
}

/// Return days with a persisted, expired retry watermark without fingerprint work.
pub fn days_with_expired_retry(
    journal: &Path,
    exclude: &BTreeSet<String>,
    now: SystemTime,
) -> Result<Vec<String>, CatchupError> {
    let now = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let mut days = BTreeSet::new();
    for entry in read_entries(journal)?.into_values() {
        let Some(record) = entry.as_object() else {
            continue;
        };
        let Some(day) = record.get("day").and_then(Value::as_str) else {
            continue;
        };
        let kind = record.get("command_kind").and_then(Value::as_str);
        if !matches!(kind, Some(KIND_DAILY_CATCHUP | KIND_SEGMENT_REPAIR))
            || exclude.contains(day)
            || record.get("active").is_some_and(json_truthy)
        {
            continue;
        }
        let Some(retry_at) = record.get("next_retry_at").and_then(Value::as_f64) else {
            continue;
        };
        if retry_at > 0.0 && now >= retry_at {
            days.insert(day.to_owned());
        }
    }
    Ok(days.into_iter().collect())
}

fn read_entries(journal: &Path) -> Result<Map<String, Value>, CatchupError> {
    let path = catchup_state_path(journal);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(source) => return Err(CatchupError::Io { path, source }),
    };
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CatchupError::State(format!("invalid JSON: {error}")))?;
    strict_catchup_entries(&value)
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn json_number(value: &Value) -> Result<f64, CatchupError> {
    value
        .as_f64()
        .ok_or_else(|| CatchupError::State("next_retry_at is not numeric".to_owned()))
}

fn is_raw_hashed(name: &str) -> bool {
    RAW_HASHED_NAMES.contains(&name)
        || RAW_HASHED_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
        || (name.starts_with("monitor_")
            && (name.ends_with("_diff.json") || name.ends_with("_diff_box.json")))
}

/// Distinguish a server-authored media analysis sidecar from a legacy raw
/// JSONL input that happens to use the same filename.
///
/// A processing record alone is not authority to exclude content from the
/// raw-input fingerprint. The record must name the canonical schema and a
/// handler whose same-stem media sibling has the exact recorded input size.
/// Any unreadable or incomplete evidence remains fingerprinted as raw.
fn is_processing_projection(path: &Path) -> bool {
    if path.extension().and_then(|value| value.to_str()) != Some("jsonl")
        || !fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
    {
        return false;
    }
    let Some(record) = read_processing_record_header(path) else {
        return false;
    };
    if record.get("schema").and_then(Value::as_str) != Some(vocab::SCHEMA) {
        return false;
    }
    let Some(handler) = record.get("handler").and_then(Value::as_str) else {
        return false;
    };
    if !matches!(
        record.get("state").and_then(Value::as_str),
        Some(vocab::STATE_ANALYZED | vocab::STATE_EMPTY | vocab::STATE_FAILED)
    ) {
        return false;
    }
    let Some(input_size) = record.get("input_size").and_then(Value::as_u64) else {
        return false;
    };
    let Some(stem) = path.file_stem() else {
        return false;
    };
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(siblings) = fs::read_dir(parent) else {
        return false;
    };
    siblings.filter_map(Result::ok).any(|entry| {
        let sibling = entry.path();
        if sibling == path
            || sibling.file_stem() != Some(stem)
            || !entry
                .file_type()
                .map(|file_type| file_type.is_file())
                .unwrap_or(false)
        {
            return false;
        }
        let Some(extension) = sibling.extension().and_then(|value| value.to_str()) else {
            return false;
        };
        expected_handler(extension) == Some(handler)
            && entry
                .metadata()
                .map(|metadata| metadata.len() == input_size)
                .unwrap_or(false)
    })
}

fn is_sized_media(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
        .unwrap_or_default();
    MEDIA_EXTENSIONS.contains(&extension.as_str()) || PDF_EXTENSIONS.contains(&extension.as_str())
}

fn compact_ascii_entries(entries: &[(String, String)]) -> String {
    let body = entries
        .iter()
        .map(|(path, marker)| format!("[{},{}]", quote_ascii(path), quote_ascii(marker)))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{body}]")
}

fn quote_ascii(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character <= '\u{1f}' => {
                use std::fmt::Write;
                write!(&mut result, "\\u{:04x}", character as u32).expect("String write");
            }
            character if character.is_ascii() => result.push(character),
            character => {
                for unit in character.encode_utf16(&mut [0; 2]) {
                    use std::fmt::Write;
                    write!(&mut result, "\\u{unit:04x}").expect("String write");
                }
            }
        }
    }
    result.push('"');
    result
}

fn sha256_file(path: &Path) -> Result<String, CatchupError> {
    let bytes = fs::read(path).map_err(|source| CatchupError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(hex_digest(Sha256::digest(&bytes)))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_dir(path: &Path) -> Result<fs::ReadDir, CatchupError> {
    fs::read_dir(path).map_err(|source| CatchupError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn metadata(path: &Path) -> Result<fs::Metadata, CatchupError> {
    fs::metadata(path).map_err(|source| CatchupError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    use sha2::{Digest, Sha256};

    use super::*;

    static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

    struct Bed {
        root: PathBuf,
    }

    impl Bed {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "solstone-catchup-{name}-{}",
                NEXT_PATH.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).expect("temporary journal");
            Self { root }
        }

        fn write(&self, relative: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            fs::write(path, contents).expect("write fixture");
        }

        fn segment_file(&self, day: &str, segment: &str, name: &str, contents: &[u8]) {
            self.write(
                Path::new("chronicle").join(day).join(segment).join(name),
                contents,
            );
        }

        fn updated_day(&self, day: &str) {
            self.write(
                Path::new("chronicle")
                    .join(day)
                    .join("health/stream.updated"),
                br#"{"version":1,"generation":1,"fingerprint":null}"#,
            );
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn state_entry(day: &str, kind: CatchupKind, entry: &str) -> String {
        format!(
            r#"{{"version":1,"entries":{{"{day}:{}":{entry}}}}}"#,
            kind.as_str()
        )
    }

    fn digest(bytes: &[u8]) -> String {
        hex_digest(Sha256::digest(bytes))
    }

    fn empty_fingerprint() -> String {
        digest(b"[]")
    }

    fn daily_marker(generation: u64, fingerprint: &str) -> String {
        json!({
            "version": 1,
            "generation": generation,
            "fingerprint": fingerprint,
        })
        .to_string()
    }

    fn daily_outcome(ended_at: f64, exit_code: i32) -> DailyCatchupOutcome {
        DailyCatchupOutcome {
            success: exit_code == 0,
            timed_out: false,
            timeout_seconds: None,
            ended_at,
            exit_code,
            exit_status: if exit_code == 0 {
                "ok".to_owned()
            } else {
                "error".to_owned()
            },
        }
    }

    #[test]
    fn daily_catchup_attempt_records_admitted_generation_and_active_reference() {
        let bed = Bed::new("daily-attempt");
        record_daily_catchup_attempt(
            &bed.root,
            "20260101",
            "supervisor-catchup-20260101",
            10.0,
            7,
            "fp",
        );

        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["command_kind"], KIND_DAILY_CATCHUP);
        assert_eq!(daily["attempts"], 1);
        assert_eq!(daily["fingerprint"], "fp");
        assert_eq!(daily["admitted_generation"], 7);
        assert_eq!(
            daily["active"],
            json!({"ref": "supervisor-catchup-20260101", "started_at": 10.0})
        );
    }

    #[test]
    fn capability_refusal_precedes_daily_catchup_admission_state() {
        let bed = Bed::new("admission-capability");
        bed.write(
            "health/catchup-state.json",
            br#"{"version":1,"entries":{"20260101:daily-catchup":{"sentinel":"keep"}}}"#,
        );
        let state_path = catchup_state_path(&bed.root);
        let before = fs::read(&state_path).expect("seeded catchup state");

        let result =
            admit_daily_catchup_with_capability(&bed.root, "20260101", "catchup", 10.0, || {
                Err(CatchupError::CapabilityUnavailable)
            });

        assert!(matches!(result, Err(CatchupError::CapabilityUnavailable)));
        assert_eq!(fs::read(state_path).expect("catchup state"), before);
    }

    #[test]
    fn capability_refusal_precedes_daily_catchup_outcome_state() {
        let bed = Bed::new("outcome-capability");
        bed.write(
            "health/catchup-state.json",
            br#"{"version":1,"entries":{"20260101:daily-catchup":{"sentinel":"keep"}}}"#,
        );
        let state_path = catchup_state_path(&bed.root);
        let before = fs::read(&state_path).expect("seeded catchup state");

        let result = record_daily_catchup_outcome_with_capability(
            &bed.root,
            "20260101",
            "catchup",
            1,
            "fingerprint",
            daily_outcome(20.0, 0),
            || Err(CatchupError::CapabilityUnavailable),
        );

        assert!(matches!(result, Err(CatchupError::CapabilityUnavailable)));
        assert_eq!(fs::read(state_path).expect("catchup state"), before);
    }

    #[test]
    fn capability_refusal_precedes_stale_catchup_reconciliation_state() {
        let bed = Bed::new("reconcile-capability");
        bed.write(
            "health/catchup-state.json",
            br#"{"version":1,"entries":{"20260101:daily-catchup":{"sentinel":"keep"}}}"#,
        );
        let state_path = catchup_state_path(&bed.root);
        let before = fs::read(&state_path).expect("seeded catchup state");

        let result =
            reconcile_stale_catchup_attempts_with_capability(&bed.root, UNIX_EPOCH, || {
                Err(CatchupError::CapabilityUnavailable)
            });

        assert!(matches!(result, Err(CatchupError::CapabilityUnavailable)));
        assert_eq!(fs::read(state_path).expect("catchup state"), before);
    }

    #[test]
    fn completed_daily_catchup_settles_preexisting_inactive_segment_repair() {
        for reconcile in [false, true] {
            let bed = Bed::new("daily-settles-repair");
            let day = "20260101";
            let fingerprint = empty_fingerprint();
            record_segment_repair_attempt(&bed.root, day, 1.0);
            record_segment_repair_outcome(
                &bed.root,
                day,
                SegmentRepairOutcome {
                    success: false,
                    timed_out: false,
                    timeout_seconds: None,
                    ended_at: 5.0,
                    cleared: Some(32),
                    remaining: Some(2),
                },
            );
            record_daily_catchup_attempt(&bed.root, day, "catchup", 10.0, 2, &fingerprint);
            bed.write(
                "chronicle/20260101/health/daily.updated",
                daily_marker(2, &fingerprint),
            );
            if reconcile {
                reconcile_stale_catchup_attempts(&bed.root, UNIX_EPOCH + Duration::from_secs(20))
                    .unwrap();
            } else {
                assert!(
                    record_daily_catchup_outcome(
                        &bed.root,
                        day,
                        "catchup",
                        2,
                        &fingerprint,
                        daily_outcome(20.0, 0),
                    )
                    .unwrap()
                );
            }
            let state = read_catchup_state(&bed.root);
            let entries = catchup_entries(&state).unwrap();
            assert_eq!(
                entries[&catchup_state_key(day, KIND_DAILY_CATCHUP)]["last_outcome"],
                "completed"
            );
            assert!(!entries.contains_key(&catchup_state_key(day, KIND_SEGMENT_REPAIR)));
            assert!(
                !entries[&catchup_state_key(day, KIND_DAILY_CATCHUP)]
                    .as_object()
                    .unwrap()
                    .contains_key(SEGMENT_REPAIR_AT_ADMISSION)
            );
        }
    }

    #[test]
    fn daily_catchup_preserves_unproven_or_overlapping_repairs() {
        for case in [
            "active-before",
            "overlap-finishes",
            "changed-after",
            "new-after",
            "different-input",
            "no-marker",
            "wrong-reference",
            "wrong-generation",
        ] {
            let bed = Bed::new(case);
            let day = "20260101";
            let fingerprint = empty_fingerprint();
            let failed = SegmentRepairOutcome {
                success: false,
                timed_out: false,
                timeout_seconds: None,
                ended_at: 5.0,
                cleared: Some(32),
                remaining: Some(2),
            };
            if case != "new-after" {
                record_segment_repair_attempt(&bed.root, day, 1.0);
                if !matches!(case, "active-before" | "overlap-finishes") {
                    record_segment_repair_outcome(&bed.root, day, failed);
                }
            }
            if case == "different-input" {
                update_catchup_state(&bed.root, false, |entries| {
                    entries
                        .get_mut(&catchup_state_key(day, KIND_SEGMENT_REPAIR))
                        .unwrap()["fingerprint"] = json!("different-input");
                    true
                });
            }
            record_daily_catchup_attempt(&bed.root, day, "catchup", 10.0, 2, &fingerprint);
            if matches!(case, "changed-after" | "new-after") {
                record_segment_repair_attempt(&bed.root, day, 15.0);
                record_segment_repair_outcome(
                    &bed.root,
                    day,
                    SegmentRepairOutcome {
                        ended_at: 16.0,
                        ..failed
                    },
                );
            } else if case == "overlap-finishes" {
                record_segment_repair_outcome(
                    &bed.root,
                    day,
                    SegmentRepairOutcome {
                        ended_at: 16.0,
                        ..failed
                    },
                );
            }
            let repair_key = catchup_state_key(day, KIND_SEGMENT_REPAIR);
            let before = read_catchup_state(&bed.root)["entries"][&repair_key].clone();
            assert!(before.is_object());
            if case != "no-marker" {
                bed.write(
                    "chronicle/20260101/health/daily.updated",
                    daily_marker(2, &fingerprint),
                );
            }
            let recorded = record_daily_catchup_outcome(
                &bed.root,
                day,
                if case == "wrong-reference" {
                    "old-catchup"
                } else {
                    "catchup"
                },
                if case == "wrong-generation" { 1 } else { 2 },
                &fingerprint,
                daily_outcome(20.0, 0),
            )
            .unwrap();
            assert_eq!(
                recorded,
                !matches!(case, "wrong-reference" | "wrong-generation"),
                "{case}"
            );
            assert_eq!(
                read_catchup_state(&bed.root)["entries"][&repair_key],
                before,
                "{case}"
            );
        }
    }

    #[test]
    fn daily_catchup_completion_uses_generation_proof_over_exit_status() {
        let bed = Bed::new("daily-complete");
        let fingerprint = empty_fingerprint();
        record_daily_catchup_attempt(&bed.root, "20260101", "catchup", 10.0, 2, &fingerprint);
        bed.write(
            "chronicle/20260101/health/daily.updated",
            daily_marker(2, &fingerprint),
        );
        assert!(
            record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "catchup",
                2,
                &fingerprint,
                daily_outcome(20.0, 1),
            )
            .unwrap()
        );

        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["last_outcome"], "completed");
        assert_eq!(daily["attempts"], 0);
        assert_eq!(daily["active"], Value::Null);
        assert_eq!(daily["next_retry_at"], 0);
        assert_eq!(daily["exit_status"], "error");
    }

    #[test]
    fn daily_catchup_zero_generation_requires_versioned_daily_marker() {
        let bed = Bed::new("daily-zero-generation");
        let fingerprint = empty_fingerprint();
        record_daily_catchup_attempt(&bed.root, "20260101", "catchup", 10.0, 0, &fingerprint);

        assert!(
            record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "catchup",
                0,
                &fingerprint,
                daily_outcome(20.0, 1)
            )
            .unwrap()
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["last_outcome"], "error");
        assert_eq!(daily["consecutive_non_completion"], 1);
        assert_eq!(daily["next_retry_at"], 620.0);

        bed.write(
            "chronicle/20260101/health/daily.updated",
            daily_marker(0, &fingerprint),
        );
        record_daily_catchup_attempt(&bed.root, "20260101", "catchup-2", 25.0, 0, &fingerprint);
        assert!(
            record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "catchup-2",
                0,
                &fingerprint,
                daily_outcome(30.0, 1)
            )
            .unwrap()
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["last_outcome"], "completed");
        assert_eq!(daily["attempts"], 0);
        assert_eq!(daily["active"], Value::Null);
    }

    #[test]
    fn daily_catchup_newer_stream_generation_skips_backoff() {
        let bed = Bed::new("daily-superseded");
        let fingerprint = empty_fingerprint();
        record_daily_catchup_attempt(&bed.root, "20260101", "catchup", 10.0, 1, &fingerprint);
        bed.write(
            "chronicle/20260101/health/stream.updated",
            br#"{"version":1,"generation":2,"fingerprint":null}"#,
        );
        assert!(
            record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "catchup",
                1,
                &fingerprint,
                daily_outcome(20.0, 1)
            )
            .unwrap()
        );

        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["last_outcome"], "superseded");
        assert_eq!(daily["consecutive_non_completion"], 0);
        assert_eq!(daily["next_retry_at"], 0);
        assert_eq!(daily["active"], Value::Null);
    }

    #[test]
    fn daily_catchup_no_progress_applies_backoff_and_stuck_projection() {
        let bed = Bed::new("daily-backoff");
        let fingerprint = empty_fingerprint();
        record_daily_catchup_attempt(&bed.root, "20260101", "catchup", 10.0, 1, &fingerprint);
        bed.write(
            "chronicle/20260101/health/stream.updated",
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        );
        assert!(
            record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "catchup",
                1,
                &fingerprint,
                daily_outcome(20.0, 1)
            )
            .unwrap()
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["consecutive_non_completion"], 1);
        assert_eq!(daily["next_retry_at"], 620.0);
        record_daily_catchup_attempt(&bed.root, "20260101", "catchup-2", 25.0, 1, &fingerprint);
        assert!(
            record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "catchup-2",
                1,
                &fingerprint,
                daily_outcome(30.0, 1)
            )
            .unwrap()
        );
        record_daily_catchup_attempt(&bed.root, "20260101", "catchup-3", 35.0, 1, &fingerprint);
        assert!(
            record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "catchup-3",
                1,
                &fingerprint,
                daily_outcome(40.0, 1)
            )
            .unwrap()
        );

        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["consecutive_non_completion"], 3);
        assert_eq!(daily["next_retry_at"], 2440.0);
        assert_eq!(daily["entered_backoff_at"], 40.0);
        assert_eq!(daily["notified_at"], 40.0);
        assert_eq!(daily["active"], Value::Null);
    }

    #[test]
    fn daily_outcome_requires_the_active_reference_and_records_measured_progress() {
        let bed = Bed::new("daily-correlation");
        let fingerprint = empty_fingerprint();
        record_daily_catchup_attempt(&bed.root, "20260101", "right", 10.0, 1, &fingerprint);
        assert!(
            !record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "wrong",
                1,
                &fingerprint,
                daily_outcome(20.0, 1),
            )
            .unwrap()
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        assert_eq!(
            state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)]["active"]["ref"],
            "right"
        );

        record_daily_catchup_progress(&bed.root, "20260101", 2, 1);
        assert!(
            record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "right",
                1,
                &fingerprint,
                daily_outcome(20.0, 1),
            )
            .unwrap()
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["last_outcome"], "progressing");
        assert_eq!(daily["consecutive_non_completion"], 0);
        assert_eq!(daily["next_retry_at"], 620.0);

        // Progress is correlated to the primary attempt, not merely to the
        // fingerprint.  A subsequent attempt that reports no progress must
        // take the ordinary retry path.
        record_daily_catchup_attempt(&bed.root, "20260101", "right-2", 21.0, 1, &fingerprint);
        assert!(
            record_daily_catchup_outcome(
                &bed.root,
                "20260101",
                "right-2",
                1,
                &fingerprint,
                daily_outcome(30.0, 1),
            )
            .unwrap()
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["consecutive_non_completion"], 1);
        assert_eq!(daily["next_retry_at"], 630.0);

        // A new dirty generation is a new lifecycle, even when its raw files
        // fingerprint identically.  It must not inherit the old backoff.
        record_daily_catchup_attempt(
            &bed.root,
            "20260101",
            "new-generation",
            31.0,
            2,
            &fingerprint,
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["consecutive_non_completion"], 0);
        assert_eq!(daily["next_retry_at"], 0.0);
    }

    #[test]
    fn stale_active_reconciliation_uses_completion_then_new_input_then_interruption() {
        let completed = Bed::new("reconcile-completed");
        let fingerprint = empty_fingerprint();
        record_daily_catchup_attempt(&completed.root, "20260101", "active", 1.0, 2, &fingerprint);
        completed.write(
            "chronicle/20260101/health/daily.updated",
            daily_marker(2, &fingerprint),
        );
        reconcile_stale_catchup_attempts(&completed.root, UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&completed.root)).unwrap())
                .unwrap();
        assert_eq!(
            state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)]["last_outcome"],
            "completed"
        );

        let superseded = Bed::new("reconcile-superseded");
        record_daily_catchup_attempt(&superseded.root, "20260101", "active", 1.0, 1, &fingerprint);
        superseded.write(
            "chronicle/20260101/health/stream.updated",
            br#"{"version":1,"generation":2,"fingerprint":null}"#,
        );
        reconcile_stale_catchup_attempts(&superseded.root, UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&superseded.root)).unwrap())
                .unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["last_outcome"], "superseded");
        assert_eq!(daily["next_retry_at"], 0);

        let interrupted = Bed::new("reconcile-interrupted");
        record_daily_catchup_attempt(
            &interrupted.root,
            "20260101",
            "active",
            1.0,
            1,
            &fingerprint,
        );
        reconcile_stale_catchup_attempts(&interrupted.root, UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&interrupted.root)).unwrap())
                .unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["last_outcome"], "interrupted");
        assert_eq!(daily["next_retry_at"], 620.0);

        let segment = Bed::new("reconcile-segment");
        record_segment_repair_attempt(&segment.root, "20260101", 1.0);
        reconcile_stale_catchup_attempts(&segment.root, UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&segment.root)).unwrap()).unwrap();
        let repair = &state["entries"][catchup_state_key("20260101", KIND_SEGMENT_REPAIR)];
        assert_eq!(repair["last_outcome"], "interrupted");
        assert_eq!(repair["next_retry_at"], 620.0);

        let changed_segment = Bed::new("reconcile-segment-changed");
        record_segment_repair_attempt(&changed_segment.root, "20260101", 1.0);
        changed_segment.segment_file("20260101", "000000_1", "audio.json", br#"{}"#);
        reconcile_stale_catchup_attempts(
            &changed_segment.root,
            UNIX_EPOCH + Duration::from_secs(20),
        )
        .unwrap();
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&changed_segment.root)).unwrap())
                .unwrap();
        let repair = &state["entries"][catchup_state_key("20260101", KIND_SEGMENT_REPAIR)];
        assert_eq!(repair["last_outcome"], "superseded");
        assert_eq!(repair["next_retry_at"], 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unreadable_fingerprint_never_completes_or_supersedes_a_daily_attempt() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let terminal = Bed::new("daily-terminal-unreadable-fingerprint");
        let fingerprint = empty_fingerprint();
        record_daily_catchup_attempt(&terminal.root, "20260101", "active", 10.0, 1, &fingerprint);
        terminal.write(
            "chronicle/20260101/health/daily.updated",
            daily_marker(1, &fingerprint),
        );
        let unreadable = terminal
            .root
            .join("chronicle/20260101/000000_1")
            .join(OsString::from_vec(vec![0xff]));
        fs::create_dir_all(unreadable.parent().unwrap()).unwrap();
        fs::write(&unreadable, b"raw").unwrap();

        assert!(
            record_daily_catchup_outcome(
                &terminal.root,
                "20260101",
                "active",
                1,
                &fingerprint,
                daily_outcome(20.0, 0),
            )
            .unwrap()
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&terminal.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["last_outcome"], "error");
        assert_eq!(daily["reason_code"], "fingerprint_unreadable");
        assert_eq!(daily["next_retry_at"], 620.0);
        // Keep the day dirty so the automatic selector reaches the retry gate
        // instead of treating the matching daily marker as already complete.
        terminal.write(
            "chronicle/20260101/health/stream.updated",
            br#"{"version":1,"generation":2,"fingerprint":null}"#,
        );
        assert_eq!(
            eligible_catchup_days(
                &terminal.root,
                &[],
                &BTreeSet::new(),
                UNIX_EPOCH + Duration::from_secs(619),
            )
            .expect("unreadable fingerprint remains held before retry"),
            Vec::<String>::new(),
        );
        assert_eq!(
            eligible_catchup_days(
                &terminal.root,
                &[],
                &BTreeSet::new(),
                UNIX_EPOCH + Duration::from_secs(620),
            )
            .expect("unreadable fingerprint is eligible at retry"),
            vec!["20260101".to_owned()],
        );

        let restart = Bed::new("daily-restart-unreadable-fingerprint");
        record_daily_catchup_attempt(&restart.root, "20260101", "active", 10.0, 1, &fingerprint);
        restart.write(
            "chronicle/20260101/health/daily.updated",
            daily_marker(1, &fingerprint),
        );
        let unreadable = restart
            .root
            .join("chronicle/20260101/000000_1")
            .join(OsString::from_vec(vec![0xff]));
        fs::create_dir_all(unreadable.parent().unwrap()).unwrap();
        fs::write(&unreadable, b"raw").unwrap();

        reconcile_stale_catchup_attempts(&restart.root, UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&restart.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(daily["last_outcome"], "error");
        assert_eq!(daily["reason_code"], "fingerprint_unreadable");
        assert_eq!(daily["next_retry_at"], 620.0);
    }

    #[test]
    fn writers_preserve_repair_transitions_and_daily_retention() {
        let bed = Bed::new("repair-writers");
        bed.segment_file("20260101", "120000_60", "audio.json", b"raw");
        record_daily_catchup_progress(&bed.root, "20260101", 2, 3);
        record_segment_repair_attempt(&bed.root, "20260101", 10.0);
        record_segment_repair_attempt(&bed.root, "20260101", 11.0);
        record_segment_repair_outcome(
            &bed.root,
            "20260101",
            SegmentRepairOutcome {
                success: false,
                timed_out: true,
                timeout_seconds: Some(9.0),
                ended_at: 20.0,
                cleared: None,
                remaining: None,
            },
        );
        record_segment_repair_outcome(
            &bed.root,
            "20260101",
            SegmentRepairOutcome {
                success: false,
                timed_out: false,
                timeout_seconds: None,
                ended_at: 21.0,
                cleared: None,
                remaining: None,
            },
        );
        record_segment_repair_outcome(
            &bed.root,
            "20260101",
            SegmentRepairOutcome {
                success: false,
                timed_out: false,
                timeout_seconds: None,
                ended_at: 22.0,
                cleared: None,
                remaining: None,
            },
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        let daily = &state["entries"][catchup_state_key("20260101", KIND_DAILY_CATCHUP)];
        assert_eq!(
            daily["daily_progress"],
            json!({"cleared": 2, "remaining": 3})
        );
        let repair = &state["entries"][catchup_state_key("20260101", KIND_SEGMENT_REPAIR)];
        assert_eq!(repair["attempts"], 2);
        assert_eq!(repair["consecutive_non_completion"], 3);
        assert_eq!(repair["entered_backoff_at"], 22.0);
        assert_eq!(repair["notified_at"], 22.0);
        record_segment_repair_outcome(
            &bed.root,
            "20260101",
            SegmentRepairOutcome {
                success: true,
                timed_out: false,
                timeout_seconds: None,
                ended_at: 23.0,
                cleared: None,
                remaining: None,
            },
        );
        let state: Value =
            serde_json::from_slice(&fs::read(catchup_state_path(&bed.root)).unwrap()).unwrap();
        assert!(
            state["entries"]
                .get(catchup_state_key("20260101", KIND_SEGMENT_REPAIR))
                .is_none()
        );
    }

    #[test]
    fn missing_catchup_state_is_eligible() {
        let bed = Bed::new("missing-state");
        assert!(
            day_eligible_to_drain(&bed.root, "20260101", CatchupKind::DailyCatchup, UNIX_EPOCH)
                .expect("missing state")
        );
    }

    #[test]
    fn strict_drain_reader_keeps_malformed_state_errors() {
        let bed = Bed::new("strict-state");

        bed.write("health/catchup-state.json", b"[]");
        assert!(matches!(
            day_eligible_to_drain(&bed.root, "20260101", CatchupKind::DailyCatchup, UNIX_EPOCH),
            Err(CatchupError::State(_))
        ));

        bed.write("health/catchup-state.json", br#"{"entries":[]}"#);
        assert!(matches!(
            day_eligible_to_drain(&bed.root, "20260101", CatchupKind::DailyCatchup, UNIX_EPOCH),
            Err(CatchupError::State(_))
        ));

        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260101",
                CatchupKind::DailyCatchup,
                r#"{"next_retry_at":"bad","fingerprint":"fingerprint"}"#,
            ),
        );
        assert!(matches!(
            day_eligible_to_drain(&bed.root, "20260101", CatchupKind::DailyCatchup, UNIX_EPOCH),
            Err(CatchupError::State(_))
        ));

        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260101",
                CatchupKind::DailyCatchup,
                r#"{"next_retry_at":10}"#,
            ),
        );
        assert!(matches!(
            day_eligible_to_drain(&bed.root, "20260101", CatchupKind::DailyCatchup, UNIX_EPOCH),
            Err(CatchupError::State(_))
        ));

        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260101",
                CatchupKind::DailyCatchup,
                r#"{"next_retry_at":10,"fingerprint":false}"#,
            ),
        );
        assert!(matches!(
            day_eligible_to_drain(&bed.root, "20260101", CatchupKind::DailyCatchup, UNIX_EPOCH),
            Err(CatchupError::State(_))
        ));
    }

    #[test]
    fn active_catchup_state_is_not_eligible() {
        let bed = Bed::new("active");
        bed.write(
            "health/catchup-state.json",
            state_entry("20260101", CatchupKind::DailyCatchup, r#"{"active":true}"#),
        );
        assert!(
            !day_eligible_to_drain(&bed.root, "20260101", CatchupKind::DailyCatchup, UNIX_EPOCH)
                .expect("active state")
        );
    }

    #[test]
    fn past_retry_is_eligible() {
        let bed = Bed::new("past-retry");
        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260101",
                CatchupKind::DailyCatchup,
                r#"{"next_retry_at":10,"fingerprint":"ignored"}"#,
            ),
        );
        assert!(
            day_eligible_to_drain(
                &bed.root,
                "20260101",
                CatchupKind::DailyCatchup,
                UNIX_EPOCH + Duration::from_secs(10)
            )
            .expect("past retry")
        );
    }

    #[test]
    fn unchanged_fingerprint_before_retry_is_not_eligible() {
        let bed = Bed::new("unchanged-fingerprint");
        let fingerprint = empty_fingerprint();
        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260101",
                CatchupKind::DailyCatchup,
                &format!(r#"{{"next_retry_at":10,"fingerprint":"{fingerprint}"}}"#),
            ),
        );
        assert!(
            !day_eligible_to_drain(
                &bed.root,
                "20260101",
                CatchupKind::DailyCatchup,
                UNIX_EPOCH + Duration::from_secs(9)
            )
            .expect("unchanged fingerprint")
        );
    }

    #[test]
    fn changed_fingerprint_before_retry_is_eligible() {
        let bed = Bed::new("changed-fingerprint");
        bed.segment_file("20260101", "120000_1", "audio.json", b"new raw input");
        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260101",
                CatchupKind::DailyCatchup,
                &format!(
                    r#"{{"next_retry_at":10,"fingerprint":"{}"}}"#,
                    empty_fingerprint()
                ),
            ),
        );
        assert!(
            day_eligible_to_drain(
                &bed.root,
                "20260101",
                CatchupKind::DailyCatchup,
                UNIX_EPOCH + Duration::from_secs(9)
            )
            .expect("changed fingerprint")
        );
    }

    #[test]
    fn raw_hash_conditions_use_sha256_markers() {
        let bed = Bed::new("raw-names");
        let files = [
            ("audio.json", b"exact".as_slice()),
            ("chat.jsonl", b"chat".as_slice()),
            ("capture_audio.jsonl", b"suffix".as_slice()),
            ("monitor_12_diff_box.json", b"glob".as_slice()),
        ];
        for (name, contents) in &files {
            bed.segment_file("20260101", "120000_1", name, contents);
        }
        let entries = files
            .iter()
            .map(|(name, contents)| (format!("120000_1/{name}"), digest(contents)))
            .collect::<Vec<_>>();
        let mut entries = entries;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            read_raw_input_fingerprint(&bed.root, "20260101").expect("fingerprint"),
            digest(compact_ascii_entries(&entries).as_bytes())
        );
        assert!(is_raw_hashed("audio.json"));
        assert!(is_raw_hashed("chat.jsonl"));
        assert!(is_raw_hashed("capture_audio.jsonl"));
        assert!(is_raw_hashed("monitor_12_diff_box.json"));
    }

    #[test]
    fn server_media_projections_do_not_redirty_their_own_raw_fingerprint() {
        for (case, media, sidecar, handler) in [
            (
                "audio",
                "audio.wav",
                "audio.jsonl",
                vocab::HANDLER_TRANSCRIBE,
            ),
            (
                "screen",
                "screen.webm",
                "screen.jsonl",
                vocab::HANDLER_DESCRIBE,
            ),
            (
                "legacy-suffix",
                "mic_audio.flac",
                "mic_audio.jsonl",
                vocab::HANDLER_TRANSCRIBE,
            ),
        ] {
            let bed = Bed::new(case);
            let raw = b"raw media bytes";
            bed.segment_file("20260101", "120000_1", media, raw);
            let before =
                read_raw_input_fingerprint(&bed.root, "20260101").expect("raw fingerprint");

            let derived = format!(
                "{{\"raw\":\"{media}\",\"_solstone_processing\":{{\"schema\":\"{}\",\"state\":\"analyzed\",\"handler\":\"{handler}\",\"input_size\":{}}}}}\n{{\"start\":\"12:00:00\",\"text\":\"derived\"}}\n",
                vocab::SCHEMA,
                raw.len()
            );
            bed.segment_file("20260101", "120000_1", sidecar, derived.as_bytes());

            let path = bed.root.join("chronicle/20260101/120000_1").join(sidecar);
            assert!(is_processing_projection(&path), "{case}");
            assert_eq!(
                read_raw_input_fingerprint(&bed.root, "20260101").expect("stable fingerprint"),
                before,
                "{case}"
            );
        }
    }

    #[test]
    fn processing_shaped_jsonl_without_exact_media_authority_remains_raw() {
        let bed = Bed::new("projection-fail-closed");
        let sidecar = format!(
            "{{\"_solstone_processing\":{{\"schema\":\"{}\",\"state\":\"analyzed\",\"handler\":\"transcribe\",\"input_size\":9}}}}\n",
            vocab::SCHEMA
        );
        bed.segment_file("20260101", "120000_1", "audio.jsonl", sidecar.as_bytes());
        let path = bed.root.join("chronicle/20260101/120000_1/audio.jsonl");
        assert!(!is_processing_projection(&path));
        assert_eq!(
            read_raw_input_fingerprint(&bed.root, "20260101").expect("fingerprint"),
            digest(
                compact_ascii_entries(&[(
                    "120000_1/audio.jsonl".to_owned(),
                    digest(sidecar.as_bytes()),
                )])
                .as_bytes()
            )
        );

        bed.segment_file("20260101", "120000_1", "audio.wav", b"wrong size");
        assert!(!is_processing_projection(&path));
    }

    #[test]
    fn non_jsonl_incomplete_and_wrong_handler_records_remain_raw() {
        let bed = Bed::new("projection-shape");
        let raw = b"raw media bytes";
        bed.segment_file("20260101", "120000_1", "audio.wav", raw);
        let processing = format!(
            "{{\"_solstone_processing\":{{\"schema\":\"{}\",\"state\":\"analyzed\",\"handler\":\"transcribe\",\"input_size\":{}}}}}\n",
            vocab::SCHEMA,
            raw.len()
        );
        bed.segment_file("20260101", "120000_1", "audio.json", processing.as_bytes());
        assert!(!is_processing_projection(
            &bed.root.join("chronicle/20260101/120000_1/audio.json")
        ));

        let incomplete = format!(
            "{{\"_solstone_processing\":{{\"schema\":\"{}\",\"handler\":\"transcribe\",\"input_size\":{}}}}}\n",
            vocab::SCHEMA,
            raw.len()
        );
        bed.segment_file("20260101", "120000_1", "audio.jsonl", incomplete.as_bytes());
        assert!(!is_processing_projection(
            &bed.root.join("chronicle/20260101/120000_1/audio.jsonl")
        ));

        bed.segment_file("20260101", "120000_1", "screen.webm", raw);
        let wrong_handler = format!(
            "{{\"_solstone_processing\":{{\"schema\":\"{}\",\"state\":\"failed\",\"handler\":\"transcribe\",\"input_size\":{}}}}}\n",
            vocab::SCHEMA,
            raw.len()
        );
        bed.segment_file(
            "20260101",
            "120000_1",
            "screen.jsonl",
            wrong_handler.as_bytes(),
        );
        assert!(!is_processing_projection(
            &bed.root.join("chronicle/20260101/120000_1/screen.jsonl")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_projection_or_media_authority_remains_raw() {
        use std::os::unix::fs::symlink;

        let bed = Bed::new("projection-symlinks");
        let segment = bed.root.join("chronicle/20260101/120000_1");
        fs::create_dir_all(&segment).unwrap();
        let raw = b"raw media bytes";
        fs::write(segment.join("source.bin"), raw).unwrap();
        symlink("source.bin", segment.join("audio.wav")).unwrap();
        let derived = format!(
            "{{\"_solstone_processing\":{{\"schema\":\"{}\",\"state\":\"empty\",\"handler\":\"transcribe\",\"input_size\":{}}}}}\n",
            vocab::SCHEMA,
            raw.len()
        );
        fs::write(segment.join("actual.jsonl"), &derived).unwrap();
        symlink("actual.jsonl", segment.join("audio.jsonl")).unwrap();
        assert!(!is_processing_projection(&segment.join("audio.jsonl")));

        fs::remove_file(segment.join("audio.jsonl")).unwrap();
        fs::write(segment.join("audio.jsonl"), derived).unwrap();
        assert!(!is_processing_projection(&segment.join("audio.jsonl")));
    }

    #[test]
    fn media_and_pdf_use_sizes_and_skip_unrelated_files() {
        let bed = Bed::new("media");
        bed.segment_file("20260101", "120000_1", "image.PNG", b"1234");
        bed.segment_file("20260101", "120000_1", "report.pdf", b"123456");
        bed.segment_file("20260101", "120000_1", "notes.txt", b"must not count");
        let entries = vec![
            ("120000_1/image.PNG".to_owned(), "size:4".to_owned()),
            ("120000_1/report.pdf".to_owned(), "size:6".to_owned()),
        ];
        assert_eq!(
            read_raw_input_fingerprint(&bed.root, "20260101").expect("fingerprint"),
            digest(compact_ascii_entries(&entries).as_bytes())
        );
    }

    #[test]
    fn fingerprint_sorts_entries_by_path_not_marker() {
        let bed = Bed::new("path-sort");
        bed.segment_file("20260101", "130000_1", "audio.json", b"a");
        bed.segment_file("20260101", "120000_1", "audio.json", b"z");
        let entries = vec![
            ("120000_1/audio.json".to_owned(), digest(b"z")),
            ("130000_1/audio.json".to_owned(), digest(b"a")),
        ];
        assert_eq!(
            read_raw_input_fingerprint(&bed.root, "20260101").expect("fingerprint"),
            digest(compact_ascii_entries(&entries).as_bytes())
        );
    }

    #[test]
    fn eligible_days_caps_natural_days_but_keeps_forced_days() {
        let bed = Bed::new("cap");
        for day in [
            "20260101", "20260102", "20260103", "20260104", "20260105", "20260106",
        ] {
            bed.updated_day(day);
        }
        let force = ["20260101".to_owned()];
        let exclude = BTreeSet::new();
        let capped_with_forced = [
            "20260106".to_owned(),
            "20260105".to_owned(),
            "20260104".to_owned(),
            "20260103".to_owned(),
            "20260101".to_owned(),
        ];
        assert_eq!(
            eligible_catchup_days(&bed.root, &force, &exclude, UNIX_EPOCH).expect("eligible days"),
            capped_with_forced
        );

        let fingerprint = empty_fingerprint();
        bed.write(
            "health/catchup-state.json",
            state_entry(
                "20260106",
                CatchupKind::DailyCatchup,
                &format!(r#"{{"next_retry_at":10,"fingerprint":"{fingerprint}"}}"#),
            ),
        );
        assert_eq!(
            eligible_catchup_days(
                &bed.root,
                &force,
                &exclude,
                UNIX_EPOCH + Duration::from_secs(9)
            )
            .expect("before retry"),
            vec![
                "20260105".to_owned(),
                "20260104".to_owned(),
                "20260103".to_owned(),
                "20260102".to_owned(),
                "20260101".to_owned(),
            ]
        );
        assert_eq!(
            eligible_catchup_days(
                &bed.root,
                &force,
                &exclude,
                UNIX_EPOCH + Duration::from_secs(10)
            )
            .expect("at retry"),
            capped_with_forced
        );
        assert_eq!(
            eligible_catchup_days(
                &bed.root,
                &force,
                &exclude,
                UNIX_EPOCH + Duration::from_secs(11)
            )
            .expect("after retry"),
            capped_with_forced
        );
    }

    #[test]
    fn forced_days_bypass_both_catchup_kind_gates() {
        let bed = Bed::new("forced-bypass");
        bed.write(
            "health/catchup-state.json",
            r#"{"version":1,"entries":{"20260101:daily-catchup":{"day":"20260101","command_kind":"daily-catchup","active":{"ref":"daily"},"next_retry_at":9999,"fingerprint":"unchanged"},"20260101:segment-repair":{"day":"20260101","command_kind":"segment-repair","active":{"ref":"repair"},"next_retry_at":9999,"fingerprint":"unchanged"}}}"#,
        );

        assert_eq!(
            eligible_catchup_days(
                &bed.root,
                &["20260101".to_owned()],
                &BTreeSet::new(),
                UNIX_EPOCH,
            )
            .expect("forced day"),
            vec!["20260101".to_owned()]
        );
    }

    #[test]
    fn expired_retry_days_are_watermark_only_and_honor_exclusion() {
        let bed = Bed::new("expired-retry");
        bed.write(
            "health/catchup-state.json",
            r#"{"version":1,"entries":{"20260101:daily-catchup":{"day":"20260101","command_kind":"daily-catchup","active":null,"next_retry_at":10,"fingerprint":"would-not-hash"},"20260102:segment-repair":{"day":"20260102","command_kind":"segment-repair","active":null,"next_retry_at":20,"fingerprint":"would-not-hash"},"20260103:daily-catchup":{"day":"20260103","command_kind":"daily-catchup","active":{"ref":"active"},"next_retry_at":1},"20260104:daily-catchup":{"day":"20260104","command_kind":"daily-catchup","active":null,"next_retry_at":0},"20260105:daily-catchup":{"day":"20260105","command_kind":"daily-catchup","active":null,"next_retry_at":30}}}"#,
        );

        assert_eq!(
            days_with_expired_retry(
                &bed.root,
                &BTreeSet::from(["20260102".to_owned()]),
                UNIX_EPOCH + Duration::from_secs(20),
            )
            .expect("expired retry days"),
            vec!["20260101".to_owned()]
        );
    }

    #[test]
    fn updated_days_honors_exclusion_and_marker_order() {
        let bed = Bed::new("updated-days");
        bed.updated_day("20260101");
        bed.write(
            "chronicle/20260102/health/daily.updated",
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        );
        bed.write(
            "chronicle/20260102/health/stream.updated",
            br#"{"version":1,"generation":2,"fingerprint":null}"#,
        );
        bed.write(
            "chronicle/20260103/health/stream.updated",
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        );
        bed.write(
            "chronicle/20260103/health/daily.updated",
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        );
        bed.write(
            "chronicle/20260104/health/stream.updated",
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        );
        bed.write(
            "chronicle/20260104/health/daily.updated",
            br#"{"version":1,"generation":2,"fingerprint":null}"#,
        );
        assert_eq!(
            updated_days(&bed.root, &BTreeSet::new()).expect("updated days"),
            vec![
                "20260101".to_owned(),
                "20260102".to_owned(),
                "20260104".to_owned(),
            ]
        );
        assert_eq!(
            updated_days(&bed.root, &BTreeSet::from(["20260101".to_owned()]))
                .expect("updated days"),
            vec!["20260102".to_owned(), "20260104".to_owned()]
        );
    }
}
