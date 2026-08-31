// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable awareness state and daily log storage.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AppendError, AtomicWriteError, JsonWriteOptions, LockError, LockOptions, MalformedPolicy,
    ReadError, append_jsonl, hold_lock, read_json, read_jsonl, write_json,
};

use crate::{FacetTrustLockError, hold_facet_trust_lock};

/// Failure while reading or mutating awareness state.
#[derive(Debug)]
pub enum AwarenessStoreError {
    TrustLock(FacetTrustLockError),
    Directory {
        path: PathBuf,
        source: std::io::Error,
    },
    Lock(LockError),
    Read(ReadError),
    Write(AtomicWriteError),
    Append(AppendError),
}

impl fmt::Display for AwarenessStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Directory { path, source } => {
                write!(
                    formatter,
                    "awareness directory failed at {}: {source}",
                    path.display()
                )
            }
            Self::Lock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Append(error) => error.fmt(formatter),
        }
    }
}

impl Error for AwarenessStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Directory { source, .. } => Some(source),
            Self::Lock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::Append(error) => Some(error),
        }
    }
}

/// Read materialized awareness state without creating the awareness directory.
///
/// Python's `get_journal()` creates this directory as a side effect. Native GET
/// paths deliberately do not reproduce that mutation.
pub fn load_current(journal_root: &Path) -> Result<Value, AwarenessStoreError> {
    read_json(
        current_path(journal_root),
        json!({}),
        MalformedPolicy::WarnAndSkip,
    )
    .map_err(AwarenessStoreError::Read)
}

/// Return the current import-tracking section or its Python-compatible default.
pub fn load_imports(journal_root: &Path) -> Result<Value, AwarenessStoreError> {
    let current = load_current(journal_root)?;
    Ok(current
        .get("imports")
        .cloned()
        .unwrap_or_else(default_imports))
}

/// Read one awareness daily log without creating the awareness directory.
pub fn read_log(journal_root: &Path, day: &str) -> Result<Vec<Value>, AwarenessStoreError> {
    read_jsonl(
        log_path(journal_root, day),
        Vec::new(),
        MalformedPolicy::WarnAndSkip,
    )
    .map_err(AwarenessStoreError::Read)
}

/// Append one entry to an awareness daily log using caller-provided time values.
#[allow(clippy::too_many_arguments)]
pub fn append_log(
    journal_root: &Path,
    kind: &str,
    key: Option<&str>,
    message: Option<&str>,
    data: Option<&Map<String, Value>>,
    day: &str,
    timestamp_ms: i64,
    extra: &Map<String, Value>,
) -> Result<Value, AwarenessStoreError> {
    let _trust = hold_facet_trust_lock(journal_root).map_err(AwarenessStoreError::TrustLock)?;
    append_log_locked(
        journal_root,
        kind,
        key,
        message,
        data,
        day,
        timestamp_ms,
        extra,
    )
}

/// Record a completed import and append its corresponding awareness event.
#[allow(clippy::too_many_arguments)]
pub fn record_import(
    journal_root: &Path,
    source_type: &str,
    source_display: Option<&str>,
    entries_written: i64,
    now_iso: &str,
    day: &str,
    timestamp_ms: i64,
) -> Result<Value, AwarenessStoreError> {
    let _trust = hold_facet_trust_lock(journal_root).map_err(AwarenessStoreError::TrustLock)?;
    let imports = update_imports(journal_root, |imports| {
        let sources = imports
            .entry("sources_used".to_owned())
            .or_insert_with(|| Value::Array(Vec::new()));
        if !sources.is_array() {
            *sources = Value::Array(Vec::new());
        }
        let sources = sources.as_array_mut().expect("sources_used was normalized");
        if !sources
            .iter()
            .any(|source| source.as_str() == Some(source_type))
        {
            sources.push(Value::String(source_type.to_owned()));
        }
        imports.insert("has_imported".to_owned(), Value::Bool(true));
        let count = imports
            .get("import_count")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        imports.insert("import_count".to_owned(), Value::from(count + 1));
        if let Some(source_display) = source_display {
            let summary = if entries_written != 0 {
                format!("{entries_written} {source_display}")
            } else {
                source_display.to_owned()
            };
            imports.insert(
                "last_completed".to_owned(),
                Value::String(now_iso.to_owned()),
            );
            imports.insert("last_result_summary".to_owned(), Value::String(summary));
        }
    })?;
    let mut data = Map::new();
    data.insert(
        "source_type".to_owned(),
        Value::String(source_type.to_owned()),
    );
    append_log_locked(
        journal_root,
        "state",
        Some("imports.completed"),
        None,
        Some(&data),
        day,
        timestamp_ms,
        &Map::new(),
    )?;
    Ok(imports)
}

/// Record that the import offer was declined and append its awareness event.
pub fn record_import_offer_declined(
    journal_root: &Path,
    now_iso: &str,
    day: &str,
    timestamp_ms: i64,
) -> Result<Value, AwarenessStoreError> {
    let _trust = hold_facet_trust_lock(journal_root).map_err(AwarenessStoreError::TrustLock)?;
    let imports = update_imports(journal_root, |imports| {
        imports.insert(
            "offer_declined".to_owned(),
            Value::String(now_iso.to_owned()),
        );
    })?;
    append_log_locked(
        journal_root,
        "state",
        Some("imports.offer_declined"),
        None,
        None,
        day,
        timestamp_ms,
        &Map::new(),
    )?;
    Ok(imports)
}

/// Record that an import nudge was shown and append its awareness event.
pub fn record_import_nudge(
    journal_root: &Path,
    now_iso: &str,
    day: &str,
    timestamp_ms: i64,
) -> Result<Value, AwarenessStoreError> {
    let _trust = hold_facet_trust_lock(journal_root).map_err(AwarenessStoreError::TrustLock)?;
    let imports = update_imports(journal_root, |imports| {
        imports.insert("last_nudge".to_owned(), Value::String(now_iso.to_owned()));
    })?;
    append_log_locked(
        journal_root,
        "state",
        Some("imports.nudge_sent"),
        None,
        None,
        day,
        timestamp_ms,
        &Map::new(),
    )?;
    Ok(imports)
}

fn update_imports(
    journal_root: &Path,
    update: impl FnOnce(&mut Map<String, Value>),
) -> Result<Value, AwarenessStoreError> {
    let path = current_path(journal_root);
    create_parent(&path)?;
    let _lock = hold_lock(&path, LockOptions::default()).map_err(AwarenessStoreError::Lock)?;
    let mut state = load_current(journal_root)?;
    let state = state
        .as_object_mut()
        .expect("awareness current reader returns an object default");
    let imports = state
        .entry("imports".to_owned())
        .or_insert_with(default_imports);
    if !imports.is_object() {
        *imports = default_imports();
    }
    let imports = imports
        .as_object_mut()
        .expect("imports was normalized to an object");
    update(imports);
    let updated = Value::Object(imports.clone());
    write_json(&path, &Value::Object(state.clone()), json_options())
        .map_err(AwarenessStoreError::Write)?;
    Ok(updated)
}

#[allow(clippy::too_many_arguments)]
fn append_log_locked(
    journal_root: &Path,
    kind: &str,
    key: Option<&str>,
    message: Option<&str>,
    data: Option<&Map<String, Value>>,
    day: &str,
    timestamp_ms: i64,
    extra: &Map<String, Value>,
) -> Result<Value, AwarenessStoreError> {
    let path = log_path(journal_root, day);
    create_parent(&path)?;
    let _lock = hold_lock(&path, LockOptions::default()).map_err(AwarenessStoreError::Lock)?;
    let mut entry = Map::new();
    entry.insert("ts".to_owned(), Value::from(timestamp_ms));
    entry.insert("kind".to_owned(), Value::String(kind.to_owned()));
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        entry.insert("key".to_owned(), Value::String(key.to_owned()));
    }
    if let Some(message) = message.filter(|message| !message.is_empty()) {
        entry.insert("message".to_owned(), Value::String(message.to_owned()));
    }
    if let Some(data) = data.filter(|data| !data.is_empty()) {
        entry.insert("data".to_owned(), Value::Object((*data).clone()));
    }
    entry.extend(extra.clone());
    let entry = Value::Object(entry);
    append_jsonl(&path, &entry).map_err(AwarenessStoreError::Append)?;
    Ok(entry)
}

fn current_path(journal_root: &Path) -> PathBuf {
    journal_root.join("awareness/current.json")
}

fn log_path(journal_root: &Path, day: &str) -> PathBuf {
    journal_root.join("awareness").join(format!("{day}.jsonl"))
}

fn default_imports() -> Value {
    json!({
        "has_imported": false,
        "import_count": 0,
        "sources_used": [],
        "offer_declined": null,
        "last_nudge": null,
    })
}

fn create_parent(path: &Path) -> Result<(), AwarenessStoreError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent).map_err(|source| AwarenessStoreError::Directory {
        path: parent.to_owned(),
        source,
    })
}

fn json_options() -> JsonWriteOptions {
    JsonWriteOptions {
        mode: Some(0o600),
        indent: Some(2),
        sort_keys: false,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;

    use super::*;

    fn temporary_journal(name: &str) -> PathBuf {
        let path = PathBuf::from("/var/tmp").join(format!(
            "solstone-awareness-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn record_import_increments_every_time_but_fills_sources_once() {
        let journal = temporary_journal("record");

        let first = record_import(
            &journal,
            "chatgpt",
            Some("ChatGPT export"),
            3,
            "20260515T12:00:00",
            "20260515",
            1_778_846_400_000,
        )
        .unwrap();
        let second = record_import(
            &journal,
            "chatgpt",
            None,
            0,
            "20260515T12:00:01",
            "20260515",
            1_778_846_401_000,
        )
        .unwrap();

        assert_eq!(first["last_completed"], "20260515T12:00:00");
        assert_eq!(first["last_result_summary"], "3 ChatGPT export");
        assert_eq!(second["import_count"], 2);
        assert_eq!(second["sources_used"], json!(["chatgpt"]));
        let log = read_log(&journal, "20260515").unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0]["key"], "imports.completed");
        assert_eq!(log[0]["data"]["source_type"], "chatgpt");
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn decline_and_nudge_update_their_distinct_import_fields() {
        let journal = temporary_journal("actions");

        let declined = record_import_offer_declined(
            &journal,
            "20260515T12:00:00",
            "20260515",
            1_778_846_400_000,
        )
        .unwrap();
        let nudged =
            record_import_nudge(&journal, "20260515T12:00:01", "20260515", 1_778_846_401_000)
                .unwrap();

        assert_eq!(declined["offer_declined"], "20260515T12:00:00");
        assert_eq!(nudged["offer_declined"], "20260515T12:00:00");
        assert_eq!(nudged["last_nudge"], "20260515T12:00:01");
        let log = read_log(&journal, "20260515").unwrap();
        assert_eq!(log[0]["key"], "imports.offer_declined");
        assert_eq!(log[1]["key"], "imports.nudge_sent");
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn append_log_returns_and_persists_entry() {
        let journal = temporary_journal("append");
        let mut data = Map::new();
        data.insert("answer".to_owned(), Value::from(42));
        let mut extra = Map::new();
        extra.insert("origin".to_owned(), Value::String("test".to_owned()));

        let entry = append_log(
            &journal,
            "observation",
            Some("test.event"),
            Some("recorded"),
            Some(&data),
            "20260515",
            1_778_846_400_000,
            &extra,
        )
        .unwrap();

        assert_eq!(entry["origin"], "test");
        assert_eq!(read_log(&journal, "20260515").unwrap(), vec![entry]);
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn reads_do_not_create_the_awareness_directory() {
        let journal = temporary_journal("read-only");

        assert_eq!(load_current(&journal).unwrap(), json!({}));
        assert_eq!(load_imports(&journal).unwrap(), default_imports());
        assert!(read_log(&journal, "20260515").unwrap().is_empty());
        assert!(!journal.join("awareness").exists());
        fs::remove_dir_all(journal).unwrap();
    }
}
