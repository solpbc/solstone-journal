// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable plain-file storage under `config/email/`, credentials under
//! `config/journal.json` (`"email"` section, redacted from public settings),
//! and regenerable cache / transient per-batch body staging under `cache/email/`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_journal_io::atomic::{AtomicWriteOptions, atomic_replace};
use solstone_core_journal_io::locking::{FileLock, LockOptions, hold_lock, lock_is_held};

use crate::choice::Choice;
use crate::error::{EmailError, Result};
use crate::model::RangePreset;
use crate::select::Decision;

pub const EMAIL_STREAM: &str = "import.email";
pub const PROVIDER_GMAIL: &str = "gmail";

const CONFIG_DIR_REL: &str = "config/email";
const ACCOUNT_REL: &str = "config/email/account.json";
const CHOICE_REL: &str = "config/email/choice.json";
const STATE_REL: &str = "config/email/state.json";
const DECISIONS_REL: &str = "config/email/decisions.jsonl";
const LANDED_REL: &str = "config/email/landed.jsonl";
const RUNS_REL: &str = "config/email/runs.json";
const LOCK_REL: &str = "config/email/sync.lock";

const CACHE_DIR_REL: &str = "cache/email";
const HEADER_CACHE_REL: &str = "cache/email/headers.sqlite";
const BODIES_DIR_REL: &str = "cache/email/bodies";
const CANCEL_REL: &str = "cache/email/cancel";

/// Credentials saved in `config/journal.json` under `"email"`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    pub provider: String,
    pub account: String,
    pub app_password: String,
}

/// Durable non-secret metadata for the connected (or previously connected) account.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountMeta {
    pub provider: String,
    pub account: String,
    pub mailbox: String,
    pub owner_aliases: Vec<String>,
    pub connected_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disconnected_at: Option<String>,
}

/// Durable scan/sync progress and attention state (`config/email/state.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SourceState {
    /// `not_connected`, `scanning`, `scan_paused`, `awaiting_choice`,
    /// `fetching`, `steady`, `needs_attention`, or `disconnected`.
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention_reason: Option<String>,
    pub scan_complete: bool,
    pub scan_paused: bool,
    pub scan_checked: u64,
    pub scan_total: u64,
    pub scan_sent_found: usize,
    pub scan_eligible_sent: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_earliest_month: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_cap_warning: Option<u64>,
    pub fetching_active: bool,
    pub fetch_done: usize,
    pub fetch_total: usize,
    pub file_history_done: usize,
    pub file_history_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_added_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub last_rules_version: u32,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// One landed exchange record in `config/email/landed.jsonl`.
///
/// Serves both as the duplicate guard and as the permanent skip-tombstone when
/// the owner deletes a message in `transcripts` or via per-run Trash (AC 17).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LandedRecord {
    pub key: String,
    pub run_id: String,
    pub day: String,
    pub segment: String,
    /// Relative to `chronicle/`, e.g. `20261118/import.email/101600_300`.
    pub segment_rel: String,
    pub landed_at: String,
    pub sha256: String,
}

/// One sync run summary in `config/email/runs.json` for `past syncs` per-run Trash.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncRunRecord {
    pub id: String,
    pub synced_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_label: Option<RangePreset>,
    pub messages: usize,
    pub segments: Vec<String>,
}

fn storage<E: std::fmt::Display>(err: E) -> EmailError {
    EmailError::Storage(err.to_string())
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

pub fn config_dir(root: &Path) -> PathBuf {
    root.join(CONFIG_DIR_REL)
}

pub fn cache_dir(root: &Path) -> PathBuf {
    root.join(CACHE_DIR_REL)
}

pub fn header_cache_path(root: &Path) -> PathBuf {
    root.join(HEADER_CACHE_REL)
}

pub fn bodies_dir(root: &Path) -> PathBuf {
    root.join(BODIES_DIR_REL)
}

pub fn cancel_flag_path(root: &Path) -> PathBuf {
    root.join(CANCEL_REL)
}

#[must_use]
pub fn decisions_path(root: &Path) -> PathBuf {
    root.join(DECISIONS_REL)
}

/// Request cancellation of an in-progress scan or sync.
pub fn request_cancel(root: &Path) -> Result<()> {
    ensure_private_dir(&cache_dir(root))?;
    atomic_replace(
        cancel_flag_path(root),
        b"1\n",
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(storage)
}

pub fn clear_cancel(root: &Path) {
    let _ = fs::remove_file(cancel_flag_path(root));
}

#[must_use]
pub fn is_cancel_requested(root: &Path) -> bool {
    cancel_flag_path(root).is_file()
        || solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(root)
}

/// Acquire the exclusive email source sync/mutation lock (`config/email/sync.lock`).
pub fn hold_sync_lock(root: &Path) -> Result<FileLock> {
    ensure_private_dir(&config_dir(root))?;
    hold_lock(
        root.join(LOCK_REL),
        LockOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )
    .map_err(|_| EmailError::Busy)
}

#[must_use]
pub fn is_sync_locked(root: &Path) -> bool {
    lock_is_held(root.join(LOCK_REL)).unwrap_or(false)
}

/// Normalize a Gmail app password (strip spaces/hyphens) and validate basic shape.
pub fn normalize_app_password(raw: &str) -> Result<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if cleaned.len() < 8 || cleaned.len() > 128 || !cleaned.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(EmailError::InvalidInput(
            "app password must be a valid ASCII password".into(),
        ));
    }
    Ok(cleaned)
}

/// Validate a basic email address string.
pub fn normalize_email_address(raw: &str) -> Result<String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    let Some((local, domain)) = trimmed.split_once('@') else {
        return Err(EmailError::InvalidInput(
            "enter a valid email address".into(),
        ));
    };
    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || trimmed.chars().any(char::is_whitespace)
    {
        return Err(EmailError::InvalidInput(
            "enter a valid email address".into(),
        ));
    }
    Ok(trimmed)
}

/// Read saved email credentials from `config/journal.json`.
pub fn read_credentials(root: &Path) -> Result<Option<Credentials>> {
    let loaded = solstone_core_journal_config::read_journal_config(root).map_err(storage)?;
    let Some(cfg) = loaded.config else {
        return Ok(None);
    };
    let Some(sec) = cfg.get("email").and_then(Value::as_object) else {
        return Ok(None);
    };
    let Some(account) = sec.get("account").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(app_password) = sec.get("app_password").and_then(Value::as_str) else {
        return Ok(None);
    };
    if account.trim().is_empty() || app_password.is_empty() {
        return Ok(None);
    }
    let provider = sec
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(PROVIDER_GMAIL)
        .to_owned();
    Ok(Some(Credentials {
        provider,
        account: account.trim().to_owned(),
        app_password: app_password.to_owned(),
    }))
}

/// Save or clear email credentials in `config/journal.json` under `"email"`.
pub fn write_credentials(root: &Path, creds: Option<&Credentials>) -> Result<()> {
    solstone_core_journal_config_write::mutate_journal_config(
        root,
        LockOptions::default(),
        |config| {
            match creds {
                Some(c) => {
                    config.insert(
                        "email".to_owned(),
                        json!({
                            "provider": c.provider,
                            "account": c.account,
                            "app_password": c.app_password,
                        }),
                    );
                }
                None => {
                    config.remove("email");
                }
            }
            solstone_core_journal_config_write::JournalConfigMutation {
                changed: true,
                value: (),
            }
        },
    )
    .map_err(storage)?;
    Ok(())
}

fn write_private_json<T: Serialize>(root: &Path, rel: &str, value: &T) -> Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(value).map_err(storage)?;
    bytes.push(b'\n');
    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) }).map_err(storage)
}

fn read_json_optional<T: for<'de> Deserialize<'de>>(root: &Path, rel: &str) -> Result<Option<T>> {
    let path = root.join(rel);
    match fs::read(&path) {
        Ok(bytes) => {
            let value = serde_json::from_slice(&bytes).map_err(storage)?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(storage(e)),
    }
}

pub fn read_account(root: &Path) -> Result<Option<AccountMeta>> {
    read_json_optional(root, ACCOUNT_REL)
}

pub fn write_account(root: &Path, meta: &AccountMeta) -> Result<()> {
    write_private_json(root, ACCOUNT_REL, meta)
}

pub fn read_choice(root: &Path) -> Result<Option<Choice>> {
    read_json_optional(root, CHOICE_REL)
}

pub fn write_choice(root: &Path, choice: &Choice) -> Result<()> {
    write_private_json(root, CHOICE_REL, choice)
}

pub fn read_state(root: &Path) -> Result<SourceState> {
    Ok(read_json_optional(root, STATE_REL)?.unwrap_or_default())
}

pub fn read_state_optional(root: &Path) -> Result<Option<SourceState>> {
    read_json_optional(root, STATE_REL)
}

pub fn write_state(root: &Path, state: &SourceState) -> Result<()> {
    write_private_json(root, STATE_REL, state)
}

pub fn read_decisions(root: &Path) -> Result<Vec<Decision>> {
    let path = root.join(DECISIONS_REL);
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(storage(e)),
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let d: Decision = serde_json::from_str(trimmed).map_err(storage)?;
        out.push(d);
    }
    Ok(out)
}

pub fn write_decisions(root: &Path, decisions: &[Decision]) -> Result<()> {
    let path = root.join(DECISIONS_REL);
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    let mut buf = Vec::new();
    for d in decisions {
        let line = serde_json::to_string(d).map_err(storage)?;
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
    }
    atomic_replace(&path, &buf, AtomicWriteOptions { mode: Some(0o600) }).map_err(storage)
}

pub fn read_landed(root: &Path) -> Result<Vec<LandedRecord>> {
    let path = root.join(LANDED_REL);
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(storage(e)),
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let r: LandedRecord = serde_json::from_str(trimmed).map_err(storage)?;
        out.push(r);
    }
    Ok(out)
}

pub fn write_landed(root: &Path, records: &[LandedRecord]) -> Result<()> {
    let path = root.join(LANDED_REL);
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    let mut buf = Vec::new();
    for r in records {
        let line = serde_json::to_string(r).map_err(storage)?;
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
    }
    atomic_replace(&path, &buf, AtomicWriteOptions { mode: Some(0o600) }).map_err(storage)
}

pub fn read_runs(root: &Path) -> Result<Vec<SyncRunRecord>> {
    Ok(read_json_optional(root, RUNS_REL)?.unwrap_or_default())
}

pub fn write_runs(root: &Path, runs: &[SyncRunRecord]) -> Result<()> {
    write_private_json(root, RUNS_REL, &runs)
}

/// Remove the transient fetched-bodies holding folder (`cache/email/bodies/`).
pub fn clear_transient_bodies(root: &Path) {
    let dir = bodies_dir(root);
    if dir.exists() {
        let _ = fs::remove_dir_all(&dir);
    }
}

/// Map every landed `email_key` to whether its journal segment is currently
/// live (transcript file exists and is not tombstoned in Trash).
#[must_use]
pub fn live_landed_map(root: &Path, landed: &[LandedRecord]) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    for rec in landed {
        let seg_dir = root.join("chronicle").join(&rec.segment_rel);
        let live = seg_dir.join("conversation_transcript.jsonl").is_file()
            && !seg_dir.join("tombstone.json").is_file();
        out.insert(rec.key.clone(), live);
    }
    out
}

/// Set of all email keys ever landed (including those later deleted by the owner).
#[must_use]
pub fn landed_key_set(landed: &[LandedRecord]) -> BTreeSet<String> {
    landed.iter().map(|r| r.key.clone()).collect()
}

/// Map of `key -> Decision` from a slice.
#[must_use]
pub fn decision_map(decisions: &[Decision]) -> HashMap<String, Decision> {
    decisions
        .iter()
        .map(|d| (d.key.clone(), d.clone()))
        .collect()
}

/// Read the journal's configured timezone from `config/journal.json`, falling
/// back to the system timezone or UTC.
#[must_use]
pub fn journal_timezone(root: &Path) -> jiff::tz::TimeZone {
    if let Ok(loaded) = solstone_core_journal_config::read_journal_config(root)
        && let Some(cfg) = loaded.config
        && let Some(tz_name) = cfg
            .get("journal")
            .and_then(|j| j.get("timezone"))
            .and_then(Value::as_str)
            .or_else(|| cfg.get("timezone").and_then(Value::as_str))
        && let Ok(tz) = jiff::tz::TimeZone::get(tz_name)
    {
        return tz;
    }
    jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC)
}
