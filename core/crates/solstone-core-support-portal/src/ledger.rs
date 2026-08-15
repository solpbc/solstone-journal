// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The sole durable writer for support portal operation records and fingerprint keys.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use getrandom::fill as random_fill;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{
    AtomicWriteOptions, LockError, LockOptions, atomic_replace, hold_lock,
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::canonical::{
    canonical_fingerprint, canonicalize_operation, derive_child_action_id, operation_key,
    principal_tag,
};
use crate::errors::{
    ACTION_ID_INVALID, KEY_BUSY, KEY_INVALID, KEY_UNAVAILABLE, KEY_UNREADABLE, KEY_UNSAFE,
    LEDGER_BUSY, OperationError, RECORD_INVALID, RECORD_UNREADABLE, TIMESTAMP_INVALID,
};

const SCHEMA_VERSION: u64 = 1;
const KEY_BYTES: usize = 32;
const LEASE_DURATION: Duration = Duration::seconds(60);
const RETENTION: Duration = Duration::days(45);

/// One operation record returned to the portal adapter.  `operation_key` is never persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecord {
    /// Caller-provided parent action ID.
    pub parent_action_id: String,
    /// Derived child action ID used for the record name.
    pub child_action_id: String,
    /// Support operation verb.
    pub verb: String,
    /// Keyed tag of the principal, never the principal itself.
    pub principal_tag: String,
    /// Keyed canonical request fingerprint.
    pub canonical_fingerprint: String,
    /// Current lifecycle state.
    pub state: String,
    /// Monotonic lease generation.
    pub generation: u64,
    /// Current lease ID, when applicable.
    pub lease_id: Option<String>,
    /// Current lease expiry in UTC ISO form.
    pub lease_expires_at: Option<String>,
    /// Accepted remote operation ID, if any.
    pub remote_operation_id: Option<String>,
    /// Separate acknowledgement state, never a lifecycle state.
    pub ack_state: String,
    /// Terminal completion time, if any.
    pub completed_at: Option<String>,
    /// Record creation time.
    pub created_at: String,
    /// Opaque terminal failure reason, if any.
    pub terminal_reason: Option<String>,
    /// In-memory-only portal idempotency key.
    pub operation_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRecord {
    schema_version: u64,
    parent_action_id: String,
    child_action_id: String,
    verb: String,
    principal_tag: String,
    canonical_fingerprint: String,
    state: String,
    generation: u64,
    lease_id: Option<String>,
    lease_expires_at: Option<String>,
    remote_operation_id: Option<String>,
    ack_state: String,
    completed_at: Option<String>,
    created_at: String,
    terminal_reason: Option<String>,
}

enum ReadRecord {
    Missing,
    Record(Box<StoredRecord>),
    Retired,
}

/// A ledger rooted at the caller-selected support portal storage directory.
#[derive(Debug, Clone)]
pub struct Ledger {
    storage_dir: PathBuf,
}

impl Ledger {
    /// Construct a ledger rooted at one explicit support portal storage directory.
    pub fn new(storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            storage_dir: storage_dir.into(),
        }
    }

    /// Create or recover a deterministic operation lease.
    pub fn begin_operation(
        &self,
        parent_action_id: &str,
        verb: &str,
        fields: &serde_json::Map<String, Value>,
        principal: &str,
        index: u32,
        now: DateTime<Utc>,
    ) -> Result<OperationRecord, OperationError> {
        self.ensure_storage()?;
        self.compact_expired_terminal_records(now)?;
        let child_action_id = derive_child_action_id(parent_action_id, verb, index);
        let canonical = canonicalize_operation(verb, fields, principal, &child_action_id)
            .map_err(|_| unavailable(RECORD_INVALID))?;
        let key = self.load_or_create_fingerprint_key()?;
        let fingerprint = canonical_fingerprint(&key, &canonical);
        let principal_tag = principal_tag(&key, principal);
        let operation_key = operation_key(&key, &canonical);
        let path = self.record_path(&child_action_id)?;
        let _lock = lock(&path, LEDGER_BUSY)?;
        match read_record(&path)? {
            ReadRecord::Missing => {
                let record = StoredRecord {
                    schema_version: SCHEMA_VERSION,
                    parent_action_id: parent_action_id.nfc().collect(),
                    child_action_id,
                    verb: verb.nfc().collect(),
                    principal_tag,
                    canonical_fingerprint: fingerprint,
                    state: "pending".to_owned(),
                    generation: 1,
                    lease_id: Some(new_lease_id()),
                    lease_expires_at: Some(iso(now + LEASE_DURATION)),
                    remote_operation_id: None,
                    ack_state: "not_applicable".to_owned(),
                    completed_at: None,
                    created_at: iso(now),
                    terminal_reason: None,
                };
                write_record(&path, &record)?;
                Ok(record.into_operation(Some(operation_key)))
            }
            ReadRecord::Retired => Err(OperationError::OperationRetired),
            ReadRecord::Record(mut stored) => {
                if stored.canonical_fingerprint != fingerprint {
                    return Err(OperationError::IdempotencyConflict);
                }
                if stored.state == "failed" {
                    return Err(terminal_failure_error(stored.terminal_reason.as_deref()));
                }
                if stored.state == "in_progress" && lease_is_live(&stored, now)? {
                    return Err(OperationError::OperationInProgress);
                }
                if matches!(stored.state.as_str(), "pending" | "in_progress")
                    && !lease_is_live(&stored, now)?
                {
                    stored.generation += 1;
                    stored.lease_id = Some(new_lease_id());
                    stored.lease_expires_at = Some(iso(now + LEASE_DURATION));
                    write_record(&path, &stored)?;
                }
                Ok((*stored).into_operation(Some(operation_key)))
            }
        }
    }

    /// Mark the current record in progress while its lease remains live.
    pub fn mark_in_progress(
        &self,
        record: &OperationRecord,
        now: DateTime<Utc>,
    ) -> Result<OperationRecord, OperationError> {
        self.update_current(record, |stored| {
            if stored.state != "pending" || !lease_is_live(stored, now)? {
                return Err(OperationError::OperationInvalidState);
            }
            stored.state = "in_progress".to_owned();
            Ok(())
        })
    }

    /// Release a retryable in-progress lease without changing its state.
    pub fn release_retryable_lease(
        &self,
        record: &OperationRecord,
        now: DateTime<Utc>,
    ) -> Result<OperationRecord, OperationError> {
        self.update_current(record, |stored| {
            if stored.state == "completed" {
                return Ok(());
            }
            if stored.state != "in_progress" {
                return Err(OperationError::OperationInvalidState);
            }
            stored.lease_expires_at = Some(iso(now));
            Ok(())
        })
    }

    /// Mark a current in-progress record completed.
    pub fn mark_completed(
        &self,
        record: &OperationRecord,
        remote_operation_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<OperationRecord, OperationError> {
        self.update_current(record, |stored| {
            if stored.state == "completed" {
                return Ok(());
            }
            if stored.state != "in_progress" || !lease_is_live(stored, now)? {
                return Err(OperationError::OperationInvalidState);
            }
            stored.state = "completed".to_owned();
            stored.remote_operation_id = remote_operation_id.map(ToOwned::to_owned);
            stored.ack_state = if remote_operation_id.is_some() {
                "unacknowledged"
            } else {
                "not_applicable"
            }
            .to_owned();
            stored.completed_at = Some(iso(now));
            stored.lease_id = None;
            stored.lease_expires_at = None;
            stored.terminal_reason = None;
            Ok(())
        })
    }

    /// Mark a current live record terminally failed.
    pub fn mark_failed(
        &self,
        record: &OperationRecord,
        reason: &str,
        now: DateTime<Utc>,
    ) -> Result<OperationRecord, OperationError> {
        self.update_current(record, |stored| {
            if !matches!(stored.state.as_str(), "pending" | "in_progress")
                || !lease_is_live(stored, now)?
            {
                return Err(OperationError::OperationInvalidState);
            }
            if !terminal_reason_is_valid(reason) {
                return Err(OperationError::OperationInvalidState);
            }
            stored.state = "failed".to_owned();
            stored.ack_state = "not_applicable".to_owned();
            stored.completed_at = Some(iso(now));
            stored.lease_id = None;
            stored.lease_expires_at = None;
            stored.terminal_reason = Some(reason.to_owned());
            Ok(())
        })
    }

    /// Acknowledge a completed remote operation.  This transition intentionally takes no clock.
    pub fn mark_acknowledged(
        &self,
        record: &OperationRecord,
    ) -> Result<OperationRecord, OperationError> {
        self.update_current(record, |stored| {
            if stored.state != "completed" {
                return Err(OperationError::OperationInvalidState);
            }
            if stored.ack_state == "acknowledged" {
                return Ok(());
            }
            if stored.ack_state != "unacknowledged" {
                return Err(OperationError::OperationInvalidState);
            }
            stored.ack_state = "acknowledged".to_owned();
            Ok(())
        })
    }

    /// List completed records whose remote acknowledgement is still pending.
    pub fn list_pending_acknowledgements(&self) -> Result<Vec<OperationRecord>, OperationError> {
        let operations = self.operations_dir();
        if !operations.is_dir() {
            return Ok(Vec::new());
        }
        let mut paths = json_record_paths(&operations)?;
        paths.sort();
        let mut records = Vec::new();
        for path in paths {
            if let ReadRecord::Record(stored) = read_record(&path)?
                && stored.state == "completed"
                && stored.ack_state == "unacknowledged"
            {
                records.push((*stored).into_operation(None));
            }
        }
        Ok(records)
    }

    /// Replace terminal records older than the reference retention window with retired markers.
    pub fn compact_expired_terminal_records(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(), OperationError> {
        self.ensure_storage()?;
        let operations = self.operations_dir();
        if !operations.is_dir() {
            return Ok(());
        }
        let mut paths = json_record_paths(&operations)?;
        paths.sort();
        for path in paths {
            let _lock = lock(&path, LEDGER_BUSY)?;
            let ReadRecord::Record(stored) = read_record(&path)? else {
                continue;
            };
            if !matches!(stored.state.as_str(), "completed" | "failed" | "conflict") {
                continue;
            }
            let Some(completed_at) = stored.completed_at.as_deref() else {
                continue;
            };
            if now - parse_iso(completed_at)? <= RETENTION {
                continue;
            }
            let marker = serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "child_action_id": stored.child_action_id,
                "terminal_reason": stored.terminal_reason.unwrap_or(stored.state),
            });
            write_json_line(&path, &marker)?;
        }
        Ok(())
    }

    /// Return the explicit storage path for diagnostics and adapter wiring.
    pub fn storage_dir(&self) -> &Path {
        &self.storage_dir
    }

    fn update_current<F>(
        &self,
        record: &OperationRecord,
        update: F,
    ) -> Result<OperationRecord, OperationError>
    where
        F: FnOnce(&mut StoredRecord) -> Result<(), OperationError>,
    {
        self.ensure_storage()?;
        let path = self.record_path(&record.child_action_id)?;
        let _lock = lock(&path, LEDGER_BUSY)?;
        let ReadRecord::Record(mut stored) = read_record(&path)? else {
            return Err(OperationError::OperationInvalidState);
        };
        if stored.generation != record.generation {
            return Err(OperationError::OperationSuperseded);
        }
        update(&mut stored)?;
        write_record(&path, &stored)?;
        Ok((*stored).into_operation(record.operation_key.clone()))
    }

    fn load_or_create_fingerprint_key(&self) -> Result<[u8; KEY_BYTES], OperationError> {
        let path = self.storage_dir.join("operation-fingerprint.key");
        let _lock = lock(&path, KEY_BUSY)?;
        if path.exists() {
            #[cfg(unix)]
            {
                let metadata = path.metadata().map_err(|_| unavailable(KEY_UNREADABLE))?;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(unavailable(KEY_UNSAFE));
                }
            }
            let bytes = fs::read(&path).map_err(|_| unavailable(KEY_UNREADABLE))?;
            return bytes.try_into().map_err(|_| unavailable(KEY_INVALID));
        }
        if has_operation_artifacts(&self.operations_dir()) {
            return Err(unavailable(KEY_UNAVAILABLE));
        }
        let mut key = [0_u8; KEY_BYTES];
        random_fill(&mut key).map_err(|_| unavailable(KEY_UNAVAILABLE))?;
        atomic_replace(&path, &key, AtomicWriteOptions { mode: Some(0o600) })
            .map_err(|_| unavailable(KEY_UNAVAILABLE))?;
        Ok(key)
    }

    fn ensure_storage(&self) -> Result<(), OperationError> {
        fs::create_dir_all(&self.storage_dir).map_err(|_| unavailable(KEY_UNAVAILABLE))
    }

    fn operations_dir(&self) -> PathBuf {
        self.storage_dir.join("operations")
    }

    fn record_path(&self, child_action_id: &str) -> Result<PathBuf, OperationError> {
        if !action_id_is_valid(child_action_id) {
            return Err(unavailable(ACTION_ID_INVALID));
        }
        Ok(self
            .operations_dir()
            .join(format!("{child_action_id}.json")))
    }
}

impl StoredRecord {
    fn into_operation(self, operation_key: Option<String>) -> OperationRecord {
        OperationRecord {
            parent_action_id: self.parent_action_id,
            child_action_id: self.child_action_id,
            verb: self.verb,
            principal_tag: self.principal_tag,
            canonical_fingerprint: self.canonical_fingerprint,
            state: self.state,
            generation: self.generation,
            lease_id: self.lease_id,
            lease_expires_at: self.lease_expires_at,
            remote_operation_id: self.remote_operation_id,
            ack_state: self.ack_state,
            completed_at: self.completed_at,
            created_at: self.created_at,
            terminal_reason: self.terminal_reason,
            operation_key,
        }
    }
}

fn read_record(path: &Path) -> Result<ReadRecord, OperationError> {
    if !path.exists() {
        return Ok(ReadRecord::Missing);
    }
    let bytes = fs::read(path).map_err(|_| unavailable(RECORD_UNREADABLE))?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| unavailable(RECORD_UNREADABLE))?;
    let Some(object) = value.as_object() else {
        return Err(unavailable(RECORD_INVALID));
    };
    if object.get("schema_version") != Some(&Value::from(SCHEMA_VERSION)) {
        return Err(unavailable(RECORD_INVALID));
    }
    const RETIRED_KEYS: [&str; 3] = ["schema_version", "child_action_id", "terminal_reason"];
    const RECORD_KEYS: [&str; 15] = [
        "schema_version",
        "parent_action_id",
        "child_action_id",
        "verb",
        "principal_tag",
        "canonical_fingerprint",
        "state",
        "generation",
        "lease_id",
        "lease_expires_at",
        "remote_operation_id",
        "ack_state",
        "completed_at",
        "created_at",
        "terminal_reason",
    ];
    if exact_keys(object, &RETIRED_KEYS) {
        let child_action_id = object
            .get("child_action_id")
            .and_then(Value::as_str)
            .ok_or_else(|| unavailable(RECORD_INVALID))?
            .to_owned();
        let terminal_reason = object
            .get("terminal_reason")
            .and_then(Value::as_str)
            .ok_or_else(|| unavailable(RECORD_INVALID))?
            .to_owned();
        let _ = (child_action_id, terminal_reason);
        return Ok(ReadRecord::Retired);
    }
    if !exact_keys(object, &RECORD_KEYS) {
        return Err(unavailable(RECORD_INVALID));
    }
    let stored = serde_json::from_value(value).map_err(|_| unavailable(RECORD_INVALID))?;
    Ok(ReadRecord::Record(Box::new(stored)))
}

fn exact_keys(object: &serde_json::Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

fn write_record(path: &Path, record: &StoredRecord) -> Result<(), OperationError> {
    let value = serde_json::to_value(record).map_err(|_| unavailable(RECORD_INVALID))?;
    write_json_line(path, &value)
}

fn write_json_line(path: &Path, value: &Value) -> Result<(), OperationError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| unavailable(RECORD_INVALID))?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(|_| unavailable(RECORD_UNREADABLE))
}

fn lock(
    path: &Path,
    message: &'static str,
) -> Result<solstone_core_journal_io::FileLock, OperationError> {
    hold_lock(
        path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|error| match error {
        LockError::Timeout(_) => unavailable(message),
        LockError::Io { .. } => unavailable(message),
    })
}

fn has_operation_artifacts(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_file() || (path.is_dir() && has_operation_artifacts(&path))
    })
}

fn json_record_paths(path: &Path) -> Result<Vec<PathBuf>, OperationError> {
    let entries = fs::read_dir(path).map_err(|_| unavailable(RECORD_UNREADABLE))?;
    Ok(entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect())
}

fn action_id_is_valid(value: &str) -> bool {
    value.strip_prefix("sact1_").is_some_and(|tail| {
        !tail.is_empty()
            && tail
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn terminal_reason_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn lease_is_live(record: &StoredRecord, now: DateTime<Utc>) -> Result<bool, OperationError> {
    let Some(expires) = record.lease_expires_at.as_deref() else {
        return Ok(false);
    };
    Ok(record.lease_id.is_some() && parse_iso(expires)? > now)
}

fn parse_iso(value: &str) -> Result<DateTime<Utc>, OperationError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| unavailable(TIMESTAMP_INVALID))
}

fn iso(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.f%:z").to_string()
}

fn new_lease_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn terminal_failure_error(reason: Option<&str>) -> OperationError {
    match reason {
        Some("idempotency_conflict") => OperationError::IdempotencyConflict,
        Some("operation_retired") => OperationError::OperationRetired,
        Some("operation_erased") => OperationError::OperationErased,
        Some("tos_changed") => OperationError::OperationTosChanged,
        _ => OperationError::OperationInvalidState,
    }
}

fn unavailable(message: &'static str) -> OperationError {
    OperationError::OperationStateUnavailable { message }
}

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod ledger_tests;
