// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable side-store managing MCP agent pause state, attempt counters, and memory source mappings.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_format::agent_memory::SourceKey;
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, PathError,
    create_directory_with_mode, hold_lock, write_json,
};

use crate::tokens::RandomSource;

const ENDPOINT_DIRECTORY: &str = "mcp-endpoint";
const AGENT_STATE_FILE: &str = "agent-state.json";
const SCHEMA_VERSION: u64 = 1;

#[derive(Debug)]
pub(crate) enum AgentStateError {
    Unreadable,
    Lock(LockError),
    Write(AtomicWriteError),
    Directory(PathError),
}

fn format_message(formatter: &mut fmt::Formatter<'_>, message: fmt::Arguments<'_>) -> fmt::Result {
    fmt::Display::fmt(&message, formatter)
}

impl std::fmt::Display for AgentStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable => f.write_str("MCP agent state store is unreadable"),
            Self::Lock(err) => format_message(
                f,
                format_args!("could not lock MCP agent state store: {err}"),
            ),
            Self::Write(err) => format_message(
                f,
                format_args!("could not write MCP agent state store: {err}"),
            ),
            Self::Directory(err) => format_message(
                f,
                format_args!("could not prepare MCP agent state directory: {err}"),
            ),
        }
    }
}

impl std::error::Error for AgentStateError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerPauseRecord {
    pub(crate) connection: String,
    pub(crate) pause_id: String,
    pub(crate) since: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttemptsRecord {
    pub(crate) connection: String,
    pub(crate) stamp: String,
    pub(crate) count: u64,
    pub(crate) last_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemorySourceRecord {
    pub(crate) connection: String,
    pub(crate) source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentStateEntry {
    OwnerPause(OwnerPauseRecord),
    Attempts(AttemptsRecord),
    MemorySource(MemorySourceRecord),
    /// An entry this binary doesn't know, kept as it was so a write here
    /// never erases what a newer binary stored.
    Unknown(Value),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AgentStateStore {
    pub(crate) entries: Vec<AgentStateEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AttemptsInfo {
    pub(crate) count: u64,
    pub(crate) last_at: Option<String>,
}

pub(crate) fn default_lock_options() -> LockOptions {
    LockOptions {
        mode: Some(0o600),
        ..LockOptions::default()
    }
}

pub(crate) fn agent_state_path(journal_root: &Path) -> PathBuf {
    journal_root.join(ENDPOINT_DIRECTORY).join(AGENT_STATE_FILE)
}

fn ensure_directory(journal_root: &Path) -> Result<(), AgentStateError> {
    create_directory_with_mode(&journal_root.join(ENDPOINT_DIRECTORY), 0o700)
        .map_err(AgentStateError::Directory)
}

fn is_64_lower_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn parse_store(bytes: &[u8]) -> Result<AgentStateStore, AgentStateError> {
    if bytes.is_empty() {
        return Err(AgentStateError::Unreadable);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| AgentStateError::Unreadable)?;
    let obj = value.as_object().ok_or(AgentStateError::Unreadable)?;
    let schema = obj
        .get("schema")
        .and_then(Value::as_u64)
        .ok_or(AgentStateError::Unreadable)?;
    if schema != SCHEMA_VERSION {
        return Err(AgentStateError::Unreadable);
    }
    let entries_array = obj
        .get("entries")
        .and_then(Value::as_array)
        .ok_or(AgentStateError::Unreadable)?;

    let mut entries = Vec::new();
    for item in entries_array {
        let item_obj = match item.as_object() {
            Some(o) => o,
            None => {
                entries.push(AgentStateEntry::Unknown(item.clone()));
                continue;
            }
        };
        let kind = match item_obj.get("kind").and_then(Value::as_str) {
            Some(k) => k,
            None => {
                entries.push(AgentStateEntry::Unknown(item.clone()));
                continue;
            }
        };
        match kind {
            "owner_pause" => {
                let connection = item_obj
                    .get("connection")
                    .and_then(Value::as_str)
                    .ok_or(AgentStateError::Unreadable)?;
                let pause_id = item_obj
                    .get("pause_id")
                    .and_then(Value::as_str)
                    .ok_or(AgentStateError::Unreadable)?;
                let since = item_obj
                    .get("since")
                    .and_then(Value::as_str)
                    .ok_or(AgentStateError::Unreadable)?;
                entries.push(AgentStateEntry::OwnerPause(OwnerPauseRecord {
                    connection: connection.to_string(),
                    pause_id: pause_id.to_string(),
                    since: since.to_string(),
                }));
            }
            "attempts" => {
                let connection = item_obj
                    .get("connection")
                    .and_then(Value::as_str)
                    .ok_or(AgentStateError::Unreadable)?;
                let stamp = item_obj
                    .get("stamp")
                    .and_then(Value::as_str)
                    .ok_or(AgentStateError::Unreadable)?;
                let count = item_obj
                    .get("count")
                    .and_then(Value::as_u64)
                    .ok_or(AgentStateError::Unreadable)?;
                let last_at = match item_obj.get("last_at") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Null) | None => None,
                    _ => return Err(AgentStateError::Unreadable),
                };
                entries.push(AgentStateEntry::Attempts(AttemptsRecord {
                    connection: connection.to_string(),
                    stamp: stamp.to_string(),
                    count,
                    last_at,
                }));
            }
            "memory_source" => {
                let connection = item_obj
                    .get("connection")
                    .and_then(Value::as_str)
                    .ok_or(AgentStateError::Unreadable)?;
                let source = item_obj
                    .get("source")
                    .and_then(Value::as_str)
                    .ok_or(AgentStateError::Unreadable)?;
                if !is_64_lower_hex(source) {
                    return Err(AgentStateError::Unreadable);
                }
                entries.push(AgentStateEntry::MemorySource(MemorySourceRecord {
                    connection: connection.to_string(),
                    source: source.to_string(),
                }));
            }
            _ => entries.push(AgentStateEntry::Unknown(item.clone())),
        }
    }
    Ok(AgentStateStore { entries })
}

fn serialize_store(store: &AgentStateStore) -> Value {
    let mut entries_json = Vec::with_capacity(store.entries.len());
    for entry in &store.entries {
        match entry {
            AgentStateEntry::OwnerPause(r) => {
                entries_json.push(json!({
                    "kind": "owner_pause",
                    "connection": r.connection,
                    "pause_id": r.pause_id,
                    "since": r.since,
                }));
            }
            AgentStateEntry::Attempts(r) => {
                entries_json.push(json!({
                    "kind": "attempts",
                    "connection": r.connection,
                    "stamp": r.stamp,
                    "count": r.count,
                    "last_at": r.last_at,
                }));
            }
            AgentStateEntry::MemorySource(r) => {
                entries_json.push(json!({
                    "kind": "memory_source",
                    "connection": r.connection,
                    "source": r.source,
                }));
            }
            AgentStateEntry::Unknown(value) => entries_json.push(value.clone()),
        }
    }
    json!({
        "schema": SCHEMA_VERSION,
        "entries": entries_json,
    })
}

fn read_store_from_path(path: &Path) -> Result<AgentStateStore, AgentStateError> {
    match fs::read(path) {
        Ok(bytes) => parse_store(&bytes),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(AgentStateStore::default()),
        Err(_) => Err(AgentStateError::Unreadable),
    }
}

fn write_store_to_path(path: &Path, store: &AgentStateStore) -> Result<(), AgentStateError> {
    let doc = serialize_store(store);
    write_json(
        path,
        &doc,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            ..JsonWriteOptions::default()
        },
    )
    .map_err(AgentStateError::Write)
}

/// The only production caller of `SourceKey::from_verified_id` in this crate.
pub(crate) fn memory_source(
    journal_root: &Path,
    verified_id: &str,
) -> Result<SourceKey, AgentStateError> {
    let store = read_store(journal_root)?;
    for entry in &store.entries {
        if let AgentStateEntry::MemorySource(ms) = entry
            && ms.connection == verified_id
        {
            let key = SourceKey::parse(format!("sha256:{}", ms.source))
                .map_err(|_| AgentStateError::Unreadable)?;
            return Ok(key);
        }
    }
    Ok(SourceKey::from_verified_id(verified_id))
}

pub(crate) fn read_store(journal_root: &Path) -> Result<AgentStateStore, AgentStateError> {
    let path = agent_state_path(journal_root);
    read_store_from_path(&path)
}

pub(crate) fn is_owner_paused(
    journal_root: &Path,
    connection: &str,
) -> Result<Option<OwnerPauseRecord>, AgentStateError> {
    let store = read_store(journal_root)?;
    for entry in store.entries {
        if let AgentStateEntry::OwnerPause(record) = entry
            && record.connection == connection
        {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

pub(crate) fn is_owner_paused_locked(
    journal_root: &Path,
    connection: &str,
) -> Result<bool, AgentStateError> {
    let path = agent_state_path(journal_root);
    if !path.exists() {
        return Ok(false);
    }
    let _lock = hold_lock(&path, default_lock_options()).map_err(AgentStateError::Lock)?;
    if !path.exists() {
        return Ok(false);
    }
    let store = read_store_from_path(&path)?;
    for entry in store.entries {
        if let AgentStateEntry::OwnerPause(record) = entry
            && record.connection == connection
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn random_hex_16(random: &dyn RandomSource) -> Result<String, AgentStateError> {
    let mut bytes = [0u8; 16];
    let mut offset = 0;
    while offset < bytes.len() {
        let n = random
            .fill(&mut bytes[offset..])
            .map_err(|_| AgentStateError::Unreadable)?;
        if n == 0 {
            return Err(AgentStateError::Unreadable);
        }
        offset += n;
    }
    let mut hex = String::with_capacity(32);
    for b in bytes {
        hex.push_str(&format!("{:02x}", b));
    }
    Ok(hex)
}

pub(crate) fn set_owner_pause(
    journal_root: &Path,
    connection: &str,
    now: DateTime<Utc>,
    random: &dyn RandomSource,
) -> Result<String, AgentStateError> {
    set_owner_pause_with_options(
        journal_root,
        connection,
        now,
        random,
        default_lock_options(),
    )
}

pub(crate) fn set_owner_pause_with_options(
    journal_root: &Path,
    connection: &str,
    now: DateTime<Utc>,
    random: &dyn RandomSource,
    lock_options: LockOptions,
) -> Result<String, AgentStateError> {
    let path = agent_state_path(journal_root);
    ensure_directory(journal_root)?;
    let _lock = hold_lock(&path, lock_options).map_err(AgentStateError::Lock)?;
    let mut store = read_store_from_path(&path)?;

    for entry in &store.entries {
        if let AgentStateEntry::OwnerPause(op) = entry
            && op.connection == connection
        {
            return Ok(op.pause_id.clone());
        }
    }

    let pause_id = random_hex_16(random)?;
    let since = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    store
        .entries
        .push(AgentStateEntry::OwnerPause(OwnerPauseRecord {
            connection: connection.to_string(),
            pause_id: pause_id.clone(),
            since,
        }));
    write_store_to_path(&path, &store)?;
    Ok(pause_id)
}

pub(crate) fn clear_owner_pause(
    journal_root: &Path,
    connection: &str,
) -> Result<bool, AgentStateError> {
    clear_owner_pause_with_options(journal_root, connection, default_lock_options())
}

pub(crate) fn clear_owner_pause_with_options(
    journal_root: &Path,
    connection: &str,
    lock_options: LockOptions,
) -> Result<bool, AgentStateError> {
    let path = agent_state_path(journal_root);
    ensure_directory(journal_root)?;
    let _lock = hold_lock(&path, lock_options).map_err(AgentStateError::Lock)?;
    let mut store = read_store_from_path(&path)?;
    let initial_len = store.entries.len();
    store.entries.retain(|entry| match entry {
        AgentStateEntry::OwnerPause(op) => op.connection != connection,
        _ => true,
    });
    let removed = store.entries.len() != initial_len;
    if removed {
        write_store_to_path(&path, &store)?;
    }
    Ok(removed)
}

pub(crate) fn bump_attempts(
    journal_root: &Path,
    connection: &str,
    stamp: &str,
    now: DateTime<Utc>,
) -> Result<(), AgentStateError> {
    bump_attempts_with_options(
        journal_root,
        connection,
        stamp,
        now,
        default_lock_options(),
        &|| (),
    )
}

pub(crate) fn bump_attempts_with_options(
    journal_root: &Path,
    connection: &str,
    stamp: &str,
    now: DateTime<Utc>,
    lock_options: LockOptions,
    on_hold: &dyn Fn(),
) -> Result<(), AgentStateError> {
    let path = agent_state_path(journal_root);
    ensure_directory(journal_root)?;
    let _lock = hold_lock(&path, lock_options).map_err(AgentStateError::Lock)?;
    let mut store = read_store_from_path(&path)?;
    on_hold();

    let now_str = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut found = false;
    for entry in &mut store.entries {
        if let AgentStateEntry::Attempts(attempts) = entry
            && attempts.connection == connection
        {
            found = true;
            if attempts.stamp == stamp {
                attempts.count = attempts.count.saturating_add(1);
                attempts.last_at = Some(now_str.clone());
            } else {
                attempts.stamp = stamp.to_string();
                attempts.count = 1;
                attempts.last_at = Some(now_str.clone());
            }
            break;
        }
    }
    if !found {
        store
            .entries
            .push(AgentStateEntry::Attempts(AttemptsRecord {
                connection: connection.to_string(),
                stamp: stamp.to_string(),
                count: 1,
                last_at: Some(now_str),
            }));
    }
    write_store_to_path(&path, &store)
}

pub(crate) fn get_attempts_for_stamp(
    journal_root: &Path,
    connection: &str,
    active_stamp: Option<&str>,
) -> Result<AttemptsInfo, AgentStateError> {
    let active_stamp = match active_stamp {
        Some(s) => s,
        None => return Ok(AttemptsInfo::default()),
    };
    let store = read_store(journal_root)?;
    for entry in store.entries {
        if let AgentStateEntry::Attempts(record) = entry
            && record.connection == connection
        {
            if record.stamp == active_stamp {
                return Ok(AttemptsInfo {
                    count: record.count,
                    last_at: record.last_at,
                });
            } else {
                return Ok(AttemptsInfo::default());
            }
        }
    }
    Ok(AttemptsInfo::default())
}

pub(crate) fn set_memory_source(
    journal_root: &Path,
    connection: &str,
    source_hex: &str,
) -> Result<(), AgentStateError> {
    set_memory_source_with_options(journal_root, connection, source_hex, default_lock_options())
}

#[allow(dead_code)]
pub(crate) fn set_memory_source_with_options(
    journal_root: &Path,
    connection: &str,
    source_hex: &str,
    lock_options: LockOptions,
) -> Result<(), AgentStateError> {
    let path = agent_state_path(journal_root);
    ensure_directory(journal_root)?;
    let _lock = hold_lock(&path, lock_options).map_err(AgentStateError::Lock)?;
    let mut store = read_store_from_path(&path)?;

    let mut found = false;
    for entry in &mut store.entries {
        if let AgentStateEntry::MemorySource(ms) = entry
            && ms.connection == connection
        {
            ms.source = source_hex.to_string();
            found = true;
            break;
        }
    }
    if !found {
        store
            .entries
            .push(AgentStateEntry::MemorySource(MemorySourceRecord {
                connection: connection.to_string(),
                source: source_hex.to_string(),
            }));
    }
    write_store_to_path(&path, &store)
}

#[allow(dead_code)]
pub(crate) fn get_memory_source(
    journal_root: &Path,
    connection: &str,
) -> Result<Option<String>, AgentStateError> {
    let store = read_store(journal_root)?;
    for entry in store.entries {
        if let AgentStateEntry::MemorySource(record) = entry
            && record.connection == connection
        {
            return Ok(Some(record.source));
        }
    }
    Ok(None)
}

#[allow(dead_code)]
pub(crate) fn find_connection_by_memory_source(
    journal_root: &Path,
    source_hex: &str,
) -> Result<Option<String>, AgentStateError> {
    let store = read_store(journal_root)?;
    for entry in store.entries {
        if let AgentStateEntry::MemorySource(record) = entry
            && record.source == source_hex
        {
            return Ok(Some(record.connection));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    #[test]
    fn missing_file_behavior() {
        let dir = TempDir::new_in(crate::test_scratch()).unwrap();
        let paused = is_owner_paused(dir.path(), "test:1").unwrap();
        assert_eq!(paused, None);
        let src = memory_source(dir.path(), "test:1").unwrap();
        assert_eq!(src, SourceKey::from_verified_id("test:1"));
        assert!(!agent_state_path(dir.path()).exists());
    }

    #[test]
    fn hand_written_store_document_with_unknown_field_and_future_kind() {
        let dir = TempDir::new_in(crate::test_scratch()).unwrap();
        ensure_directory(dir.path()).unwrap();
        let path = agent_state_path(dir.path());
        let doc = json!({
            "schema": 1,
            "entries": [
                {
                    "kind": "owner_pause",
                    "connection": "test:conn",
                    "pause_id": "0123456789abcdef0123456789abcdef",
                    "since": "2026-11-01T00:00:00Z",
                    "extra_field": "preserved"
                },
                {
                    "kind": "future_kind",
                    "foo": "bar"
                }
            ]
        });
        fs::write(&path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

        let paused = is_owner_paused(dir.path(), "test:conn")
            .unwrap()
            .expect("pause exists");
        assert_eq!(paused.pause_id, "0123456789abcdef0123456789abcdef");
        assert_eq!(paused.since, "2026-11-01T00:00:00Z");

        let random = crate::tokens::SystemRandomSource;
        let now = Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap();
        let second_pause_id = set_owner_pause(dir.path(), "test:conn", now, &random).unwrap();
        assert_eq!(second_pause_id, "0123456789abcdef0123456789abcdef");

        let paused_after = is_owner_paused(dir.path(), "test:conn").unwrap().unwrap();
        assert_eq!(paused_after.pause_id, "0123456789abcdef0123456789abcdef");
        assert_eq!(paused_after.since, "2026-11-01T00:00:00Z");

        // A write by this binary keeps the entry it doesn't know.
        set_memory_source(dir.path(), "test:other", &"ab".repeat(32)).unwrap();
        let written: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(
            written["entries"]
                .as_array()
                .unwrap()
                .contains(&json!({"kind": "future_kind", "foo": "bar"}))
        );
    }

    #[test]
    fn malformed_cases_are_unreadable() {
        let cases: Vec<Vec<u8>> = vec![
            b"not valid json".to_vec(),
            serde_json::to_vec(&json!({
                "schema": 2,
                "entries": []
            }))
            .unwrap(),
            serde_json::to_vec(&json!({
                "schema": 1,
                "entries": [
                    {
                        "kind": "owner_pause",
                        "connection": "test:conn",
                        "since": "2026-11-01T00:00:00Z"
                    }
                ]
            }))
            .unwrap(),
            serde_json::to_vec(&json!({
                "schema": 1,
                "entries": [
                    {
                        "kind": "memory_source",
                        "connection": "test:conn",
                        "source": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789ABCDEF"
                    }
                ]
            }))
            .unwrap(),
        ];

        for (index, payload) in cases.into_iter().enumerate() {
            let dir = TempDir::new_in(crate::test_scratch()).unwrap();
            ensure_directory(dir.path()).unwrap();
            let path = agent_state_path(dir.path());
            fs::write(&path, payload).unwrap();

            assert!(
                matches!(read_store(dir.path()), Err(AgentStateError::Unreadable)),
                "case {index} must be Unreadable from read_store"
            );
            assert!(
                matches!(
                    memory_source(dir.path(), "test:conn"),
                    Err(AgentStateError::Unreadable)
                ),
                "case {index} must be Err from memory_source"
            );
        }
    }

    #[test]
    fn same_datetime_pause_re_pause_different_id_and_attempts_preserved() {
        let dir = TempDir::new_in(crate::test_scratch()).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap();
        let random = crate::tokens::SystemRandomSource;
        let conn = "test:repause";

        let pause_id1 = set_owner_pause(dir.path(), conn, now, &random).unwrap();
        bump_attempts(dir.path(), conn, &pause_id1, now).unwrap();
        bump_attempts(dir.path(), conn, &pause_id1, now).unwrap();
        bump_attempts(dir.path(), conn, &pause_id1, now).unwrap();

        clear_owner_pause(dir.path(), conn).unwrap();

        let pause_id2 = set_owner_pause(dir.path(), conn, now, &random).unwrap();
        assert_ne!(pause_id1, pause_id2);

        let attempts_new = get_attempts_for_stamp(dir.path(), conn, Some(&pause_id2)).unwrap();
        assert_eq!(attempts_new.count, 0);
        assert_eq!(attempts_new.last_at, None);

        let raw = fs::read_to_string(agent_state_path(dir.path())).unwrap();
        let val: Value = serde_json::from_str(&raw).unwrap();
        let entries = val["entries"].as_array().expect("entries array");
        let has_attempts = entries
            .iter()
            .any(|e| e["kind"] == "attempts" && e["stamp"] == pause_id1 && e["count"] == 3);
        assert!(has_attempts, "attempts row must still be in the file");
    }

    #[test]
    fn idle_pause_bump_hold_does_not_lose_or_double_count() {
        let now = Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap();

        // 1. Uncontended probe: one bump with timeout zero on a fresh journal must succeed.
        let probe_dir = TempDir::new_in(crate::test_scratch()).unwrap();
        let zero_options = LockOptions {
            timeout: std::time::Duration::ZERO,
            mode: Some(0o600),
            ..Default::default()
        };
        let probe_res = bump_attempts_with_options(
            probe_dir.path(),
            "probe:conn",
            "probe:stamp",
            now,
            zero_options,
            &|| (),
        );
        let b_timeout = if probe_res.is_ok() {
            std::time::Duration::ZERO
        } else {
            std::time::Duration::from_millis(20)
        };

        // 2. Thread hold test.
        let dir = TempDir::new_in(crate::test_scratch()).unwrap();
        let conn = "test:contention";
        let stamp = "stamp:test";

        let (hold_tx, hold_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let root_a = dir.path().to_path_buf();
        let conn_a = conn.to_string();
        let stamp_a = stamp.to_string();
        let handle_a = std::thread::spawn(move || {
            bump_attempts_with_options(
                &root_a,
                &conn_a,
                &stamp_a,
                now,
                default_lock_options(),
                &|| {
                    hold_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                },
            )
        });

        // Wait until Thread A enters on_hold.
        hold_rx.recv().unwrap();

        // Thread B attempts to bump with zero (or 20ms) timeout while Thread A holds the lock.
        let start = std::time::Instant::now();
        let b_res = bump_attempts_with_options(
            dir.path(),
            conn,
            stamp,
            now,
            LockOptions {
                timeout: b_timeout,
                mode: Some(0o600),
                ..Default::default()
            },
            &|| (),
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert!(
            matches!(b_res, Err(AgentStateError::Lock(_))),
            "Thread B must fail with Lock error while A holds"
        );

        // Release Thread A.
        release_tx.send(()).unwrap();
        handle_a.join().unwrap().unwrap();

        // A's bump is the only write (count 1).
        let attempts_1 = get_attempts_for_stamp(dir.path(), conn, Some(stamp)).unwrap();
        assert_eq!(attempts_1.count, 1);

        // Five more bumps with the default options make count 6.
        for _ in 0..5 {
            bump_attempts(dir.path(), conn, stamp, now).unwrap();
        }
        let attempts_6 = get_attempts_for_stamp(dir.path(), conn, Some(stamp)).unwrap();
        assert_eq!(attempts_6.count, 6);
    }
}
