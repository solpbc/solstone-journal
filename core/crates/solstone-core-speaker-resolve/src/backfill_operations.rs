// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Append-only durable ledger for resumable speaker backfill operations.
//!
//! This module intentionally exposes only strict read/fold and append operations.
//! It contains no rewrite, compaction, or deletion primitive.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{append_jsonl, hold_lock, AppendError, LockError, LockOptions};
use thiserror::Error;

pub const BACKFILL_OPERATION_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BackfillSegmentKey {
    pub day: String,
    pub stream: String,
    pub segment_key: String,
}

impl BackfillSegmentKey {
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "day": self.day,
            "stream": self.stream,
            "segment_key": self.segment_key,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillEventKind {
    Prepared,
    Checkpoint,
    Completed,
}

impl BackfillEventKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Checkpoint => "checkpoint",
            Self::Completed => "completed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "prepared" => Self::Prepared,
            "checkpoint" => Self::Checkpoint,
            "completed" => Self::Completed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillOperationPayload {
    Prepared {
        started_at: String,
        reattribute: bool,
        total_count: usize,
        segments: Vec<BackfillSegmentKey>,
    },
    Checkpoint {
        segment: BackfillSegmentKey,
        outcome: BackfillCheckpointOutcome,
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
            BackfillOperationPayload::Prepared { .. } => BackfillEventKind::Prepared,
            BackfillOperationPayload::Checkpoint { .. } => BackfillEventKind::Checkpoint,
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
            BackfillOperationPayload::Prepared {
                started_at,
                reattribute,
                total_count,
                segments,
            } => {
                row.insert("started_at".to_owned(), Value::String(started_at.clone()));
                row.insert("reattribute".to_owned(), Value::Bool(*reattribute));
                row.insert("total_count".to_owned(), Value::from(*total_count));
                row.insert(
                    "segments".to_owned(),
                    Value::Array(segments.iter().map(BackfillSegmentKey::to_json).collect()),
                );
            }
            BackfillOperationPayload::Checkpoint { segment, outcome } => {
                row.insert("day".to_owned(), Value::String(segment.day.clone()));
                row.insert("stream".to_owned(), Value::String(segment.stream.clone()));
                row.insert(
                    "segment_key".to_owned(),
                    Value::String(segment.segment_key.clone()),
                );
                row.insert(
                    "outcome".to_owned(),
                    Value::String(outcome.as_str().to_owned()),
                );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillOperationTerminalStatus {
    Resumable,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillOperationState {
    pub operation_id: String,
    pub started_at: String,
    pub reattribute: bool,
    pub total_segments: usize,
    pub checkpointed_segments: BTreeMap<BackfillSegmentKey, BackfillCheckpointOutcome>,
    pub pending_segments: Vec<BackfillSegmentKey>,
    pub terminal_status: BackfillOperationTerminalStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillOperationStatus {
    pub total_count: usize,
    pub completed_count: usize,
    pub pending_count: usize,
    pub resumable: bool,
    pub done: bool,
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
    #[error("prepared total_count does not match segments")]
    PreparedTotalCountMismatch,
    #[error("prepared segment is not an object")]
    PreparedSegmentNotObject,
    #[error("operation must have exactly one prepared row")]
    PreparedRowCount,
    #[error("checkpoint segment is absent from the prepared snapshot")]
    CheckpointOutsidePrepared,
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
    if object.get("schema_version").and_then(Value::as_i64)
        != Some(BACKFILL_OPERATION_SCHEMA_VERSION)
    {
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
                .map(parse_segment)
                .collect::<Result<Vec<_>, _>>()?;
            if total_count != segments.len() {
                return Err(BackfillOperationError::PreparedTotalCountMismatch);
            }
            BackfillOperationPayload::Prepared {
                started_at,
                reattribute,
                total_count,
                segments,
            }
        }
        BackfillEventKind::Checkpoint => {
            let segment = parse_segment(&Value::Object(object.clone()))?;
            let outcome_text = required_string(object, "outcome")?;
            let outcome = BackfillCheckpointOutcome::parse(&outcome_text).ok_or(
                BackfillOperationError::InvalidCheckpointOutcome {
                    outcome: outcome_text,
                },
            )?;
            BackfillOperationPayload::Checkpoint { segment, outcome }
        }
        BackfillEventKind::Completed => BackfillOperationPayload::Completed {
            completed_at: required_string(object, "completed_at")?,
        },
    };
    Ok(BackfillOperationEvent {
        schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
        event_id,
        operation_id,
        ts,
        payload,
    })
}

/// Strictly load every JSONL row. One malformed row aborts the entire load.
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
    contents
        .lines()
        .enumerate()
        .filter_map(|(index, line)| (!line.trim().is_empty()).then_some((index + 1, line)))
        .map(|(line, raw_json)| BackfillLedgerRow::parse(path, line, raw_json))
        .collect()
}

/// Append one validated row. Existing rows are strictly validated while held locked.
pub fn append_backfill_event(
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
    let _lock = hold_lock(path, LockOptions::default())?;
    let _ = load_backfill_operations(path)?;
    append_jsonl(path, &value)?;
    Ok(())
}

/// Fold one operation to its resume state. Rows for other operations are ignored.
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
    let prepared = events
        .iter()
        .filter_map(|event| match &event.payload {
            BackfillOperationPayload::Prepared {
                started_at,
                reattribute,
                total_count,
                segments,
            } => Some((started_at, reattribute, total_count, segments)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if prepared.len() != 1 {
        return Err(BackfillOperationError::PreparedRowCount);
    }
    let (started_at, reattribute, total_count, segments) = prepared[0];
    let planned = segments.iter().cloned().collect::<BTreeSet<_>>();
    let mut checkpointed_segments = BTreeMap::new();
    let mut done = false;
    for event in events {
        match &event.payload {
            BackfillOperationPayload::Checkpoint { segment, outcome } => {
                if !planned.contains(segment) {
                    return Err(BackfillOperationError::CheckpointOutsidePrepared);
                }
                checkpointed_segments.insert(segment.clone(), *outcome);
            }
            BackfillOperationPayload::Completed { .. } => done = true,
            BackfillOperationPayload::Prepared { .. } => {}
        }
    }
    let pending_segments = segments
        .iter()
        .filter(|segment| !checkpointed_segments.contains_key(*segment))
        .cloned()
        .collect();
    Ok(Some(BackfillOperationState {
        operation_id: operation_id.to_owned(),
        started_at: started_at.clone(),
        reattribute: *reattribute,
        total_segments: *total_count,
        checkpointed_segments,
        pending_segments,
        terminal_status: if done {
            BackfillOperationTerminalStatus::Done
        } else {
            BackfillOperationTerminalStatus::Resumable
        },
    }))
}

/// Return counts and resumability without mutating the ledger.
pub fn backfill_operation_status(
    rows: &[BackfillLedgerRow],
    operation_id: &str,
) -> Result<Option<BackfillOperationStatus>, BackfillOperationError> {
    let Some(state) = fold_backfill_operation(rows, operation_id)? else {
        return Ok(None);
    };
    let done = state.terminal_status == BackfillOperationTerminalStatus::Done;
    Ok(Some(BackfillOperationStatus {
        total_count: state.total_segments,
        completed_count: state.checkpointed_segments.len(),
        pending_count: state.pending_segments.len(),
        resumable: !done,
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

fn parse_segment(value: &Value) -> Result<BackfillSegmentKey, BackfillOperationError> {
    let object = value
        .as_object()
        .ok_or(BackfillOperationError::PreparedSegmentNotObject)?;
    Ok(BackfillSegmentKey {
        day: required_string(object, "day")?,
        stream: required_string(object, "stream")?,
        segment_key: required_string(object, "segment_key")?,
    })
}
