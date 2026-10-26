// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Project actual native results into the existing restic consumer schema.
//! This grants no startup authority and never infers rollback from cancellation.

use std::collections::BTreeSet;
use std::io;

use serde::Deserialize;
use serde_json::{Value, json};

use super::native::{Completed, ResultFields};
use crate::ios_linked_command::command::Operation;
use crate::runner::{ResticResult, RunnerError};

const MAX_METADATA: usize = 4 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 4096;

fn invalid() -> RunnerError {
    RunnerError::Process(io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid callable backup result",
    ))
}

fn text(bytes: &[u8]) -> Result<&str, RunnerError> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(invalid)?;
    if bytes[end..].iter().any(|byte| *byte != 0) {
        return Err(invalid());
    }
    std::str::from_utf8(&bytes[..end]).map_err(|_| invalid())
}

pub(super) fn identifier(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn optional_id(bytes: &[u8]) -> Result<Option<&str>, RunnerError> {
    let value = text(bytes)?;
    if value.is_empty() {
        Ok(None)
    } else if identifier(value) {
        Ok(Some(value))
    } else {
        Err(invalid())
    }
}

pub(super) fn diagnostic(fields: &ResultFields) -> Result<&str, RunnerError> {
    let value = text(&fields.diagnostic)?;
    if matches!(
        value,
        "" | "INVALID_ARGUMENT"
            | "CAP_EXCEEDED"
            | "AUTH_FAILED"
            | "REPO_MISSING"
            | "REPO_EXISTS"
            | "REPO_CORRUPT"
            | "IO_FAILED"
            | "REPO_LOCKED"
            | "CANCELLED"
            | "CANCELLED_UNCERTAIN"
            | "SNAPSHOT_EMPTY"
            | "RESTORE_DESTINATION_NOT_EMPTY"
            | "RESTORE_DESTINATION_EXISTS"
            | "RESTORE_DESTINATION_INVALID"
            | "INTERNAL_ERROR"
            | "COMMITTED_UNCERTAIN"
            | "WORKSPACE_CLEANUP_REQUIRED"
            | "REMOTE_PENDING"
            | "REMOTE_PENDING_RESTORE_COMMITTED"
            | "REMOTE_PENDING_WORKSPACE_CLEANUP_REQUIRED"
            | "REMOTE_PENDING_LOCAL_CLEANUP_REQUIRED"
            | "REMOTE_PENDING_RESTORE_COMMITTED_LOCAL_CLEANUP_REQUIRED"
            | "STATE_COMMITTED_UNCERTAIN"
            | "STATE_COMMITTED_UNCERTAIN_RESTORE_COMMITTED"
            | "STATE_COMMITTED_UNCERTAIN_WORKSPACE_CLEANUP_REQUIRED"
            | "STATE_COMMITTED_UNCERTAIN_LOCAL_CLEANUP_REQUIRED"
            | "STATE_COMMITTED_UNCERTAIN_RESTORE_COMMITTED_LOCAL_CLEANUP_REQUIRED"
            | "LOCAL_CLEANUP_REQUIRED"
            | "RESTORE_COMMITTED_LOCAL_CLEANUP_REQUIRED"
    ) {
        Ok(value)
    } else {
        Err(invalid())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryEnvelope {
    schema: String,
    kind: String,
    repository: RepositoryConfig,
}
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RepositoryConfig {
    version: u32,
    id: String,
    chunker_polynomial: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogEnvelope {
    schema: String,
    kind: String,
    snapshots: Vec<CatalogRow>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogRow {
    id: String,
    snapshot: Snapshot,
}
#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    time: String,
    paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summary: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreEnvelope {
    schema: String,
    kind: String,
    state: RestoreState,
}
/// restic's own restore progress state; restic's JSON summary reports exactly these.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(non_snake_case)]
struct RestoreState {
    FilesFinished: u64,
    FilesTotal: u64,
    FilesSkipped: u64,
    FilesDeleted: u64,
    AllBytesWritten: u64,
    AllBytesTotal: u64,
    AllBytesSkipped: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryKeyEnvelope {
    schema: String,
    kind: String,
    keys: Vec<RecoveryKeyRow>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryKeyRow {
    id: String,
    current: bool,
}

pub(super) fn decode_recovery_keys(
    completed: &Completed,
) -> Result<Vec<(String, bool)>, RunnerError> {
    let raw = metadata(completed)?;
    let envelope: RecoveryKeyEnvelope = serde_json::from_slice(raw).map_err(|_| invalid())?;
    if envelope.schema != "solstone.backup-metadata.v1"
        || envelope.kind != "recovery-key-list"
        || envelope.keys.len() > 16
    {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    let mut current_count = 0;
    let mut keys = Vec::with_capacity(envelope.keys.len());
    for row in envelope.keys {
        if !identifier(&row.id) || !seen.insert(row.id.clone()) {
            return Err(invalid());
        }
        if row.current {
            current_count += 1;
        }
        keys.push((row.id, row.current));
    }
    if current_count != 1 {
        return Err(invalid());
    }
    Ok(keys)
}

fn metadata(completed: &Completed) -> Result<&[u8], RunnerError> {
    completed
        .metadata
        .as_ref()
        .ok()
        .and_then(Option::as_deref)
        .filter(|bytes| !bytes.is_empty() && bytes.len() <= MAX_METADATA)
        .ok_or_else(invalid)
}

fn successful(
    operation: Option<&Operation>,
    completed: &Completed,
) -> Result<Option<Value>, RunnerError> {
    match operation {
        Some(Operation::Open) => {
            let envelope: RepositoryEnvelope =
                serde_json::from_slice(metadata(completed)?).map_err(|_| invalid())?;
            if envelope.schema != "solstone.backup-metadata.v1"
                || envelope.kind != "repository-config"
                || !identifier(&envelope.repository.id)
                || !matches!(envelope.repository.version, 1 | 2)
            {
                return Err(invalid());
            }
            serde_json::to_value(envelope.repository)
                .map(Some)
                .map_err(|_| invalid())
        }
        Some(Operation::ListSnapshots) => {
            let envelope: CatalogEnvelope =
                serde_json::from_slice(metadata(completed)?).map_err(|_| invalid())?;
            let count = completed.fields.snapshot_count as usize;
            if envelope.schema != "solstone.backup-metadata.v1"
                || envelope.kind != "snapshot-catalog"
                || count > MAX_SNAPSHOTS
                || envelope.snapshots.len() != count
            {
                return Err(invalid());
            }
            let actual = completed.fields.snapshot_ids[..count]
                .iter()
                .map(|bytes| optional_id(bytes).and_then(|id| id.ok_or_else(invalid)))
                .collect::<Result<BTreeSet<_>, _>>()?;
            if actual.len() != count {
                return Err(invalid());
            }
            let mut seen = BTreeSet::new();
            let mut rows = Vec::with_capacity(count);
            for row in envelope.snapshots {
                if !identifier(&row.id)
                    || !actual.contains(row.id.as_str())
                    || !seen.insert(row.id.clone())
                    || row.snapshot.paths.len() > MAX_SNAPSHOTS
                    || row
                        .snapshot
                        .tags
                        .as_ref()
                        .is_some_and(|tags| tags.len() > MAX_SNAPSHOTS)
                {
                    return Err(invalid());
                }
                let mut snapshot = serde_json::to_value(row.snapshot).map_err(|_| invalid())?;
                snapshot
                    .as_object_mut()
                    .ok_or_else(invalid)?
                    .insert("id".into(), Value::String(row.id));
                rows.push(snapshot);
            }
            Ok(Some(Value::Array(rows)))
        }
        Some(Operation::Snapshot { .. }) => {
            let snapshot_id = optional_id(&completed.fields.snapshot_id)?.ok_or_else(invalid)?;
            // No SDK byte/file counters are exposed by this ABI. Never invent them.
            Ok(Some(
                json!({"message_type":"summary","snapshot_id":snapshot_id}),
            ))
        }
        Some(Operation::Restore { .. }) => {
            let envelope: RestoreEnvelope =
                serde_json::from_slice(metadata(completed)?).map_err(|_| invalid())?;
            if envelope.schema != "solstone.backup-metadata.v1"
                || envelope.kind != "existing-root-restore"
            {
                return Err(invalid());
            }
            let state = envelope.state;
            Ok(Some(json!({
                "message_type": "summary",
                "total_files": state.FilesTotal,
                "files_restored": state.FilesFinished,
                "files_skipped": state.FilesSkipped,
                "files_deleted": state.FilesDeleted,
                "total_bytes": state.AllBytesTotal,
                "bytes_restored": state.AllBytesWritten,
                "bytes_skipped": state.AllBytesSkipped,
            })))
        }
        Some(Operation::Init | Operation::Check(_) | Operation::Retention { .. }) | None => {
            Ok(None)
        }
    }
}

pub(super) fn project(
    operation: Option<&Operation>,
    completed: Completed,
    args: &[String],
    want_json: bool,
) -> Result<ResticResult, RunnerError> {
    let status = completed.fields.status;
    if !(0..=11).contains(&status) {
        return Err(invalid());
    }
    let diagnostic = diagnostic(&completed.fields)?;
    let key_id = optional_id(&completed.fields.key_id)?;
    let snapshot_id = optional_id(&completed.fields.snapshot_id)?;
    let returncode = if completed.monitor_failed {
        1
    } else {
        match (status, diagnostic) {
            (0, "") => 0,
            (2, "REPO_MISSING") => 10,
            (2, "REPO_LOCKED") => 11,
            (2, "AUTH_FAILED") => 12,
            (3, _) if completed.deadline_requested && !completed.cancellation_requested => 124,
            (3, _) => 130,
            // Native 10/11 are cleanup/pending, never CLI repo-missing/locked.
            _ => 1,
        }
    };
    let value = if returncode == 0 {
        successful(operation, &completed)?
    } else {
        Some(
            json!({"ios_callable":{"status":status,"diagnostic":diagnostic,"key_id":key_id,"snapshot_id":snapshot_id,"monitor_failed":completed.monitor_failed,"cancellation_requested":completed.cancellation_requested,"deadline_requested":completed.deadline_requested}}),
        )
    };
    let stdout = if returncode == 0 {
        value
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| invalid())?
            .unwrap_or_default()
    } else {
        String::new()
    };
    Ok(ResticResult {
        returncode,
        stdout,
        stderr: if returncode == 0 {
            String::new()
        } else {
            // Non-JSON mutating callers still receive actual current-call IDs
            // and uncertainty flags. This contains only validated fixed fields.
            serde_json::to_string(&value).map_err(|_| invalid())?
        },
        json: if want_json { value } else { None },
        argv: args.to_vec(),
    })
}

pub(super) fn callable_failure(
    completed: &Completed,
) -> Result<Option<crate::repo::IosCallableFailure>, RunnerError> {
    let status = completed.fields.status;
    if !(0..=11).contains(&status) {
        return Err(invalid());
    }
    let diagnostic = diagnostic(&completed.fields)?;
    let key_id = optional_id(&completed.fields.key_id)?.map(str::to_owned);
    let snapshot_id = optional_id(&completed.fields.snapshot_id)?.map(str::to_owned);
    if status == 0 && diagnostic.is_empty() && !completed.monitor_failed {
        if key_id.is_none() {
            return Err(invalid());
        }
        return Ok(None);
    }
    Ok(Some(crate::repo::IosCallableFailure {
        native_status: status,
        diagnostic: diagnostic.to_owned(),
        key_id,
        snapshot_id,
        cancellation_requested: completed.cancellation_requested,
        deadline_requested: completed.deadline_requested,
        monitor_failed: completed.monitor_failed,
    }))
}

pub(super) fn callable_remove_failure(
    completed: &Completed,
) -> Result<Option<crate::repo::IosCallableFailure>, RunnerError> {
    let status = completed.fields.status;
    if !(0..=11).contains(&status) {
        return Err(invalid());
    }
    let diagnostic = diagnostic(&completed.fields)?;
    let key_id = optional_id(&completed.fields.key_id)?.map(str::to_owned);
    let snapshot_id = optional_id(&completed.fields.snapshot_id)?.map(str::to_owned);
    if status == 0 && diagnostic.is_empty() && !completed.monitor_failed {
        return Ok(None);
    }
    Ok(Some(crate::repo::IosCallableFailure {
        native_status: status,
        diagnostic: diagnostic.to_owned(),
        key_id,
        snapshot_id,
        cancellation_requested: completed.cancellation_requested,
        deadline_requested: completed.deadline_requested,
        monitor_failed: completed.monitor_failed,
    }))
}
