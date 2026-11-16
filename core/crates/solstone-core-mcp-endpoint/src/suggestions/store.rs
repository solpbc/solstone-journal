// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::atomic::{
    AtomicWriteOptions, DetailedAtomicError, write_bytes_exclusive_detailed,
};
use solstone_core_journal_io::{
    AtomicWriteError, FileLock, JsonWriteOptions, LockError, LockOptions,
    create_directory_with_mode, hold_lock, write_json,
};

use super::SETTINGS_SCHEMA;

const BASE_DIR: &str = "mcp-endpoint/correction-suggestions";
const RECORDS_DIR: &str = "records";
const SETTINGS_FILE: &str = "settings.json";
const LOCK_FILE: &str = "suggestions.lock";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("directory creation error: {0}")]
    Directory(io::Error),
    #[error("lock error: {0}")]
    Lock(#[from] LockError),
    #[error("io error on {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("corrupt settings file")]
    CorruptSettings,
    #[error("corrupt record file: {0}")]
    CorruptRecord(PathBuf),
    #[error("atomic write error: {0}")]
    AtomicWrite(#[from] AtomicWriteError),
    #[error("detailed atomic error: {0}")]
    DetailedAtomic(#[from] DetailedAtomicError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsRecord {
    pub schema: u32,
    #[serde(default)]
    pub enabled: BTreeMap<String, bool>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

impl Default for SettingsRecord {
    fn default() -> Self {
        Self {
            schema: SETTINGS_SCHEMA,
            enabled: BTreeMap::new(),
            labels: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SuggestionReceipt {
    #[serde(default)]
    pub incomplete: bool,
    #[serde(default)]
    pub undo_started: bool,
    #[serde(default)]
    pub undo_asked: Option<bool>,
    #[serde(default)]
    pub undo_ask_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stretch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_position: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_reverts: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_version_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_after: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_before: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionRecord {
    pub id: String,
    pub schema: u32,
    pub status: String,
    pub kind: String,
    pub basis: String,
    pub reason: String,
    pub value: Value,
    pub operation_id: String,
    pub request_sha256: String,
    pub similarity_key: String,
    pub authority_id: String,
    pub source: String,
    pub label: String,
    pub created_at: String,
    pub updated_at: String,
    pub target: Value,
    pub baseline: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_stretch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<SuggestionReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_reason: Option<String>,
    pub tool_result: Value,
}

pub fn suggestions_dir(journal_root: &Path) -> PathBuf {
    journal_root.join(BASE_DIR)
}

pub fn records_dir(journal_root: &Path) -> PathBuf {
    suggestions_dir(journal_root).join(RECORDS_DIR)
}

pub fn settings_path(journal_root: &Path) -> PathBuf {
    suggestions_dir(journal_root).join(SETTINGS_FILE)
}

pub fn lock_path(journal_root: &Path) -> PathBuf {
    suggestions_dir(journal_root).join(LOCK_FILE)
}

pub fn record_path(journal_root: &Path, id: &str) -> PathBuf {
    records_dir(journal_root).join(format!("{id}.json"))
}

pub fn ensure_suggestions_directories(journal_root: &Path) -> Result<(), StoreError> {
    create_directory_with_mode(&suggestions_dir(journal_root), 0o700)
        .map_err(|e| StoreError::Directory(io::Error::other(e.to_string())))?;
    create_directory_with_mode(&records_dir(journal_root), 0o700)
        .map_err(|e| StoreError::Directory(io::Error::other(e.to_string())))?;
    Ok(())
}

pub fn hold_suggestions_lock(journal_root: &Path) -> Result<FileLock, StoreError> {
    ensure_suggestions_directories(journal_root)?;
    let path = lock_path(journal_root);
    let held = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )?;
    Ok(held)
}

pub fn read_settings(journal_root: &Path) -> Result<SettingsRecord, StoreError> {
    let path = settings_path(journal_root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(SettingsRecord::default());
        }
        Err(err) => return Err(StoreError::Io { path, source: err }),
    };
    serde_json::from_slice::<SettingsRecord>(&bytes).map_err(|_| StoreError::CorruptSettings)
}

pub fn write_settings(journal_root: &Path, settings: &SettingsRecord) -> Result<(), StoreError> {
    ensure_suggestions_directories(journal_root)?;
    let path = settings_path(journal_root);
    if path.exists() {
        let bytes = fs::read(&path).map_err(|err| StoreError::Io {
            path: path.clone(),
            source: err,
        })?;
        if serde_json::from_slice::<SettingsRecord>(&bytes).is_err() {
            return Err(StoreError::CorruptSettings);
        }
    }
    write_json(
        &path,
        settings,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            ..Default::default()
        },
    )?;
    Ok(())
}

pub fn update_connection_enabled(
    journal_root: &Path,
    authority_id: &str,
    enabled: bool,
    label: Option<&str>,
) -> Result<(), StoreError> {
    let _lock = hold_suggestions_lock(journal_root)?;
    let mut settings = read_settings(journal_root)?;
    settings.enabled.insert(authority_id.to_owned(), enabled);
    if let Some(label) = label {
        let source_hex = crate::agent_state::memory_source(journal_root, authority_id)
            .map(|s| s.component().to_owned())
            .unwrap_or_default();
        if !source_hex.is_empty() {
            settings.labels.insert(source_hex, label.to_owned());
        }
    }
    write_settings(journal_root, &settings)
}

pub fn update_stored_label(
    journal_root: &Path,
    source_hex: &str,
    label: &str,
) -> Result<(), StoreError> {
    let _lock = hold_suggestions_lock(journal_root)?;
    let mut settings = read_settings(journal_root)?;
    settings
        .labels
        .insert(source_hex.to_owned(), label.to_owned());
    write_settings(journal_root, &settings)
}

pub fn read_record(journal_root: &Path, id: &str) -> Result<Option<SuggestionRecord>, StoreError> {
    if !valid_record_id(id) {
        return Ok(None);
    }
    let path = record_path(journal_root, id);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(StoreError::Io { path, source: err }),
    };
    let record = serde_json::from_slice::<SuggestionRecord>(&bytes)
        .map_err(|_| StoreError::CorruptRecord(path))?;
    if record.id != id || record.schema != super::SUGGESTION_SCHEMA {
        return Err(StoreError::CorruptRecord(record_path(journal_root, id)));
    }
    Ok(Some(record))
}

pub fn write_new_record(journal_root: &Path, record: &SuggestionRecord) -> Result<(), StoreError> {
    ensure_suggestions_directories(journal_root)?;
    let path = record_path(journal_root, &record.id);
    let bytes = serde_json::to_vec_pretty(record).map_err(|err| StoreError::Io {
        path: path.clone(),
        source: io::Error::new(io::ErrorKind::InvalidData, err),
    })?;
    write_bytes_exclusive_detailed(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })?;
    Ok(())
}

pub fn save_record(journal_root: &Path, record: &SuggestionRecord) -> Result<(), StoreError> {
    ensure_suggestions_directories(journal_root)?;
    let path = record_path(journal_root, &record.id);
    write_json(
        &path,
        record,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            ..Default::default()
        },
    )?;
    Ok(())
}

pub fn list_records(journal_root: &Path) -> Result<Vec<SuggestionRecord>, StoreError> {
    let dir = records_dir(journal_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let read_dir = fs::read_dir(&dir).map_err(|source| StoreError::Io {
        path: dir.clone(),
        source,
    })?;
    let mut records = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|source| StoreError::Io {
            path: dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|id| valid_record_id(id))
            .ok_or_else(|| StoreError::CorruptRecord(path.clone()))?;
        let record = read_record(journal_root, id)?.ok_or(StoreError::CorruptRecord(path))?;
        records.push(record);
    }
    records.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(records)
}

fn valid_record_id(id: &str) -> bool {
    id.strip_prefix("sc_").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskOutcome {
    pub asked: bool,
    pub reason: Option<String>,
}

fn ask_outcome_path(root: &Path, id: &str, action: &str) -> PathBuf {
    assert!(valid_record_id(id) && matches!(action, "accept" | "undo"));
    suggestions_dir(root)
        .join("ask-outcomes")
        .join(format!("{id}-{action}.json"))
}

pub fn read_ask_outcome(
    root: &Path,
    id: &str,
    action: &str,
) -> Result<Option<AskOutcome>, StoreError> {
    let path = ask_outcome_path(root, id, action);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(StoreError::Io { path, source }),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| StoreError::CorruptRecord(path))
}

pub fn save_ask_outcome(
    root: &Path,
    id: &str,
    action: &str,
    outcome: &AskOutcome,
) -> Result<(), StoreError> {
    let path = ask_outcome_path(root, id, action);
    create_directory_with_mode(path.parent().expect("ask outcome directory"), 0o700)
        .map_err(|error| StoreError::Directory(io::Error::other(error.to_string())))?;
    write_json(
        &path,
        outcome,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            ..Default::default()
        },
    )?;
    Ok(())
}
