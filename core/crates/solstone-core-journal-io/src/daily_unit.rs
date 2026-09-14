// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Revision-aware durable daily unit records.
//!
//! A daily unit record captures both the in-flight attempt (for crash recovery,
//! short-lock fencing, and telemetry) and the last accepted committed result
//! (for revision-aware reuse).

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic::{JsonWriteOptions, write_json};
use crate::errors::{AtomicWriteError, LockError};
use crate::locking::{DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT, LockOptions, hold_lock};

pub const DAILY_UNIT_RECORD_VERSION: u32 = 1;

/// Identifies a discrete daily unit within a chronicle day or maintenance scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DailyUnitIdentity {
    pub day: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
}

impl DailyUnitIdentity {
    pub fn new(day: impl Into<String>, name: impl Into<String>, facet: Option<String>) -> Self {
        let name = name.into();
        let day = if name == "daily_schedule" {
            String::new()
        } else {
            day.into()
        };
        Self { day, name, facet }
    }
}

/// Execution and terminal disposition for a daily unit attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DailyUnitStatus {
    Unfinished,
    Committed,
    CommittedNoOutput,
    Failed,
    Conflicting,
    Capped,
}

impl DailyUnitStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Unfinished)
    }

    pub fn is_terminal_success(self) -> bool {
        matches!(self, Self::Committed | Self::CommittedNoOutput)
    }

    pub fn is_terminal_success_or_capped(self) -> bool {
        matches!(
            self,
            Self::Committed | Self::CommittedNoOutput | Self::Capped
        )
    }

    pub fn is_success(self) -> bool {
        self.is_terminal_success()
    }
}

/// Last accepted and committed result for this daily unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedDailyResult {
    pub evidence_revision: String,
    pub contract_digest: String,
    pub status: DailyUnitStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipts: Vec<serde_json::Value>,
    pub committed_at_ms: i64,
}

impl AcceptedDailyResult {
    fn has_valid_proof(&self) -> bool {
        let digest_valid = self.packet_digest.as_deref().is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        let response_valid = self.generated_result.as_ref().is_some_and(|result| {
            result
                .get("response")
                .is_some_and(serde_json::Value::is_string)
                && result
                    .get("output")
                    .is_some_and(serde_json::Value::is_string)
        });
        if !digest_valid || !response_valid {
            return false;
        }
        let mut actions = 0usize;
        for receipt in &self.receipts {
            match receipt.get("kind").and_then(serde_json::Value::as_str) {
                Some("owner_action") => {
                    actions += 1;
                    if receipt.get("state").and_then(serde_json::Value::as_str) != Some("committed")
                        || !receipt
                            .get("action_id")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| !s.is_empty())
                        || !receipt
                            .get("token")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|s| !s.is_empty())
                    {
                        return false;
                    }
                }
                Some("required_artifact") => {}
                _ => return false,
            }
        }
        match self.status {
            DailyUnitStatus::Committed => actions > 0,
            DailyUnitStatus::CommittedNoOutput => self.receipts.is_empty(),
            _ => false,
        }
    }
}

/// Durable record for a daily unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyUnitRecord {
    pub version: u32,
    pub identity: DailyUnitIdentity,
    pub status: DailyUnitStatus,
    pub evidence_revision: String,
    pub contract_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_packet: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_plan: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipts: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<AcceptedDailyResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    pub attempts: u32,
    #[serde(default)]
    pub failure_count: u32,
    #[serde(default)]
    pub environmental_retry_day: Option<String>,
    pub updated_at_ms: i64,
}

impl DailyUnitRecord {
    pub fn new(
        identity: DailyUnitIdentity,
        evidence_revision: impl Into<String>,
        contract_digest: impl Into<String>,
    ) -> Self {
        Self {
            version: DAILY_UNIT_RECORD_VERSION,
            identity,
            status: DailyUnitStatus::Unfinished,
            evidence_revision: evidence_revision.into(),
            contract_digest: contract_digest.into(),
            frozen_packet: None,
            packet_digest: None,
            use_id: None,
            lock_token: None,
            generated_result: None,
            action_plan: None,
            receipts: Vec::new(),
            accepted: None,
            reason_code: None,
            error_detail: None,
            attempts: 0,
            failure_count: 0,
            environmental_retry_day: None,
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Unit reuse is strictly evidence-bound (E + contract digest).
    /// Whole-day raw input guards remain separate from semantic reuse.
    pub fn is_reusable_for(&self, evidence_rev: &str, contract_dig: &str) -> bool {
        self.accepted.as_ref().is_some_and(|acc| {
            acc.evidence_revision == evidence_rev
                && acc.contract_digest == contract_dig
                && acc.status.is_terminal_success()
                && acc.has_valid_proof()
        })
    }
}

#[derive(Debug)]
pub enum DailyUnitError {
    Io(io::Error),
    Lock(LockError),
    AtomicWrite(AtomicWriteError),
    Malformed(String),
    Json(serde_json::Error),
}

impl fmt::Display for DailyUnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Lock(err) => write!(f, "Lock error: {err}"),
            Self::AtomicWrite(err) => write!(f, "Atomic write error: {err}"),
            Self::Malformed(msg) => write!(f, "Malformed daily unit record: {msg}"),
            Self::Json(err) => write!(f, "JSON error: {err}"),
        }
    }
}

impl std::error::Error for DailyUnitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Lock(err) => Some(err),
            Self::AtomicWrite(err) => Some(err),
            Self::Malformed(_) => None,
            Self::Json(err) => Some(err),
        }
    }
}

impl From<io::Error> for DailyUnitError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<LockError> for DailyUnitError {
    fn from(err: LockError) -> Self {
        Self::Lock(err)
    }
}

impl From<AtomicWriteError> for DailyUnitError {
    fn from(err: AtomicWriteError) -> Self {
        Self::AtomicWrite(err)
    }
}

impl From<serde_json::Error> for DailyUnitError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

/// Portable percent-encoding for unit identity components (avoids colons, slashes, etc. on Windows).
pub fn encode_filename_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match *b {
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => out.push(*b as char),
            b => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

/// Decodes a percent-encoded unit identity component.
pub fn decode_filename_component(s: &str) -> Result<String, DailyUnitError> {
    let mut bytes = Vec::new();
    let mut iter = s.as_bytes().iter().copied();
    while let Some(b) = iter.next() {
        if b == b'%' {
            let h1 = iter.next().ok_or_else(|| {
                DailyUnitError::Malformed("truncated percent-encoding".to_owned())
            })?;
            let h2 = iter.next().ok_or_else(|| {
                DailyUnitError::Malformed("truncated percent-encoding".to_owned())
            })?;
            let hex_buf = [h1, h2];
            let hex_str = std::str::from_utf8(&hex_buf)
                .map_err(|_| DailyUnitError::Malformed("invalid hex utf8".to_owned()))?;
            let byte = u8::from_str_radix(hex_str, 16)
                .map_err(|_| DailyUnitError::Malformed("invalid hex byte".to_owned()))?;
            bytes.push(byte);
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes)
        .map_err(|e| DailyUnitError::Malformed(format!("invalid utf8 decoded string: {e}")))
}

/// Resolves the directory for daily unit records for a specific day.
pub fn daily_unit_record_dir(journal: &Path, day: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join(day)
        .join("health")
        .join("daily-units")
}

/// Resolves the global maintenance path for a maintenance unit.
pub fn maintenance_unit_record_path(journal: &Path, name: &str) -> PathBuf {
    journal
        .join("health")
        .join("maintenance")
        .join(format!("{name}.json"))
}

/// Resolves the exact file path for a daily unit record with portable encoded names.
pub fn daily_unit_record_path(journal: &Path, identity: &DailyUnitIdentity) -> PathBuf {
    if identity.name == "daily_schedule" {
        return maintenance_unit_record_path(journal, &identity.name);
    }
    let enc_name = encode_filename_component(&identity.name);
    let mut filename = match &identity.facet {
        Some(facet) => format!("u-{}.f-{}.json", enc_name, encode_filename_component(facet)),
        None => format!("u-{}.json", enc_name),
    };
    // Leave room for the lock and atomic-write names even when a valid facet
    // already approaches the filesystem's component limit. The distinct prefix
    // cannot collide with the readable name namespace.
    if filename.len() > 128 {
        use sha2::{Digest, Sha256};
        let key = serde_json::to_vec(identity).expect("daily unit identity serializes");
        filename = format!("h-{:x}.json", Sha256::digest(key));
    }
    daily_unit_record_dir(journal, &identity.day).join(filename)
}

/// Reads a daily unit record from a specific path, verifying record version.
pub fn read_daily_unit_record(path: &Path) -> Result<Option<DailyUnitRecord>, DailyUnitError> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(DailyUnitError::Io(e)),
    };
    let record: DailyUnitRecord = serde_json::from_str(&content)
        .map_err(|e| DailyUnitError::Malformed(format!("{}: {}", path.display(), e)))?;
    if record.version != DAILY_UNIT_RECORD_VERSION {
        return Err(DailyUnitError::Malformed(format!(
            "{}: unsupported version {}",
            path.display(),
            record.version
        )));
    }
    Ok(Some(record))
}

fn validate_identity(identity: &DailyUnitIdentity) -> Result<(), DailyUnitError> {
    let valid_day = if identity.name == "daily_schedule" {
        identity.day.is_empty() && identity.facet.is_none()
    } else {
        identity.day.len() == 8
            && identity.day.bytes().all(|b| b.is_ascii_digit())
            && chrono::NaiveDate::parse_from_str(&identity.day, "%Y%m%d").is_ok()
    };
    if !valid_day || identity.name.is_empty() || identity.facet.as_deref() == Some("") {
        return Err(DailyUnitError::Malformed(
            "invalid daily unit identity".into(),
        ));
    }
    Ok(())
}

/// Loads a daily unit record by identity and verifies identity match.
pub fn load_daily_unit_record(
    journal: &Path,
    identity: &DailyUnitIdentity,
) -> Result<Option<DailyUnitRecord>, DailyUnitError> {
    validate_identity(identity)?;
    let path = daily_unit_record_path(journal, identity);
    let record = read_daily_unit_record(&path)?;
    if let Some(ref rec) = record
        && rec.identity != *identity
    {
        return Err(DailyUnitError::Malformed(format!(
            "identity mismatch in record at {}: expected {:?}, found {:?}",
            path.display(),
            identity,
            rec.identity
        )));
    }
    Ok(record)
}

/// A unit's held publication authority. Checkpoints publish to disk without releasing
/// this lock; callers retain it through the actual owner write and its receipt.
pub struct DailyUnitAuthority {
    path: PathBuf,
    identity: DailyUnitIdentity,
    record: Option<DailyUnitRecord>,
}

impl DailyUnitAuthority {
    pub fn record(&self) -> Option<&DailyUnitRecord> {
        self.record.as_ref()
    }
    pub fn record_mut(&mut self) -> &mut Option<DailyUnitRecord> {
        &mut self.record
    }
    pub fn require_token(&self, token: &str) -> Result<(), DailyUnitError> {
        let record = self
            .record
            .as_ref()
            .ok_or_else(|| DailyUnitError::Malformed("missing publication authority".into()))?;
        if record.lock_token.as_deref() != Some(token) || token.is_empty() {
            return Err(DailyUnitError::Malformed(
                "publication token was replaced".into(),
            ));
        }
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<(), DailyUnitError> {
        let record = self
            .record
            .as_ref()
            .ok_or_else(|| DailyUnitError::Malformed("missing checkpoint record".into()))?;
        if record.identity != self.identity || record.version != DAILY_UNIT_RECORD_VERSION {
            return Err(DailyUnitError::Malformed(
                "checkpoint identity/version mismatch".into(),
            ));
        }
        write_json(
            &self.path,
            record,
            JsonWriteOptions {
                indent: Some(2),
                sort_keys: false,
                mode: Some(0o600),
            },
        )?;
        Ok(())
    }
}

/// Read and mutate one authority. The callback explicitly checkpoints before
/// side effects; an error never publishes uncheckpointed in-memory changes.
pub fn with_daily_unit_authority<F, T>(
    journal: &Path,
    identity: &DailyUnitIdentity,
    f: F,
) -> Result<T, DailyUnitError>
where
    F: FnOnce(&mut DailyUnitAuthority) -> Result<T, DailyUnitError>,
{
    validate_identity(identity)?;
    let path = daily_unit_record_path(journal, identity);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _lock = hold_lock(
        path.with_extension("lock"),
        LockOptions {
            timeout: DEFAULT_LOCK_TIMEOUT,
            poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
            mode: Some(0o600),
        },
    )?;
    let record = load_daily_unit_record(journal, identity)?;
    let mut authority = DailyUnitAuthority {
        path,
        identity: identity.clone(),
        record,
    };
    f(&mut authority)
}

/// Atomic reservation/update under the same authority used for publication.
pub fn with_locked_daily_unit_record<F, T>(
    journal: &Path,
    identity: &DailyUnitIdentity,
    f: F,
) -> Result<T, DailyUnitError>
where
    F: FnOnce(&mut Option<DailyUnitRecord>) -> Result<T, DailyUnitError>,
{
    with_daily_unit_authority(journal, identity, |authority| {
        let result = f(authority.record_mut())?;
        if authority.record().is_some() {
            authority.checkpoint()?;
        }
        Ok(result)
    })
}

/// Writes a daily unit record directly using atomic replace under a lock.
pub fn write_daily_unit_record(
    path: &Path,
    record: &DailyUnitRecord,
) -> Result<(), DailyUnitError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("lock");
    let _lock = hold_lock(
        &lock_path,
        LockOptions {
            timeout: DEFAULT_LOCK_TIMEOUT,
            poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
            mode: None,
        },
    )?;
    write_json(
        path,
        record,
        JsonWriteOptions {
            indent: Some(2),
            sort_keys: false,
            mode: None,
        },
    )?;
    Ok(())
}

/// Saves a daily unit record to its canonical journal path.
pub fn save_daily_unit_record(
    journal: &Path,
    record: &DailyUnitRecord,
) -> Result<(), DailyUnitError> {
    with_locked_daily_unit_record(journal, &record.identity, |slot| {
        *slot = Some(record.clone());
        Ok(())
    })
}

/// Lists all daily unit records for a day.
pub fn list_daily_unit_records(
    journal: &Path,
    day: &str,
) -> Result<Vec<DailyUnitRecord>, DailyUnitError> {
    let dir = daily_unit_record_dir(journal, day);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json")
            && let Some(rec) = read_daily_unit_record(&path)?
        {
            if rec.identity.day != day || daily_unit_record_path(journal, &rec.identity) != path {
                return Err(DailyUnitError::Malformed(format!(
                    "misplaced record {}",
                    path.display()
                )));
            }
            records.push(rec);
        }
    }
    records.sort_by(|a, b| a.identity.cmp(&b.identity));
    Ok(records)
}

/// Verify replace-only artifacts without treating historical domain receipts as
/// permission to recreate observations, calendar entries, or owner config.
pub fn accepted_daily_artifacts_valid(
    journal: &Path,
    record: &DailyUnitRecord,
) -> Result<bool, DailyUnitError> {
    use sha2::{Digest, Sha256};
    let Some(accepted) = &record.accepted else {
        return Ok(false);
    };
    if !accepted.has_valid_proof() {
        return Err(DailyUnitError::Malformed(
            "invalid accepted daily result proof".into(),
        ));
    }
    let mut proofs = Vec::new();
    for receipt in &accepted.receipts {
        if receipt.get("kind").and_then(serde_json::Value::as_str) == Some("required_artifact") {
            let path = receipt
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| DailyUnitError::Malformed("artifact receipt lacks path".into()))?;
            let sha = receipt
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| DailyUnitError::Malformed("artifact receipt lacks digest".into()))?;
            proofs.push((path, sha));
        }
    }
    for (relative, expected) in proofs {
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(DailyUnitError::Malformed("invalid artifact path".into()));
        }
        let bytes = match fs::read(journal.join(path)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if format!("{:x}", Sha256::digest(&bytes)) != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn malformed_accepted_success_cannot_certify_coverage() {
        let identity = DailyUnitIdentity::new("20260910", "schedule", None);
        let mut record = DailyUnitRecord::new(identity, "E", "C");
        let mut accepted = AcceptedDailyResult {
            evidence_revision: "E".into(),
            contract_digest: "C".into(),
            status: DailyUnitStatus::Committed,
            packet_digest: None,
            generated_result: None,
            receipts: Vec::new(),
            committed_at_ms: 1,
        };
        record.accepted = Some(accepted.clone());
        assert!(!record.is_reusable_for("E", "C"));
        accepted.packet_digest = Some("a".repeat(64));
        accepted.generated_result = Some(serde_json::json!({"response":"[]","output":"[]"}));
        record.accepted = Some(accepted.clone());
        assert!(!record.is_reusable_for("E", "C"));
        accepted.status = DailyUnitStatus::CommittedNoOutput;
        record.accepted = Some(accepted);
        assert!(record.is_reusable_for("E", "C"));
        // A newer pending packet does not change the accepted historical proof.
        record.evidence_revision = "E2".into();
        record.packet_digest = Some("b".repeat(64));
        assert!(record.is_reusable_for("E", "C"));
        assert!(!record.is_reusable_for("E2", "C"));
    }

    #[test]
    fn checkpoint_survives_callback_failure_before_owner_publication() {
        let root = tempdir().unwrap();
        let identity = DailyUnitIdentity::new("20260910", "schedule", None);
        let failure = with_daily_unit_authority(root.path(), &identity, |authority| {
            let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
            record.generated_result = Some(serde_json::json!({"output":"retained"}));
            record.action_plan = Some(serde_json::json!({"actions":[]}));
            *authority.record_mut() = Some(record);
            authority.checkpoint()?;
            let durable = load_daily_unit_record(root.path(), &identity)?.unwrap();
            assert!(durable.generated_result.is_some());
            assert!(durable.action_plan.is_some());
            Err::<(), _>(DailyUnitError::Malformed(
                "interrupted before owner write".into(),
            ))
        });
        assert!(failure.is_err());
        assert!(
            load_daily_unit_record(root.path(), &identity)
                .unwrap()
                .unwrap()
                .generated_result
                .is_some()
        );
    }

    #[test]
    fn maintenance_authority_is_shared_across_historical_days_and_paths_are_portable() {
        let root = tempdir().unwrap();
        let first = DailyUnitIdentity::new("20260910", "daily_schedule", None);
        let later = DailyUnitIdentity::new("20260911", "daily_schedule", None);
        assert_eq!(first, later);
        save_daily_unit_record(
            root.path(),
            &DailyUnitRecord::new(first, "window", "contract"),
        )
        .unwrap();
        assert!(
            load_daily_unit_record(root.path(), &later)
                .unwrap()
                .is_some()
        );
        let upper = DailyUnitIdentity::new("20260910", "CON", None);
        let lower = DailyUnitIdentity::new("20260910", "con", None);
        let a = daily_unit_record_path(root.path(), &upper)
            .to_string_lossy()
            .to_lowercase();
        let b = daily_unit_record_path(root.path(), &lower)
            .to_string_lossy()
            .to_lowercase();
        assert_ne!(a, b);
        assert!(b.ends_with("u-con.json"));
    }

    #[test]
    fn misplaced_or_unreadable_record_directory_is_not_empty_coverage() {
        let root = tempdir().unwrap();
        let dir = daily_unit_record_dir(root.path(), "20260910");
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::write(&dir, "not a directory").unwrap();
        assert!(list_daily_unit_records(root.path(), "20260910").is_err());
    }

    #[test]
    fn test_daily_unit_record_round_trip() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let identity = DailyUnitIdentity::new("20260813", "schedule", Some("work".to_owned()));

        let record = DailyUnitRecord {
            version: DAILY_UNIT_RECORD_VERSION,
            identity: identity.clone(),
            status: DailyUnitStatus::Unfinished,
            evidence_revision: "sha-evidence-1".to_owned(),
            contract_digest: "sha-contract-1".to_owned(),
            frozen_packet: Some(serde_json::json!({"prompt": "test"})),
            packet_digest: Some("sha-packet-1".to_owned()),
            use_id: Some("use-123".to_owned()),
            lock_token: Some("token-abc".to_owned()),
            generated_result: None,
            action_plan: None,
            receipts: Vec::new(),
            accepted: None,
            reason_code: None,
            error_detail: None,
            attempts: 1,
            failure_count: 0,
            environmental_retry_day: None,
            updated_at_ms: 1000,
        };

        save_daily_unit_record(journal, &record).unwrap();
        let loaded = load_daily_unit_record(journal, &identity).unwrap().unwrap();
        assert_eq!(loaded, record);
        assert!(!loaded.is_reusable_for("sha-evidence-1", "sha-contract-1"));

        // Now test accepted result reuse
        let mut committed = record.clone();
        committed.status = DailyUnitStatus::Committed;
        committed.accepted = Some(AcceptedDailyResult {
            evidence_revision: "sha-evidence-1".to_owned(),
            contract_digest: "sha-contract-1".to_owned(),
            status: DailyUnitStatus::Committed,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(
                serde_json::json!({"response": "# Schedule", "output":"# Schedule"}),
            ),
            receipts: vec![
                serde_json::json!({"kind":"owner_action", "action_id":"fixture", "token":"attempt", "state":"committed"}),
            ],
            committed_at_ms: 1050,
        });

        save_daily_unit_record(journal, &committed).unwrap();
        let loaded = load_daily_unit_record(journal, &identity).unwrap().unwrap();
        assert!(loaded.is_reusable_for("sha-evidence-1", "sha-contract-1"));
        assert!(!loaded.is_reusable_for("sha-evidence-2", "sha-contract-1"));
        assert!(!loaded.is_reusable_for("sha-evidence-1", "sha-contract-2"));
    }

    #[test]
    fn test_maintenance_unit_path() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let identity = DailyUnitIdentity::new("20260813", "daily_schedule", None);
        let path = daily_unit_record_path(journal, &identity);
        assert_eq!(
            path,
            journal
                .join("health")
                .join("maintenance")
                .join("daily_schedule.json")
        );
    }

    #[test]
    fn test_colon_talent_name_portable_encoding_and_roundtrip() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let identity = DailyUnitIdentity::new(
            "20260813",
            "entities:entities_review",
            Some("work".to_owned()),
        );
        let path = daily_unit_record_path(journal, &identity);
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(file_name, "u-entities%3Aentities_review.f-work.json");

        let record = DailyUnitRecord::new(identity.clone(), "ev-rev", "ct-dig");
        save_daily_unit_record(journal, &record).unwrap();

        let loaded = load_daily_unit_record(journal, &identity).unwrap().unwrap();
        assert_eq!(loaded.identity, identity);

        let listed = list_daily_unit_records(journal, "20260813").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].identity, identity);
    }

    #[test]
    fn long_facet_record_names_round_trip_without_component_overflow() {
        let dir = tempdir().unwrap();
        let facets = ["a".repeat(240), format!("{}b", "a".repeat(239))];
        let mut paths = Vec::new();
        for facet in facets {
            let facet_dir = dir.path().join("facets").join(&facet);
            fs::create_dir_all(&facet_dir).unwrap();
            fs::write(facet_dir.join("facet.json"), "{}").unwrap();
            let identity =
                DailyUnitIdentity::new("20260813", "entities:entity_observer", Some(facet));
            let record = DailyUnitRecord::new(identity.clone(), "E", "C");
            save_daily_unit_record(dir.path(), &record).unwrap();
            assert_eq!(
                load_daily_unit_record(dir.path(), &identity).unwrap(),
                Some(record)
            );
            let path = daily_unit_record_path(dir.path(), &identity);
            assert!(path.file_name().unwrap().len() <= 128);
            paths.push(path);
        }
        assert_ne!(paths[0], paths[1]);
        assert_eq!(
            list_daily_unit_records(dir.path(), "20260813")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn test_read_rejects_unsupported_version() {
        let dir = tempdir().unwrap();
        let record_path = dir.path().join("test.json");
        let json = r#"{
            "version": 99,
            "identity": {"day": "20260813", "name": "schedule"},
            "status": "unfinished",
            "evidence_revision": "e",
            "contract_digest": "c",
            "attempts": 0,
            "updated_at_ms": 0
        }"#;
        fs::write(&record_path, json).unwrap();
        let err = read_daily_unit_record(&record_path).unwrap_err();
        assert!(
            matches!(err, DailyUnitError::Malformed(ref msg) if msg.contains("unsupported version 99"))
        );
    }

    #[test]
    fn test_load_rejects_identity_mismatch() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let expected_identity = DailyUnitIdentity::new("20260813", "schedule", None);
        let mismatched_record = DailyUnitRecord::new(
            DailyUnitIdentity::new("20260813", "other_talent", None),
            "e",
            "c",
        );
        let path = daily_unit_record_path(journal, &expected_identity);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_daily_unit_record(&path, &mismatched_record).unwrap();

        let err = load_daily_unit_record(journal, &expected_identity).unwrap_err();
        assert!(
            matches!(err, DailyUnitError::Malformed(ref msg) if msg.contains("identity mismatch"))
        );
    }

    #[test]
    fn test_with_locked_daily_unit_record_fencing_and_no_deadlock() {
        let dir = tempdir().unwrap();
        let journal = dir.path();
        let identity = DailyUnitIdentity::new("20260813", "schedule", None);

        // Initial creation
        with_locked_daily_unit_record(journal, &identity, |slot| {
            let mut rec = DailyUnitRecord::new(identity.clone(), "e1", "c1");
            rec.lock_token = Some("token-1".to_owned());
            *slot = Some(rec);
            Ok(())
        })
        .unwrap();

        // Mismatched token cannot commit
        let err = with_locked_daily_unit_record(journal, &identity, |slot| {
            let rec = slot.as_mut().unwrap();
            if rec.lock_token.as_deref() != Some("stale-token") {
                return Err(DailyUnitError::Malformed("lock token mismatch".to_owned()));
            }
            rec.status = DailyUnitStatus::Committed;
            Ok(())
        });
        assert!(err.is_err());

        // Replacement token wins and commits without deadlock
        with_locked_daily_unit_record(journal, &identity, |slot| {
            let rec = slot.as_mut().unwrap();
            assert_eq!(rec.lock_token.as_deref(), Some("token-1"));
            rec.lock_token = Some("token-2".to_owned());
            rec.status = DailyUnitStatus::Committed;
            Ok(())
        })
        .unwrap();

        let loaded = load_daily_unit_record(journal, &identity).unwrap().unwrap();
        assert_eq!(loaded.status, DailyUnitStatus::Committed);
        assert_eq!(loaded.lock_token.as_deref(), Some("token-2"));
    }
}
