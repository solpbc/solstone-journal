// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Append-only durable ledger for resumable speaker repair operations.
//!
//! Exposes strict read/fold and append operations for schema version 1.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{
    AppendError, FileLock, LockError, LockOptions, SegmentLayout, append_jsonl, hold_lock,
};
use thiserror::Error;

pub const REPAIR_OPERATION_SCHEMA_VERSION: i64 = 1;

/// Exact segment key tuple identifying a chronicle segment across streams and twin layouts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SegmentTupleKey {
    pub day: String,
    pub stream_layout: SegmentLayout,
    pub stream: String,
    pub segment_name: String,
}

impl SegmentTupleKey {
    #[must_use]
    pub fn new(
        day: impl Into<String>,
        stream_layout: SegmentLayout,
        stream: impl Into<String>,
        segment_name: impl Into<String>,
    ) -> Self {
        Self {
            day: day.into(),
            stream_layout,
            stream: stream.into(),
            segment_name: segment_name.into(),
        }
    }
}

/// Prepared segment snapshot capturing pre-mutation hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedSegmentSnapshot {
    pub day: String,
    pub stream_layout: SegmentLayout,
    pub stream: String,
    pub segment_name: String,
    pub label_sha256: String,
    pub corrections_sha256: String,
}

/// Information recorded in a write intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteIntentInfo {
    pub intent_id: String,
    pub supersedes_intent_id: Option<String>,
    pub key: SegmentTupleKey,
    pub expected_current_label_sha256: String,
    pub expected_corrections_sha256: String,
    pub intended_payload_sha256: String,
    pub intended_payload: Value,
    pub timestamp: String,
}

/// Information about a failed attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairFailureInfo {
    pub stage: String,
    pub detail: String,
    pub retryable: bool,
    pub timestamp: String,
}

/// Structured events stored in `speakers/repair-operations.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RepairEvent {
    Accepted {
        schema_version: i64,
        operation_id: String,
        timestamp: String,
    },
    AttemptStarted {
        schema_version: i64,
        operation_id: String,
        attempt_id: String,
        timestamp: String,
    },
    Prepared {
        schema_version: i64,
        operation_id: String,
        attempt_id: String,
        planned_segments: Vec<PreparedSegmentSnapshot>,
        timestamp: String,
    },
    WriteIntent {
        schema_version: i64,
        operation_id: String,
        attempt_id: String,
        intent_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        supersedes_intent_id: Option<String>,
        day: String,
        stream_layout: SegmentLayout,
        stream: String,
        segment_name: String,
        expected_current_label_sha256: String,
        expected_corrections_sha256: String,
        intended_payload_sha256: String,
        intended_payload: Value,
        timestamp: String,
    },
    Checkpoint {
        schema_version: i64,
        operation_id: String,
        attempt_id: String,
        intent_id: String,
        day: String,
        stream_layout: SegmentLayout,
        stream: String,
        segment_name: String,
        timestamp: String,
    },
    AttemptFailed {
        schema_version: i64,
        operation_id: String,
        attempt_id: String,
        stage: String,
        detail: String,
        retryable: bool,
        timestamp: String,
    },
    Completed {
        schema_version: i64,
        operation_id: String,
        attempt_id: String,
        summary: Value,
        timestamp: String,
    },
}

#[derive(Debug, Error)]
pub enum RepairLedgerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("append error: {0}")]
    Append(#[from] AppendError),
    #[error("lock error: {0}")]
    Lock(#[from] LockError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid ledger path: {0}")]
    Path(String),
}

/// In-memory folded state of a repair operation.
#[derive(Debug, Clone, Default)]
pub struct RepairOperationState {
    pub operation_id: String,
    pub is_accepted: bool,
    pub is_completed: bool,
    pub latest_attempt_id: Option<String>,
    pub prepared: Option<Vec<PreparedSegmentSnapshot>>,
    pub latest_intents: BTreeMap<SegmentTupleKey, WriteIntentInfo>,
    pub checkpointed_segments: BTreeSet<SegmentTupleKey>,
    pub latest_failure: Option<RepairFailureInfo>,
    pub summary: Option<Value>,
}

pub fn ledger_path(journal_root: &Path) -> PathBuf {
    journal_root.join("speakers/repair-operations.jsonl")
}

pub fn execution_lock_path(journal_root: &Path) -> PathBuf {
    journal_root.join("health/locks/speaker-repair")
}

/// Acquire the non-blocking execution lock for speaker repair.
pub fn acquire_repair_lock(journal_root: &Path) -> Result<FileLock, LockError> {
    let path = execution_lock_path(journal_root);
    hold_lock(
        &path,
        LockOptions {
            timeout: Duration::ZERO,
            ..Default::default()
        },
    )
}

/// Append a single repair event to `speakers/repair-operations.jsonl`.
pub fn append_repair_event(
    journal_root: &Path,
    event: &RepairEvent,
) -> Result<(), RepairLedgerError> {
    let p = ledger_path(journal_root);
    append_jsonl(&p, event)?;
    Ok(())
}

/// Read all raw events from the repair ledger.
pub fn load_repair_ledger(journal_root: &Path) -> Result<Vec<RepairEvent>, RepairLedgerError> {
    let p = ledger_path(journal_root);
    if !p.is_file() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&p)?;
    let mut events = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event = serde_json::from_str::<RepairEvent>(trimmed)?;
        events.push(event);
    }
    Ok(events)
}

/// Fold events for a specific operation ID from the ledger.
pub fn fold_repair_operation(
    journal_root: &Path,
    target_operation_id: &str,
) -> Result<Option<RepairOperationState>, RepairLedgerError> {
    let events = load_repair_ledger(journal_root)?;
    let mut state: Option<RepairOperationState> = None;

    for event in events {
        match event {
            RepairEvent::Accepted {
                operation_id, ..
            } => {
                if operation_id == target_operation_id {
                    let s = state.get_or_insert_with(|| RepairOperationState {
                        operation_id: operation_id.clone(),
                        ..Default::default()
                    });
                    s.is_accepted = true;
                }
            }
            RepairEvent::AttemptStarted {
                operation_id,
                attempt_id,
                ..
            } => {
                if operation_id == target_operation_id {
                    let s = state.get_or_insert_with(|| RepairOperationState {
                        operation_id: operation_id.clone(),
                        ..Default::default()
                    });
                    s.latest_attempt_id = Some(attempt_id);
                }
            }
            RepairEvent::Prepared {
                operation_id,
                attempt_id,
                planned_segments,
                ..
            } => {
                if operation_id == target_operation_id {
                    let s = state.get_or_insert_with(|| RepairOperationState {
                        operation_id: operation_id.clone(),
                        ..Default::default()
                    });
                    s.latest_attempt_id = Some(attempt_id);
                    s.prepared = Some(planned_segments);
                }
            }
            RepairEvent::WriteIntent {
                operation_id,
                attempt_id,
                intent_id,
                supersedes_intent_id,
                day,
                stream_layout,
                stream,
                segment_name,
                expected_current_label_sha256,
                expected_corrections_sha256,
                intended_payload_sha256,
                intended_payload,
                timestamp,
                ..
            } => {
                if operation_id == target_operation_id {
                    let s = state.get_or_insert_with(|| RepairOperationState {
                        operation_id: operation_id.clone(),
                        ..Default::default()
                    });
                    s.latest_attempt_id = Some(attempt_id);
                    let key = SegmentTupleKey::new(day, stream_layout, stream, segment_name);
                    s.latest_intents.insert(
                        key.clone(),
                        WriteIntentInfo {
                            intent_id,
                            supersedes_intent_id,
                            key,
                            expected_current_label_sha256,
                            expected_corrections_sha256,
                            intended_payload_sha256,
                            intended_payload,
                            timestamp,
                        },
                    );
                }
            }
            RepairEvent::Checkpoint {
                operation_id,
                day,
                stream_layout,
                stream,
                segment_name,
                ..
            } => {
                if operation_id == target_operation_id {
                    let s = state.get_or_insert_with(|| RepairOperationState {
                        operation_id: operation_id.clone(),
                        ..Default::default()
                    });
                    let key = SegmentTupleKey::new(day, stream_layout, stream, segment_name);
                    s.checkpointed_segments.insert(key);
                }
            }
            RepairEvent::AttemptFailed {
                operation_id,
                attempt_id,
                stage,
                detail,
                retryable,
                timestamp,
                ..
            } => {
                if operation_id == target_operation_id {
                    let s = state.get_or_insert_with(|| RepairOperationState {
                        operation_id: operation_id.clone(),
                        ..Default::default()
                    });
                    s.latest_attempt_id = Some(attempt_id);
                    s.latest_failure = Some(RepairFailureInfo {
                        stage,
                        detail,
                        retryable,
                        timestamp,
                    });
                }
            }
            RepairEvent::Completed {
                operation_id,
                attempt_id,
                summary,
                ..
            } => {
                if operation_id == target_operation_id {
                    let s = state.get_or_insert_with(|| RepairOperationState {
                        operation_id: operation_id.clone(),
                        ..Default::default()
                    });
                    s.latest_attempt_id = Some(attempt_id);
                    s.is_completed = true;
                    s.summary = Some(summary);
                }
            }
        }
    }

    Ok(state)
}
