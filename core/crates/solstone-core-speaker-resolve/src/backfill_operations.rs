// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Append-only durable ledger for resumable speaker backfill operations.
//!
//! This module intentionally exposes only strict read/fold and append operations.
//! It contains no rewrite, compaction, or deletion primitive.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_journal_io::{
    AppendError, LockError, LockOptions, SegmentLayout, append_jsonl, hold_lock,
};
use thiserror::Error;

pub const BACKFILL_OPERATION_SCHEMA_VERSION: i64 = 3;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BackfillSegmentKey {
    pub day: String,
    pub stream_layout: SegmentLayout,
    pub stream: String,
    pub segment_key: String,
}

impl BackfillSegmentKey {
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "day": self.day,
            "stream_layout": self.stream_layout.as_str(),
            "stream": self.stream,
            "segment_key": self.segment_key,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillEventKind {
    Accepted,
    AttemptStarted,
    Prepared,
    CheckpointBatch,
    Checkpoint,
    AttemptFailed,
    Completed,
}

impl BackfillEventKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::AttemptStarted => "attempt_started",
            Self::Prepared => "prepared",
            Self::CheckpointBatch => "checkpoint_batch",
            Self::Checkpoint => "checkpoint",
            Self::AttemptFailed => "attempt_failed",
            Self::Completed => "completed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "accepted" => Self::Accepted,
            "attempt_started" => Self::AttemptStarted,
            "prepared" => Self::Prepared,
            "checkpoint_batch" => Self::CheckpointBatch,
            "checkpoint" => Self::Checkpoint,
            "attempt_failed" => Self::AttemptFailed,
            "completed" => Self::Completed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillCheckpointOutcome {
    Processed,
    Skipped,
    Error,
}

impl BackfillCheckpointOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Processed => "processed",
            Self::Skipped => "skipped",
            Self::Error => "error",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "processed" => Self::Processed,
            "skipped" => Self::Skipped,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillFailureStage {
    Scan,
    Executor,
    Panic,
}

impl BackfillFailureStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Executor => "executor",
            Self::Panic => "panic",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "scan" => Self::Scan,
            "executor" => Self::Executor,
            "panic" => Self::Panic,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackfillCheckpointResult {
    pub segment: BackfillSegmentKey,
    pub outcome: BackfillCheckpointOutcome,
    pub error_detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillOperationPayload {
    Accepted {
        commit: bool,
        reattribute: bool,
        accumulation: bool,
    },
    AttemptStarted {
        generation: u64,
    },
    Prepared {
        started_at: String,
        reattribute: bool,
        total_count: usize,
        segments: Vec<BackfillSegmentKey>,
        total_scanned: usize,
        selected: usize,
        protected_skipped: usize,
    },
    CheckpointBatch {
        generation: u64,
        results: Vec<BackfillCheckpointResult>,
    },
    Checkpoint {
        segment: BackfillSegmentKey,
        outcome: BackfillCheckpointOutcome,
        error_detail: Option<String>,
    },
    AttemptFailed {
        generation: u64,
        stage: BackfillFailureStage,
        detail: String,
    },
    Completed {
        completed_at: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillOperationEvent {
    pub schema_version: i64,
    pub event_id: String,
    pub operation_id: String,
    pub ts: String,
    pub payload: BackfillOperationPayload,
}

impl BackfillOperationEvent {
    #[must_use]
    pub fn event_kind(&self) -> BackfillEventKind {
        match self.payload {
            BackfillOperationPayload::Accepted { .. } => BackfillEventKind::Accepted,
            BackfillOperationPayload::AttemptStarted { .. } => BackfillEventKind::AttemptStarted,
            BackfillOperationPayload::Prepared { .. } => BackfillEventKind::Prepared,
            BackfillOperationPayload::CheckpointBatch { .. } => BackfillEventKind::CheckpointBatch,
            BackfillOperationPayload::Checkpoint { .. } => BackfillEventKind::Checkpoint,
            BackfillOperationPayload::AttemptFailed { .. } => BackfillEventKind::AttemptFailed,
            BackfillOperationPayload::Completed { .. } => BackfillEventKind::Completed,
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut row = Map::new();
        row.insert(
            "schema_version".to_owned(),
            Value::from(self.schema_version),
        );
        row.insert("event_id".to_owned(), Value::String(self.event_id.clone()));
        row.insert(
            "operation_id".to_owned(),
            Value::String(self.operation_id.clone()),
        );
        row.insert(
            "event_kind".to_owned(),
            Value::String(self.event_kind().as_str().to_owned()),
        );
        row.insert("ts".to_owned(), Value::String(self.ts.clone()));
        match &self.payload {
            BackfillOperationPayload::Accepted {
                commit,
                reattribute,
                accumulation,
            } => {
                row.insert("commit".to_owned(), Value::Bool(*commit));
                row.insert("reattribute".to_owned(), Value::Bool(*reattribute));
                row.insert("accumulation".to_owned(), Value::Bool(*accumulation));
            }
            BackfillOperationPayload::AttemptStarted { generation } => {
                row.insert("generation".to_owned(), Value::from(*generation));
            }
            BackfillOperationPayload::Prepared {
                started_at,
                reattribute,
                total_count,
                segments,
                total_scanned,
                selected,
                protected_skipped,
            } => {
                row.insert("started_at".to_owned(), Value::String(started_at.clone()));
                row.insert("reattribute".to_owned(), Value::Bool(*reattribute));
                row.insert("total_count".to_owned(), Value::from(*total_count));
                row.insert("total_scanned".to_owned(), Value::from(*total_scanned));
                row.insert("selected".to_owned(), Value::from(*selected));
                row.insert(
                    "protected_skipped".to_owned(),
                    Value::from(*protected_skipped),
                );
                row.insert(
                    "segments".to_owned(),
                    Value::Array(segments.iter().map(BackfillSegmentKey::to_json).collect()),
                );
            }
            BackfillOperationPayload::CheckpointBatch {
                generation,
                results,
            } => {
                row.insert("generation".to_owned(), Value::from(*generation));
                let results_json = results
                    .iter()
                    .map(|r| {
                        let mut item = Map::new();
                        item.insert("segment".to_owned(), r.segment.to_json());
                        item.insert(
                            "outcome".to_owned(),
                            Value::String(r.outcome.as_str().to_owned()),
                        );
                        if let Some(detail) = &r.error_detail {
                            item.insert("error_detail".to_owned(), Value::String(detail.clone()));
                        }
                        Value::Object(item)
                    })
                    .collect::<Vec<_>>();
                row.insert("results".to_owned(), Value::Array(results_json));
            }
            BackfillOperationPayload::Checkpoint {
                segment,
                outcome,
                error_detail,
            } => {
                row.insert("day".to_owned(), Value::String(segment.day.clone()));
                row.insert(
                    "stream_layout".to_owned(),
                    Value::String(segment.stream_layout.as_str().to_owned()),
                );
                row.insert("stream".to_owned(), Value::String(segment.stream.clone()));
                row.insert(
                    "segment_key".to_owned(),
                    Value::String(segment.segment_key.clone()),
                );
                row.insert(
                    "outcome".to_owned(),
                    Value::String(outcome.as_str().to_owned()),
                );
                if let Some(error_detail) = error_detail {
                    row.insert(
                        "error_detail".to_owned(),
                        Value::String(error_detail.clone()),
                    );
                }
            }
            BackfillOperationPayload::AttemptFailed {
                generation,
                stage,
                detail,
            } => {
                row.insert("generation".to_owned(), Value::from(*generation));
                row.insert("stage".to_owned(), Value::String(stage.as_str().to_owned()));
                row.insert("detail".to_owned(), Value::String(detail.clone()));
            }
            BackfillOperationPayload::Completed { completed_at } => {
                row.insert(
                    "completed_at".to_owned(),
                    Value::String(completed_at.clone()),
                );
            }
        }
        Value::Object(row)
    }
}

/// One validated ledger row retaining its original JSON for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillLedgerRow {
    pub event: BackfillOperationEvent,
    raw_json: String,
}

impl BackfillLedgerRow {
    fn parse(path: &Path, line: usize, raw_json: &str) -> Result<Self, BackfillOperationError> {
        let value: Value = serde_json::from_str(raw_json).map_err(|source| {
            BackfillOperationError::MalformedJson {
                path: path.to_path_buf(),
                line,
                source,
            }
        })?;
        if !value.is_object() {
            return Err(BackfillOperationError::NonObjectRow {
                path: path.to_path_buf(),
                line,
            });
        }
        let event =
            validate_backfill_row(&value).map_err(|source| BackfillOperationError::InvalidRow {
                path: path.to_path_buf(),
                line,
                source: Box::new(source),
            })?;
        Ok(Self {
            event,
            raw_json: raw_json.to_owned(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillStatusKind {
    NotFound,
    ActivePreparing,
    ActiveRunning,
    InactiveAccepted,
    InactiveInterrupted,
    ResumableMemberErrors,
    AttemptFailed,
    Done,
}

impl BackfillStatusKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::ActivePreparing => "active_preparing",
            Self::ActiveRunning => "active_running",
            Self::InactiveAccepted => "inactive_accepted",
            Self::InactiveInterrupted => "inactive_interrupted",
            Self::ResumableMemberErrors => "resumable_member_errors",
            Self::AttemptFailed => "attempt_failed",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillOperationState {
    pub operation_id: String,
    pub commit: bool,
    pub reattribute: bool,
    pub accumulation: bool,
    pub started_at: Option<String>,
    pub latest_generation: Option<u64>,
    pub total_scanned: usize,
    pub selected_count: usize,
    pub protected_skipped: usize,
    pub total_segments: usize,
    pub planned_segments: Vec<BackfillSegmentKey>,
    pub checkpointed_segments: BTreeMap<BackfillSegmentKey, BackfillCheckpointOutcome>,
    pub error_details: BTreeMap<BackfillSegmentKey, String>,
    pub pending_segments: Vec<BackfillSegmentKey>,
    pub attempt_failed: Option<(u64, BackfillFailureStage, String)>,
    pub completed: bool,
}

impl BackfillOperationState {
    #[must_use]
    pub fn status_kind(&self, has_active_token: bool) -> BackfillStatusKind {
        if self.completed && self.pending_segments.is_empty() && self.error_details.is_empty() {
            return BackfillStatusKind::Done;
        }
        if let Some((fail_gen, _, _)) = &self.attempt_failed
            && self.latest_generation == Some(*fail_gen)
        {
            return BackfillStatusKind::AttemptFailed;
        }
        if has_active_token {
            if self.latest_generation.is_none() && self.started_at.is_none() {
                BackfillStatusKind::ActivePreparing
            } else {
                BackfillStatusKind::ActiveRunning
            }
        } else if self.latest_generation.is_none() && self.started_at.is_none() {
            BackfillStatusKind::InactiveAccepted
        } else if !self.error_details.is_empty() {
            BackfillStatusKind::ResumableMemberErrors
        } else {
            BackfillStatusKind::InactiveInterrupted
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackfillOperationStatus {
    pub operation_id: String,
    pub status: BackfillStatusKind,
    pub commit: bool,
    pub reattribute: bool,
    pub accumulation: bool,
    pub latest_generation: Option<u64>,
    pub total_scanned: usize,
    pub selected_count: usize,
    pub protected_skipped: usize,
    pub total_count: usize,
    pub completed_count: usize,
    pub pending_count: usize,
    pub error_count: usize,
    pub error_segments: Vec<BackfillSegmentError>,
    pub failure_stage: Option<BackfillFailureStage>,
    pub failure_detail: Option<String>,
    pub done: bool,
}

/// A retryable segment failure retained in the append-only backfill ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackfillSegmentError {
    pub segment: BackfillSegmentKey,
    pub detail: String,
}

#[derive(Debug, Error)]
pub enum BackfillOperationError {
    #[error("failed to read backfill operation ledger {path}: {source}")]
    ReadIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed backfill operation JSONL at {path}:{line}: {source}")]
    MalformedJson {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("non-object backfill operation JSONL at {path}:{line}")]
    NonObjectRow { path: PathBuf, line: usize },
    #[error("invalid backfill operation row at {path}:{line}: {source}")]
    InvalidRow {
        path: PathBuf,
        line: usize,
        #[source]
        source: Box<Self>,
    },
    #[error("invalid schema_version")]
    InvalidSchemaVersion,
    #[error("missing or invalid {field}")]
    MissingOrInvalidField { field: &'static str },
    #[error("unknown event_kind: {event_kind}")]
    UnknownEventKind { event_kind: String },
    #[error("invalid checkpoint outcome: {outcome}")]
    InvalidCheckpointOutcome { outcome: String },
    #[error("invalid failure stage: {stage}")]
    InvalidFailureStage { stage: String },
    #[error("prepared total_count does not match segments")]
    PreparedTotalCountMismatch,
    #[error("prepared segment is not an object")]
    PreparedSegmentNotObject,
    #[error("operation must have at most one prepared row")]
    PreparedRowCount,
    #[error("checkpoint segment is absent from the prepared snapshot")]
    CheckpointOutsidePrepared,
    #[error("checkpoint batch exceeded maximum size 64")]
    CheckpointBatchTooLarge,
    #[error("failed to create backfill operation ledger directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("backfill operation ledger lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("backfill operation ledger append failed: {0}")]
    Append(#[from] AppendError),
}

/// Return the durable backfill-operation ledger path below a journal root.
#[must_use]
pub fn backfill_operations_path(journal_root: &Path) -> PathBuf {
    journal_root.join("speakers/backfill-operations.jsonl")
}

/// Validate one JSON row and return its typed event.
pub fn validate_backfill_row(
    row: &Value,
) -> Result<BackfillOperationEvent, BackfillOperationError> {
    let object = row
        .as_object()
        .ok_or(BackfillOperationError::MissingOrInvalidField { field: "row" })?;
    let schema_version = object
        .get("schema_version")
        .and_then(Value::as_i64)
        .ok_or(BackfillOperationError::InvalidSchemaVersion)?;
    if !(1..=3).contains(&schema_version) {
        return Err(BackfillOperationError::InvalidSchemaVersion);
    }
    let event_kind_text = required_string(object, "event_kind")?;
    let event_kind = BackfillEventKind::parse(&event_kind_text).ok_or(
        BackfillOperationError::UnknownEventKind {
            event_kind: event_kind_text,
        },
    )?;
    let event_id = required_string(object, "event_id")?;
    let operation_id = required_string(object, "operation_id")?;
    let ts = required_string(object, "ts")?;
    let payload = match event_kind {
        BackfillEventKind::Accepted => {
            let commit = object
                .get("commit")
                .and_then(Value::as_bool)
                .ok_or(BackfillOperationError::MissingOrInvalidField { field: "commit" })?;
            let reattribute = object.get("reattribute").and_then(Value::as_bool).ok_or(
                BackfillOperationError::MissingOrInvalidField {
                    field: "reattribute",
                },
            )?;
            let accumulation = object.get("accumulation").and_then(Value::as_bool).ok_or(
                BackfillOperationError::MissingOrInvalidField {
                    field: "accumulation",
                },
            )?;
            if !commit && accumulation {
                return Err(BackfillOperationError::MissingOrInvalidField {
                    field: "accumulation",
                });
            }
            BackfillOperationPayload::Accepted {
                commit,
                reattribute,
                accumulation,
            }
        }
        BackfillEventKind::AttemptStarted => {
            let generation = object.get("generation").and_then(Value::as_u64).ok_or(
                BackfillOperationError::MissingOrInvalidField {
                    field: "generation",
                },
            )?;
            BackfillOperationPayload::AttemptStarted { generation }
        }
        BackfillEventKind::Prepared => {
            let started_at = required_string(object, "started_at")?;
            let reattribute = object.get("reattribute").and_then(Value::as_bool).ok_or(
                BackfillOperationError::MissingOrInvalidField {
                    field: "reattribute",
                },
            )?;
            let total_count = object
                .get("total_count")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(BackfillOperationError::MissingOrInvalidField {
                    field: "total_count",
                })?;
            let segments = object
                .get("segments")
                .and_then(Value::as_array)
                .ok_or(BackfillOperationError::MissingOrInvalidField { field: "segments" })?
                .iter()
                .map(|v| parse_segment(v, schema_version))
                .collect::<Result<Vec<_>, _>>()?;
            if total_count != segments.len() {
                return Err(BackfillOperationError::PreparedTotalCountMismatch);
            }
            let total_scanned = object
                .get("total_scanned")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(total_count);
            let selected = object
                .get("selected")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(total_count);
            let protected_skipped = object
                .get("protected_skipped")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(0);
            BackfillOperationPayload::Prepared {
                started_at,
                reattribute,
                total_count,
                segments,
                total_scanned,
                selected,
                protected_skipped,
            }
        }
        BackfillEventKind::CheckpointBatch => {
            let generation = object.get("generation").and_then(Value::as_u64).ok_or(
                BackfillOperationError::MissingOrInvalidField {
                    field: "generation",
                },
            )?;
            let raw_results = object
                .get("results")
                .and_then(Value::as_array)
                .ok_or(BackfillOperationError::MissingOrInvalidField { field: "results" })?;
            if raw_results.is_empty() || raw_results.len() > 64 {
                return Err(BackfillOperationError::CheckpointBatchTooLarge);
            }
            let mut results = Vec::with_capacity(raw_results.len());
            for item in raw_results {
                let item_obj = item
                    .as_object()
                    .ok_or(BackfillOperationError::MissingOrInvalidField { field: "result" })?;
                let segment_val = item_obj
                    .get("segment")
                    .ok_or(BackfillOperationError::MissingOrInvalidField { field: "segment" })?;
                let segment = parse_segment(segment_val, schema_version)?;
                let outcome_text = required_string(item_obj, "outcome")?;
                let outcome = BackfillCheckpointOutcome::parse(&outcome_text).ok_or(
                    BackfillOperationError::InvalidCheckpointOutcome {
                        outcome: outcome_text,
                    },
                )?;
                let error_detail = item_obj
                    .get("error_detail")
                    .and_then(Value::as_str)
                    .filter(|detail| !detail.is_empty())
                    .map(str::to_owned);
                if outcome != BackfillCheckpointOutcome::Error && error_detail.is_some() {
                    return Err(BackfillOperationError::MissingOrInvalidField {
                        field: "error_detail",
                    });
                }
                results.push(BackfillCheckpointResult {
                    segment,
                    outcome,
                    error_detail,
                });
            }
            BackfillOperationPayload::CheckpointBatch {
                generation,
                results,
            }
        }
        BackfillEventKind::Checkpoint => {
            let segment = parse_segment(&Value::Object(object.clone()), schema_version)?;
            let outcome_text = required_string(object, "outcome")?;
            let outcome = BackfillCheckpointOutcome::parse(&outcome_text).ok_or(
                BackfillOperationError::InvalidCheckpointOutcome {
                    outcome: outcome_text,
                },
            )?;
            let error_detail = object
                .get("error_detail")
                .and_then(Value::as_str)
                .filter(|detail| !detail.is_empty())
                .map(str::to_owned);
            if outcome != BackfillCheckpointOutcome::Error && error_detail.is_some() {
                return Err(BackfillOperationError::MissingOrInvalidField {
                    field: "error_detail",
                });
            }
            BackfillOperationPayload::Checkpoint {
                segment,
                outcome,
                error_detail,
            }
        }
        BackfillEventKind::AttemptFailed => {
            let generation = object.get("generation").and_then(Value::as_u64).ok_or(
                BackfillOperationError::MissingOrInvalidField {
                    field: "generation",
                },
            )?;
            let stage_text = required_string(object, "stage")?;
            let stage = BackfillFailureStage::parse(&stage_text)
                .ok_or(BackfillOperationError::InvalidFailureStage { stage: stage_text })?;
            let detail = required_string(object, "detail")?;
            BackfillOperationPayload::AttemptFailed {
                generation,
                stage,
                detail,
            }
        }
        BackfillEventKind::Completed => BackfillOperationPayload::Completed {
            completed_at: required_string(object, "completed_at")?,
        },
    };
    Ok(BackfillOperationEvent {
        schema_version,
        event_id,
        operation_id,
        ts,
        payload,
    })
}

/// Strictly load every complete newline-terminated JSONL row.
/// Unterminated final bytes are ignored without failing.
pub fn load_backfill_operations(
    path: &Path,
) -> Result<Vec<BackfillLedgerRow>, BackfillOperationError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path).map_err(|source| BackfillOperationError::ReadIo {
        path: path.to_path_buf(),
        source,
    })?;
    let complete_contents = match contents.rfind('\n') {
        Some(idx) => &contents[..=idx],
        None => "",
    };
    let mut rows = Vec::new();
    let mut line_number = 0;
    for line in complete_contents.split('\n') {
        line_number += 1;
        if line.trim().is_empty() {
            continue;
        }
        rows.push(BackfillLedgerRow::parse(path, line_number, line)?);
    }
    Ok(rows)
}

/// Truncate any unterminated torn tail from the ledger file under lock.
pub fn truncate_torn_tail_if_present(path: &Path) -> Result<(), BackfillOperationError> {
    if !path.exists() {
        return Ok(());
    }
    let bytes = fs::read(path).map_err(|source| BackfillOperationError::ReadIo {
        path: path.to_path_buf(),
        source,
    })?;
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        return Ok(());
    }
    let last_newline = bytes.iter().rposition(|&b| b == b'\n');
    let new_len = match last_newline {
        Some(idx) => (idx + 1) as u64,
        None => 0,
    };
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|source| BackfillOperationError::ReadIo {
            path: path.to_path_buf(),
            source,
        })?;
    file.set_len(new_len)
        .map_err(|source| BackfillOperationError::ReadIo {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

/// Append one validated row assuming the ledger lock is already held.
pub fn append_backfill_event_locked(
    path: &Path,
    event: &BackfillOperationEvent,
) -> Result<(), BackfillOperationError> {
    let value = event.to_json();
    validate_backfill_row(&value)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| BackfillOperationError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    truncate_torn_tail_if_present(path)?;
    append_jsonl(path, &value)?;
    Ok(())
}

/// Append one validated row under lock, truncating any torn tail first.
pub fn append_backfill_event(
    path: &Path,
    event: &BackfillOperationEvent,
) -> Result<(), BackfillOperationError> {
    let _lock = hold_lock(path, LockOptions::default())?;
    append_backfill_event_locked(path, event)
}

/// Fold one operation to its state. Rows for other operations are ignored.
pub fn fold_backfill_operation(
    rows: &[BackfillLedgerRow],
    operation_id: &str,
) -> Result<Option<BackfillOperationState>, BackfillOperationError> {
    let events = rows
        .iter()
        .filter(|row| row.event.operation_id == operation_id)
        .map(|row| &row.event)
        .collect::<Vec<_>>();
    if events.is_empty() {
        return Ok(None);
    }

    let mut commit = true;
    let mut reattribute = false;
    let mut accumulation = false;
    let mut has_accepted = false;
    let mut latest_generation = None;
    let mut prepared_data = None;
    let mut checkpointed_segments = BTreeMap::new();
    let mut error_details = BTreeMap::new();
    let mut attempt_failed = None;
    let mut completed = false;

    for event in events {
        match &event.payload {
            BackfillOperationPayload::Accepted {
                commit: c,
                reattribute: r,
                accumulation: a,
            } => {
                commit = *c;
                reattribute = *r;
                accumulation = *a;
                has_accepted = true;
            }
            BackfillOperationPayload::AttemptStarted { generation } => {
                latest_generation = Some(*generation);
            }
            BackfillOperationPayload::Prepared {
                started_at,
                reattribute: prep_reattribute,
                total_count,
                segments,
                total_scanned,
                selected,
                protected_skipped,
            } => {
                if prepared_data.is_some() {
                    return Err(BackfillOperationError::PreparedRowCount);
                }
                if !has_accepted {
                    reattribute = *prep_reattribute;
                }
                prepared_data = Some((
                    started_at.clone(),
                    *total_count,
                    segments.clone(),
                    *total_scanned,
                    *selected,
                    *protected_skipped,
                ));
            }
            BackfillOperationPayload::CheckpointBatch {
                generation,
                results,
            } => {
                latest_generation = Some(latest_generation.unwrap_or(0).max(*generation));
                let planned = prepared_data
                    .as_ref()
                    .map(|(_, _, segs, _, _, _)| segs.iter().cloned().collect::<BTreeSet<_>>())
                    .unwrap_or_default();
                for r in results {
                    if !planned.is_empty() && !planned.contains(&r.segment) {
                        return Err(BackfillOperationError::CheckpointOutsidePrepared);
                    }
                    checkpointed_segments.insert(r.segment.clone(), r.outcome);
                    if r.outcome == BackfillCheckpointOutcome::Error {
                        error_details.insert(
                            r.segment.clone(),
                            r.error_detail.clone().unwrap_or_else(|| {
                                "checkpoint did not retain error detail".to_owned()
                            }),
                        );
                    } else {
                        error_details.remove(&r.segment);
                    }
                }
            }
            BackfillOperationPayload::Checkpoint {
                segment,
                outcome,
                error_detail,
            } => {
                let planned = prepared_data
                    .as_ref()
                    .map(|(_, _, segs, _, _, _)| segs.iter().cloned().collect::<BTreeSet<_>>())
                    .unwrap_or_default();
                if !planned.is_empty() && !planned.contains(segment) {
                    return Err(BackfillOperationError::CheckpointOutsidePrepared);
                }
                checkpointed_segments.insert(segment.clone(), *outcome);
                if *outcome == BackfillCheckpointOutcome::Error {
                    error_details.insert(
                        segment.clone(),
                        error_detail.clone().unwrap_or_else(|| {
                            "legacy checkpoint did not retain error detail".to_owned()
                        }),
                    );
                } else {
                    error_details.remove(segment);
                }
            }
            BackfillOperationPayload::AttemptFailed {
                generation,
                stage,
                detail,
            } => {
                latest_generation = Some(latest_generation.unwrap_or(0).max(*generation));
                attempt_failed = Some((*generation, *stage, detail.clone()));
            }
            BackfillOperationPayload::Completed { .. } => {
                completed = true;
            }
        }
    }

    let (
        started_at,
        total_segments,
        planned_segments,
        total_scanned,
        selected_count,
        protected_skipped,
    ) = match prepared_data {
        Some((sa, tc, segs, ts, sel, ps)) => (Some(sa), tc, segs, ts, sel, ps),
        None => (None, 0, Vec::new(), 0, 0, 0),
    };

    let pending_segments = planned_segments
        .iter()
        .filter(|segment| {
            !checkpointed_segments.contains_key(*segment)
                || checkpointed_segments.get(*segment) == Some(&BackfillCheckpointOutcome::Error)
        })
        .cloned()
        .collect();

    Ok(Some(BackfillOperationState {
        operation_id: operation_id.to_owned(),
        commit,
        reattribute,
        accumulation,
        started_at,
        latest_generation,
        total_scanned,
        selected_count,
        protected_skipped,
        total_segments,
        planned_segments,
        checkpointed_segments,
        error_details,
        pending_segments,
        attempt_failed,
        completed,
    }))
}

/// Return detailed status without mutating the ledger.
pub fn backfill_operation_status(
    rows: &[BackfillLedgerRow],
    operation_id: &str,
    has_active_token: bool,
) -> Result<Option<BackfillOperationStatus>, BackfillOperationError> {
    let Some(state) = fold_backfill_operation(rows, operation_id)? else {
        return Ok(None);
    };
    let status = state.status_kind(has_active_token);
    let done = status == BackfillStatusKind::Done;
    let (failure_stage, failure_detail) = match &state.attempt_failed {
        Some((_, stage, detail)) => (Some(*stage), Some(detail.clone())),
        None => (None, None),
    };
    Ok(Some(BackfillOperationStatus {
        operation_id: state.operation_id,
        status,
        commit: state.commit,
        reattribute: state.reattribute,
        accumulation: state.accumulation,
        latest_generation: state.latest_generation,
        total_scanned: state.total_scanned,
        selected_count: state.selected_count,
        protected_skipped: state.protected_skipped,
        total_count: state.total_segments,
        completed_count: state
            .checkpointed_segments
            .values()
            .filter(|outcome| **outcome != BackfillCheckpointOutcome::Error)
            .count(),
        pending_count: state.pending_segments.len(),
        error_count: state.error_details.len(),
        error_segments: state
            .error_details
            .iter()
            .map(|(segment, detail)| BackfillSegmentError {
                segment: segment.clone(),
                detail: detail.clone(),
            })
            .collect(),
        failure_stage,
        failure_detail,
        done,
    }))
}

fn required_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, BackfillOperationError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(BackfillOperationError::MissingOrInvalidField { field })
}

fn parse_segment(
    value: &Value,
    schema_version: i64,
) -> Result<BackfillSegmentKey, BackfillOperationError> {
    let object = value
        .as_object()
        .ok_or(BackfillOperationError::PreparedSegmentNotObject)?;
    let day = required_string(object, "day")?;
    let stream = required_string(object, "stream")?;
    let segment_key = required_string(object, "segment_key")?;
    let stream_layout = if schema_version >= 2 {
        let layout_str = required_string(object, "stream_layout")?;
        match layout_str.as_str() {
            "direct" => SegmentLayout::Direct,
            "named" => SegmentLayout::Named,
            _ => {
                return Err(BackfillOperationError::MissingOrInvalidField {
                    field: "stream_layout",
                });
            }
        }
    } else if let Some(layout_str) = object.get("stream_layout").and_then(Value::as_str) {
        match layout_str {
            "direct" => SegmentLayout::Direct,
            "named" => SegmentLayout::Named,
            _ => {
                return Err(BackfillOperationError::MissingOrInvalidField {
                    field: "stream_layout",
                });
            }
        }
    } else if stream == "_default" {
        SegmentLayout::Direct
    } else {
        SegmentLayout::Named
    };
    Ok(BackfillSegmentKey {
        day,
        stream_layout,
        stream,
        segment_key,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-backfill-ops-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn ledger(&self) -> PathBuf {
            backfill_operations_path(&self.0)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn segment(index: usize) -> BackfillSegmentKey {
        BackfillSegmentKey {
            day: "20260808".to_owned(),
            stream_layout: SegmentLayout::Named,
            stream: "mic".to_owned(),
            segment_key: format!("120{index:02}_300"),
        }
    }

    fn accepted_event() -> BackfillOperationEvent {
        BackfillOperationEvent {
            schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
            event_id: "accepted".to_owned(),
            operation_id: "bfop_test".to_owned(),
            ts: "2026-08-08T00:00:00Z".to_owned(),
            payload: BackfillOperationPayload::Accepted {
                commit: true,
                reattribute: false,
                accumulation: false,
            },
        }
    }

    fn attempt_started_event(generation: u64) -> BackfillOperationEvent {
        BackfillOperationEvent {
            schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
            event_id: format!("attempt-{generation}"),
            operation_id: "bfop_test".to_owned(),
            ts: "2026-08-08T00:00:01Z".to_owned(),
            payload: BackfillOperationPayload::AttemptStarted { generation },
        }
    }

    fn prepared_event() -> BackfillOperationEvent {
        BackfillOperationEvent {
            schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
            event_id: "prepared".to_owned(),
            operation_id: "bfop_test".to_owned(),
            ts: "2026-08-08T00:00:02Z".to_owned(),
            payload: BackfillOperationPayload::Prepared {
                started_at: "2026-08-08T00:00:00Z".to_owned(),
                reattribute: false,
                total_count: 5,
                segments: (0..5).map(segment).collect(),
                total_scanned: 10,
                selected: 5,
                protected_skipped: 5,
            },
        }
    }

    fn batch_checkpoint_event(
        generation: u64,
        results: Vec<BackfillCheckpointResult>,
    ) -> BackfillOperationEvent {
        BackfillOperationEvent {
            schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
            event_id: format!("batch-{generation}"),
            operation_id: "bfop_test".to_owned(),
            ts: "2026-08-08T00:00:03Z".to_owned(),
            payload: BackfillOperationPayload::CheckpointBatch {
                generation,
                results,
            },
        }
    }

    #[allow(dead_code)]
    fn completed_event() -> BackfillOperationEvent {
        BackfillOperationEvent {
            schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
            event_id: "completed".to_owned(),
            operation_id: "bfop_test".to_owned(),
            ts: "2026-08-08T01:00:00Z".to_owned(),
            payload: BackfillOperationPayload::Completed {
                completed_at: "2026-08-08T01:00:00Z".to_owned(),
            },
        }
    }

    #[test]
    fn malformed_row_fails_loudly_on_load_and_invalid_event_fails_validation() {
        let temporary = TempDir::new();
        let path = temporary.ledger();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{not json}\n").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            load_backfill_operations(&path),
            Err(BackfillOperationError::MalformedJson { line: 1, .. })
        ));

        // Appending an event with invalid schema/payload fails validation before write
        let mut invalid_event = prepared_event();
        invalid_event.schema_version = 999;
        assert!(matches!(
            append_backfill_event(&path, &invalid_event),
            Err(BackfillOperationError::InvalidSchemaVersion)
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn torn_tail_is_ignored_on_read_and_truncated_on_append() {
        let temporary = TempDir::new();
        let path = temporary.ledger();
        append_backfill_event(&path, &accepted_event()).unwrap();
        let valid_len = fs::read(&path).unwrap().len();

        // Write an incomplete torn tail without trailing newline
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut file, b"{\"schema_version\":3,\"event_id\":\"incomp")
            .unwrap();
        drop(file);

        // Read path ignores the torn tail
        let rows = load_backfill_operations(&path).unwrap();
        assert_eq!(rows.len(), 1);

        // Append truncates the torn tail and successfully writes
        append_backfill_event(&path, &attempt_started_event(1)).unwrap();
        let rows = load_backfill_operations(&path).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].event.event_id, "accepted");
        assert_eq!(rows[1].event.event_id, "attempt-1");

        let raw = fs::read(&path).unwrap();
        assert!(raw.len() > valid_len);
    }

    #[test]
    fn batch_checkpoint_folds_correctly_with_overrides() {
        let temporary = TempDir::new();
        let path = temporary.ledger();
        append_backfill_event(&path, &accepted_event()).unwrap();
        append_backfill_event(&path, &attempt_started_event(1)).unwrap();
        append_backfill_event(&path, &prepared_event()).unwrap();

        // Checkpoint 0 processed, 1 error
        let results1 = vec![
            BackfillCheckpointResult {
                segment: segment(0),
                outcome: BackfillCheckpointOutcome::Processed,
                error_detail: None,
            },
            BackfillCheckpointResult {
                segment: segment(1),
                outcome: BackfillCheckpointOutcome::Error,
                error_detail: Some("transient error".to_owned()),
            },
        ];
        append_backfill_event(&path, &batch_checkpoint_event(1, results1)).unwrap();

        let rows = load_backfill_operations(&path).unwrap();
        let state = fold_backfill_operation(&rows, "bfop_test")
            .unwrap()
            .unwrap();
        assert_eq!(state.pending_segments.len(), 4);
        assert_eq!(state.error_details.len(), 1);
        assert_eq!(
            state.status_kind(false),
            BackfillStatusKind::ResumableMemberErrors
        );
        assert_eq!(state.status_kind(true), BackfillStatusKind::ActiveRunning);

        // In generation 2, retry segment 1 successfully
        append_backfill_event(&path, &attempt_started_event(2)).unwrap();
        let results2 = vec![BackfillCheckpointResult {
            segment: segment(1),
            outcome: BackfillCheckpointOutcome::Processed,
            error_detail: None,
        }];
        append_backfill_event(&path, &batch_checkpoint_event(2, results2)).unwrap();

        let rows = load_backfill_operations(&path).unwrap();
        let state = fold_backfill_operation(&rows, "bfop_test")
            .unwrap()
            .unwrap();
        assert_eq!(state.pending_segments.len(), 3);
        assert_eq!(state.error_details.len(), 0);
        assert_eq!(
            state.status_kind(false),
            BackfillStatusKind::InactiveInterrupted
        );
    }

    #[test]
    fn attempt_failed_reports_status_and_detail() {
        let temporary = TempDir::new();
        let path = temporary.ledger();
        append_backfill_event(&path, &accepted_event()).unwrap();
        append_backfill_event(&path, &attempt_started_event(1)).unwrap();
        let fail_event = BackfillOperationEvent {
            schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
            event_id: "fail-1".to_owned(),
            operation_id: "bfop_test".to_owned(),
            ts: "2026-08-08T00:00:05Z".to_owned(),
            payload: BackfillOperationPayload::AttemptFailed {
                generation: 1,
                stage: BackfillFailureStage::Scan,
                detail: "labels gap detected".to_owned(),
            },
        };
        append_backfill_event(&path, &fail_event).unwrap();

        let rows = load_backfill_operations(&path).unwrap();
        let status = backfill_operation_status(&rows, "bfop_test", false)
            .unwrap()
            .unwrap();
        assert_eq!(status.status, BackfillStatusKind::AttemptFailed);
        assert_eq!(status.failure_stage, Some(BackfillFailureStage::Scan));
        assert_eq!(
            status.failure_detail.as_deref(),
            Some("labels gap detected")
        );
    }

    #[test]
    fn historical_v1_folds_with_implied_flags() {
        let temporary = TempDir::new();
        let path = temporary.ledger();
        let v1_prepared = br#"{"schema_version":1,"event_id":"bfop-legacy:prepared","operation_id":"bfop-legacy","event_kind":"prepared","ts":"2026-08-08T00:00:00Z","started_at":"2026-08-08T00:00:00Z","reattribute":false,"total_count":2,"segments":[{"day":"20260808","stream":"mic","segment_key":"120000_300"},{"day":"20260808","stream":"mic","segment_key":"120500_300"}]}
"#;
        let v1_checkpoint = br#"{"schema_version":1,"event_id":"bfop-legacy:cp1","operation_id":"bfop-legacy","event_kind":"checkpoint","ts":"2026-08-08T00:00:01Z","day":"20260808","stream":"mic","segment_key":"120000_300","outcome":"processed"}
"#;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            [v1_prepared.as_slice(), v1_checkpoint.as_slice()].concat(),
        )
        .unwrap();

        let rows = load_backfill_operations(&path).unwrap();
        let status = backfill_operation_status(&rows, "bfop-legacy", false)
            .unwrap()
            .unwrap();
        assert_eq!(status.status, BackfillStatusKind::InactiveInterrupted);
        assert!(status.commit);
        assert!(!status.reattribute);
        assert!(!status.accumulation);
        assert_eq!(status.total_count, 2);
        assert_eq!(status.completed_count, 1);
        assert_eq!(status.pending_count, 1);
    }

    #[test]
    fn torn_tails_for_all_six_event_kinds() {
        let events: Vec<BackfillOperationEvent> = vec![
            accepted_event(),
            attempt_started_event(1),
            prepared_event(),
            batch_checkpoint_event(
                1,
                vec![BackfillCheckpointResult {
                    segment: segment(0),
                    outcome: BackfillCheckpointOutcome::Processed,
                    error_detail: None,
                }],
            ),
            BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: "fail-1".to_owned(),
                operation_id: "bfop_test".to_owned(),
                ts: "2026-08-08T00:00:04Z".to_owned(),
                payload: BackfillOperationPayload::AttemptFailed {
                    generation: 1,
                    stage: BackfillFailureStage::Scan,
                    detail: "scan failed".to_owned(),
                },
            },
            completed_event(),
        ];

        for event in events {
            let temp = TempDir::new();
            let path = temp.ledger();

            // Append base valid row
            append_backfill_event(&path, &accepted_event()).unwrap();
            let base_len = fs::read(&path).unwrap().len();

            // 1. Unterminated valid JSON (missing newline)
            let json_text = serde_json::to_string(&event.to_json()).unwrap();
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            std::io::Write::write_all(&mut file, json_text.as_bytes()).unwrap();
            drop(file);

            let rows = load_backfill_operations(&path).unwrap();
            assert_eq!(rows.len(), 1); // unterminated row ignored

            // 2. Unterminated malformed slice
            let raw = fs::read(&path).unwrap();
            let truncated_len = base_len + json_text.len() / 2;
            fs::write(&path, &raw[..truncated_len.min(raw.len())]).unwrap();

            let rows2 = load_backfill_operations(&path).unwrap();
            assert_eq!(rows2.len(), 1); // partial slice ignored

            // Next writer truncates and succeeds
            append_backfill_event(&path, &attempt_started_event(1)).unwrap();
            let rows3 = load_backfill_operations(&path).unwrap();
            assert_eq!(rows3.len(), 2);

            // 3. Newline-terminated malformed hard-fails
            let mut file3 = fs::OpenOptions::new().append(true).open(&path).unwrap();
            std::io::Write::write_all(&mut file3, b"{\"malformed json with newline\"\n").unwrap();
            drop(file3);

            assert!(matches!(
                load_backfill_operations(&path),
                Err(BackfillOperationError::MalformedJson { .. })
            ));
        }
    }

    #[test]
    fn batch_checkpoint_65_and_193_members_ceil_boundary() {
        // Test N = 65 -> ceil(65 / 64) = 2 batches
        let temp65 = TempDir::new();
        let path65 = temp65.ledger();
        append_backfill_event(&path65, &accepted_event()).unwrap();
        append_backfill_event(&path65, &attempt_started_event(1)).unwrap();

        let segments65 = (0..65)
            .map(|i| BackfillSegmentKey {
                day: "20260808".to_owned(),
                stream_layout: SegmentLayout::Named,
                stream: "mic".to_owned(),
                segment_key: format!("120000_{i:03}"),
            })
            .collect::<Vec<_>>();

        append_backfill_event(
            &path65,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: "prep65".to_owned(),
                operation_id: "bfop_test".to_owned(),
                ts: "2026-08-08T00:00:02Z".to_owned(),
                payload: BackfillOperationPayload::Prepared {
                    started_at: "2026-08-08T00:00:00Z".to_owned(),
                    reattribute: false,
                    total_count: 65,
                    segments: segments65.clone(),
                    total_scanned: 65,
                    selected: 65,
                    protected_skipped: 0,
                },
            },
        )
        .unwrap();

        let batch1_results = segments65[..64]
            .iter()
            .cloned()
            .map(|s| BackfillCheckpointResult {
                segment: s,
                outcome: BackfillCheckpointOutcome::Processed,
                error_detail: None,
            })
            .collect::<Vec<_>>();
        append_backfill_event(&path65, &batch_checkpoint_event(1, batch1_results)).unwrap();

        let batch2_results = segments65[64..]
            .iter()
            .cloned()
            .map(|s| BackfillCheckpointResult {
                segment: s,
                outcome: BackfillCheckpointOutcome::Processed,
                error_detail: None,
            })
            .collect::<Vec<_>>();
        append_backfill_event(&path65, &batch_checkpoint_event(1, batch2_results)).unwrap();

        let rows65 = load_backfill_operations(&path65).unwrap();
        let checkpoint_rows = rows65
            .iter()
            .filter(|r| {
                matches!(
                    r.event.payload,
                    BackfillOperationPayload::CheckpointBatch { .. }
                )
            })
            .count();
        assert_eq!(checkpoint_rows, 2);

        let state65 = fold_backfill_operation(&rows65, "bfop_test")
            .unwrap()
            .unwrap();
        assert_eq!(state65.checkpointed_segments.len(), 65);
        assert_eq!(state65.pending_segments.len(), 0);

        // Test N = 193 -> ceil(193 / 64) = 4 batches (64, 64, 64, 1)
        let temp193 = TempDir::new();
        let path193 = temp193.ledger();
        append_backfill_event(&path193, &accepted_event()).unwrap();
        append_backfill_event(&path193, &attempt_started_event(1)).unwrap();

        let segments193 = (0..193)
            .map(|i| BackfillSegmentKey {
                day: "20260808".to_owned(),
                stream_layout: SegmentLayout::Named,
                stream: "mic".to_owned(),
                segment_key: format!("120000_{i:03}"),
            })
            .collect::<Vec<_>>();

        append_backfill_event(
            &path193,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: "prep193".to_owned(),
                operation_id: "bfop_test".to_owned(),
                ts: "2026-08-08T00:00:02Z".to_owned(),
                payload: BackfillOperationPayload::Prepared {
                    started_at: "2026-08-08T00:00:00Z".to_owned(),
                    reattribute: false,
                    total_count: 193,
                    segments: segments193.clone(),
                    total_scanned: 193,
                    selected: 193,
                    protected_skipped: 0,
                },
            },
        )
        .unwrap();

        for chunk in segments193.chunks(64) {
            let res = chunk
                .iter()
                .cloned()
                .map(|s| BackfillCheckpointResult {
                    segment: s,
                    outcome: BackfillCheckpointOutcome::Processed,
                    error_detail: None,
                })
                .collect::<Vec<_>>();
            append_backfill_event(&path193, &batch_checkpoint_event(1, res)).unwrap();
        }

        let rows193 = load_backfill_operations(&path193).unwrap();
        let batch_count = rows193
            .iter()
            .filter(|r| {
                matches!(
                    r.event.payload,
                    BackfillOperationPayload::CheckpointBatch { .. }
                )
            })
            .count();
        assert_eq!(batch_count, 4);

        let state193 = fold_backfill_operation(&rows193, "bfop_test")
            .unwrap()
            .unwrap();
        assert_eq!(state193.checkpointed_segments.len(), 193);
        assert_eq!(state193.pending_segments.len(), 0);
    }

    #[test]
    fn table_driven_status_fixtures_for_all_kinds_paired_tokens() {
        let seg = segment(0);

        // 1. InactiveAccepted vs ActivePreparing
        let state_accepted = BackfillOperationState {
            operation_id: "op1".to_owned(),
            commit: true,
            reattribute: false,
            accumulation: false,
            started_at: None,
            latest_generation: None,
            total_scanned: 0,
            selected_count: 0,
            protected_skipped: 0,
            total_segments: 0,
            planned_segments: vec![],
            checkpointed_segments: BTreeMap::new(),
            error_details: BTreeMap::new(),
            pending_segments: vec![],
            attempt_failed: None,
            completed: false,
        };
        assert_eq!(
            state_accepted.status_kind(false),
            BackfillStatusKind::InactiveAccepted
        );
        assert_eq!(
            state_accepted.status_kind(true),
            BackfillStatusKind::ActivePreparing
        );

        // 2. ActiveRunning vs InactiveInterrupted
        let mut state_running = state_accepted.clone();
        state_running.latest_generation = Some(1);
        state_running.started_at = Some("2026-08-08T00:00:00Z".to_owned());
        state_running.planned_segments = vec![seg.clone()];
        state_running.pending_segments = vec![seg.clone()];
        assert_eq!(
            state_running.status_kind(true),
            BackfillStatusKind::ActiveRunning
        );
        assert_eq!(
            state_running.status_kind(false),
            BackfillStatusKind::InactiveInterrupted
        );

        // 3. ResumableMemberErrors
        let mut state_errors = state_running.clone();
        state_errors
            .error_details
            .insert(seg.clone(), "failed".to_owned());
        assert_eq!(
            state_errors.status_kind(false),
            BackfillStatusKind::ResumableMemberErrors
        );
        assert_eq!(
            state_errors.status_kind(true),
            BackfillStatusKind::ActiveRunning
        );

        // 4. AttemptFailed
        let mut state_failed = state_running.clone();
        state_failed.attempt_failed = Some((1, BackfillFailureStage::Scan, "scan fail".to_owned()));
        assert_eq!(
            state_failed.status_kind(false),
            BackfillStatusKind::AttemptFailed
        );
        assert_eq!(
            state_failed.status_kind(true),
            BackfillStatusKind::AttemptFailed
        );

        // 5. Done
        let mut state_done = state_running.clone();
        state_done.pending_segments.clear();
        state_done.error_details.clear();
        state_done.completed = true;
        assert_eq!(state_done.status_kind(false), BackfillStatusKind::Done);
        assert_eq!(state_done.status_kind(true), BackfillStatusKind::Done);

        // Assert distinctness
        assert_ne!(
            BackfillStatusKind::InactiveAccepted,
            BackfillStatusKind::InactiveInterrupted
        );
        assert_ne!(
            BackfillStatusKind::InactiveInterrupted,
            BackfillStatusKind::ResumableMemberErrors
        );
        assert_ne!(
            BackfillStatusKind::ResumableMemberErrors,
            BackfillStatusKind::AttemptFailed
        );
    }

    #[test]
    fn suffix_siblings_and_stream_layout_twins_remain_distinct() {
        let seg_direct_default = BackfillSegmentKey {
            day: "20260808".to_owned(),
            stream_layout: SegmentLayout::Direct,
            stream: "_default".to_owned(),
            segment_key: "120000_300".to_owned(),
        };
        let seg_named_default = BackfillSegmentKey {
            day: "20260808".to_owned(),
            stream_layout: SegmentLayout::Named,
            stream: "_default".to_owned(),
            segment_key: "120000_300".to_owned(),
        };
        let seg_mic = BackfillSegmentKey {
            day: "20260808".to_owned(),
            stream_layout: SegmentLayout::Named,
            stream: "mic".to_owned(),
            segment_key: "120000_300".to_owned(),
        };
        let seg_mic_suffix = BackfillSegmentKey {
            day: "20260808".to_owned(),
            stream_layout: SegmentLayout::Named,
            stream: "mic".to_owned(),
            segment_key: "120000_300_extra".to_owned(),
        };

        let temporary = TempDir::new();
        let path = temporary.ledger();
        append_backfill_event(&path, &accepted_event()).unwrap();
        append_backfill_event(&path, &attempt_started_event(1)).unwrap();
        append_backfill_event(
            &path,
            &BackfillOperationEvent {
                schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
                event_id: "prep".to_owned(),
                operation_id: "bfop_test".to_owned(),
                ts: "2026-08-08T00:00:02Z".to_owned(),
                payload: BackfillOperationPayload::Prepared {
                    started_at: "2026-08-08T00:00:00Z".to_owned(),
                    reattribute: false,
                    total_count: 4,
                    segments: vec![
                        seg_direct_default.clone(),
                        seg_named_default.clone(),
                        seg_mic.clone(),
                        seg_mic_suffix.clone(),
                    ],
                    total_scanned: 4,
                    selected: 4,
                    protected_skipped: 0,
                },
            },
        )
        .unwrap();

        // Checkpoint 2 of the 4
        append_backfill_event(
            &path,
            &batch_checkpoint_event(
                1,
                vec![
                    BackfillCheckpointResult {
                        segment: seg_direct_default.clone(),
                        outcome: BackfillCheckpointOutcome::Processed,
                        error_detail: None,
                    },
                    BackfillCheckpointResult {
                        segment: seg_mic.clone(),
                        outcome: BackfillCheckpointOutcome::Processed,
                        error_detail: None,
                    },
                ],
            ),
        )
        .unwrap();

        let rows = load_backfill_operations(&path).unwrap();
        let state = fold_backfill_operation(&rows, "bfop_test")
            .unwrap()
            .unwrap();
        assert_eq!(state.planned_segments.len(), 4);
        assert_eq!(state.checkpointed_segments.len(), 2);
        assert_eq!(state.pending_segments.len(), 2);

        // Pending must contain seg_named_default and seg_mic_suffix
        assert!(state.pending_segments.contains(&seg_named_default));
        assert!(state.pending_segments.contains(&seg_mic_suffix));
        assert!(!state.pending_segments.contains(&seg_direct_default));
        assert!(!state.pending_segments.contains(&seg_mic));
    }
}
