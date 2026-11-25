// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Garmin Connect Personal API owner-key credentials, 24h bearer JWT cache,
//! fixed-origin HTTP transport, and status ledger.
//!
//! Strictly owner-local: the owner's `GCDC_` key lives only in
//! `config/journal.json` (redacted from public config projections), exchanges
//! directly with `https://personal.connectapi.garmin.com`, and caches its
//! short-lived bearer JWT in `0o600` `health/garmin-bearer.json`
//! (excluded from backups and portable copies).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use jiff::civil::Date;
use jiff::{Span, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, JsonWriteOptions, LockOptions, atomic_replace,
    create_directory_with_mode, hold_lock, lock_is_held, write_json,
};

mod http;
pub use http::LiveGarminHttp;

/// Fixed Garmin Personal API origin. No response URL is ever followed.
pub const API_ORIGIN: &str = "https://personal.connectapi.garmin.com";
/// Token exchange endpoint: POST with `Authorization: ApiKey <key>` and `Accept: */*`.
pub const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";
/// Persistent source status & run ledger (never holds credentials or remote health values).
pub const STATE_REL: &str = "imports/garmin-source.json";
/// Local 24h bearer JWT cache (mode `0o600`, excluded from backups/copies).
pub const TOKEN_CACHE_REL: &str = "health/garmin-bearer.json";
/// Source-wide lock shared across sync runs, key changes, and per-run trash moves.
pub const LOCK_REL: &str = "imports/.garmin";
/// Coverage ledger for newest-first backfill & hourly trailing convergence.
pub const COVERAGE_REL: &str = "imports/.garmin-coverage.json";

/// Minimum spacing between live HTTP requests (~92 req/min, under Garmin's 100 req/min cap).
pub const MIN_PACE_MS: u64 = 650;
/// Re-exchange the cached bearer JWT when this many seconds or fewer remain.
const TOKEN_REFRESH_LEAD_SECS: i64 = 300;
/// Garmin shortens minted JWT TTL below 24h (`86400s`) when the underlying API key
/// expires within 24h. Flag pre-expiry below 23h to allow clock skew.
const KEY_EXPIRING_TTL_THRESHOLD_SECS: i64 = 82_800;
/// Advisory warning window ahead of an owner-supplied `key_expires_on` date.
const KEY_EXPIRY_WARNING_DAYS: i64 = 7;
/// Suppress isolated transient hourly blips until this many consecutive hourly runs fail.
pub const TRANSIENT_FAILURE_THRESHOLD: u32 = 3;
/// Maximum stored run records in `imports/garmin-source.json`.
const MAX_STORED_RUNS: usize = 500;
const PRIVATE_FILE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// Closed failures intentionally carry no key, JWT, URL, or response body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidKey,
    InvalidExpiry,
    InvalidRange,
    Key,
    Access,
    Limited,
    Unavailable,
    Response,
    Storage,
    Busy,
    Stopped,
    NotConnected,
    UnavailableOnMobile,
}

impl Error {
    /// Stable wire code shared across settings API, Sources summary, and CLI.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidKey => "invalid_key",
            Self::InvalidExpiry => "invalid_expiry",
            Self::InvalidRange => "invalid_range",
            Self::Key => "key",
            Self::Access => "access",
            Self::Limited => "limited",
            Self::Unavailable => "unavailable",
            Self::Response => "response",
            Self::Storage => "storage",
            Self::Busy => "busy",
            Self::Stopped => "stopped",
            Self::NotConnected => "not_connected",
            Self::UnavailableOnMobile => "unavailable_on_mobile",
        }
    }

    /// Plain English diagnostic for logs and CLI fallback.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidKey => {
                "that isn't a complete Garmin Connect Personal API key. it starts with GCDC_."
            }
            Self::InvalidExpiry => "enter an expiration date as YYYY-MM-DD, or leave it blank.",
            Self::InvalidRange => {
                "enter a valid date range (YYYY-MM-DD) ending on or before today."
            }
            Self::Key => {
                "Garmin no longer accepts this key. it may have expired, been revoked, or been replaced by a newer key. add a new key to carry on."
            }
            Self::Access => {
                "Garmin turned this key away. check your Personal API key in Garmin Connect and add a new key if needed."
            }
            Self::Limited => {
                "Garmin asked your journal to slow down. it will try again on its next check."
            }
            Self::Unavailable => {
                "couldn't reach Garmin. your journal will try again on its next check."
            }
            Self::Response => {
                "Garmin sent something your journal couldn't read. it will try again on its next check."
            }
            Self::Storage => {
                "your journal couldn't save what came from Garmin. check that this computer has free space."
            }
            Self::Busy => "your journal is already checking Garmin. try again when it finishes.",
            Self::Stopped => {
                "the check stopped before it finished. your journal will pick it up on its next check."
            }
            Self::NotConnected => "Garmin Connect isn't connected.",
            Self::UnavailableOnMobile => {
                "Garmin Connect sync runs on desktop journals and isn't available on mobile journals."
            }
        }
    }

    /// Whether this failure is a transient blip suppressed on isolated hourly runs.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(
            self,
            Self::Limited | Self::Unavailable | Self::Response | Self::Stopped
        )
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for Error {}

/// Raw bounded HTTP response returned by [`GarminHttp`].
#[derive(Clone, Debug)]
pub struct RawResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub retry_after_secs: Option<u64>,
}

/// Fixed-origin HTTP transport boundary. Tests inject recorded or loopback responses.
pub trait GarminHttp {
    /// `POST /personalapitoken-service/personal-api/personal-api-key/token`
    /// with `Authorization: ApiKey <api_key>` and `Accept: */*`.
    fn exchange_token(&mut self, api_key: &str) -> Result<RawResponse, Error>;

    /// `GET <path>` with `Authorization: Bearer <bearer>` and `Accept: application/json`.
    fn get_json(
        &mut self,
        bearer: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<RawResponse, Error>;
}

/// Cached 24h bearer JWT stored in `health/garmin-bearer.json` (`0o600`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCache {
    pub key_sha256: String,
    pub access_token: String,
    pub expires_at_epoch_s: i64,
    #[serde(default)]
    pub key_expiring_within_24h: bool,
}

/// One Garmin sync run that actually wrote rows into the journal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<[String; 2]>,
    #[serde(default)]
    pub days_count: usize,
    #[serde(default)]
    pub rows: u64,
    #[serde(default)]
    pub workouts: u64,
}

/// Live newest-first backfill progress reported to Settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackfillState {
    #[serde(default)]
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached_day: Option<String>,
}

/// Persistent status ledger kept in `imports/garmin-source.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_added: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_code: Option<String>,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub backfill: BackfillState,
    #[serde(default)]
    pub runs: Vec<RunRecord>,
}

/// Snapshot produced by [`status`] for `/app/settings/api/garmin` and CLI status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GarminStatus {
    pub available: bool,
    pub connected: bool,
    pub syncing: bool,
    pub key_expires_on: Option<String>,
    pub key_expiring_soon: bool,
    pub last_sync: Option<String>,
    pub last_added: Option<String>,
    pub attention_code: Option<String>,
    pub days_covered: usize,
    pub workouts: u64,
    pub backfill: BackfillState,
    pub runs: Vec<RunRecord>,
}

fn storage<T>(_: T) -> Error {
    Error::Storage
}

/// SHA-256 hex digest of a key, used only to bind the cached JWT to the active key.
#[must_use]
pub fn key_sha256(api_key: &str) -> String {
    Sha256::digest(api_key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Validate a Garmin Connect Personal API key (`GCDC_...`).
pub fn validate_key(key: &str) -> Result<(), Error> {
    if !key.starts_with("GCDC_")
        || key.len() <= 5
        || key.len() > 4096
        || !key.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(Error::InvalidKey);
    }
    Ok(())
}

/// Validate an optional owner-supplied expiration date (`YYYY-MM-DD`).
pub fn validate_expiry(text: &str) -> Result<String, Error> {
    let trimmed = text.trim();
    if trimmed.len() != 10 {
        return Err(Error::InvalidExpiry);
    }
    let date = Date::strptime("%Y-%m-%d", trimmed).map_err(|_| Error::InvalidExpiry)?;
    Ok(date.strftime("%Y-%m-%d").to_string())
}

/// Read the saved Garmin API key from `config/journal.json`.
pub fn key(root: &Path) -> Result<Option<String>, Error> {
    let config = solstone_core_journal_config::read_journal_config(root)
        .map_err(storage)?
        .config
        .unwrap_or_default();
    Ok(config
        .get("garmin")
        .and_then(|value| value.get("api_key"))
        .and_then(Value::as_str)
        .map(str::to_owned))
}

/// Read the optional saved key expiration date (`YYYY-MM-DD`) from `config/journal.json`.
pub fn key_expires_on(root: &Path) -> Result<Option<String>, Error> {
    let config = solstone_core_journal_config::read_journal_config(root)
        .map_err(storage)?
        .config
        .unwrap_or_default();
    Ok(config
        .get("garmin")
        .and_then(|value| value.get("key_expires_on"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned))
}

/// Serialize connection mutations with sync runs without holding config locks over HTTP.
pub fn hold(root: &Path) -> Result<FileLock, Error> {
    create_directory_with_mode(&root.join("imports"), 0o700).map_err(storage)?;
    hold_lock(
        root.join(LOCK_REL),
        LockOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )
    .map_err(|_| Error::Busy)
}

/// Whether a Garmin sync or key-change hold currently owns `imports/.garmin`.
#[must_use]
pub fn is_sync_locked(root: &Path) -> bool {
    lock_is_held(root.join(LOCK_REL)).unwrap_or(false)
}

/// Read `imports/garmin-source.json`, returning default empty state if absent.
pub fn read_state(root: &Path) -> Result<SourceState, Error> {
    match fs::read(root.join(STATE_REL)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(storage),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SourceState::default()),
        Err(error) => Err(storage(error)),
    }
}

/// Persist `imports/garmin-source.json` atomically with `0o600`.
pub fn save_state(root: &Path, state: &SourceState) -> Result<(), Error> {
    create_directory_with_mode(&root.join("imports"), 0o700).map_err(storage)?;
    write_json(
        root.join(STATE_REL),
        state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )
    .map_err(storage)
}

/// Whether hourly scheduled runs should pause data requests because Garmin already
/// rejected the current key (`401`). A new key clears this gate.
#[must_use]
pub fn paused_for_key_error(root: &Path) -> bool {
    read_state(root)
        .ok()
        .and_then(|state| state.attention_code)
        .as_deref()
        == Some(Error::Key.code())
}

/// Save or clear the owner's Garmin key in `config/journal.json`.
///
/// When replacing a key, clears any previous `attention_code` and `consecutive_failures`
/// and drops any token cache minted for an earlier key. When clearing (`None`), removes
/// the credential and token cache while keeping all synced journal data and run history.
pub fn set_key(root: &Path, api_key: Option<&str>, expires_on: Option<&str>) -> Result<(), Error> {
    let normalized_expiry = match expires_on.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Some(validate_expiry(value)?),
        None => None,
    };
    if let Some(api_key) = api_key {
        validate_key(api_key)?;
    }
    use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
    mutate_journal_config(root, LockOptions::default(), |config| {
        if let Some(api_key) = api_key {
            let mut section = serde_json::Map::new();
            section.insert("api_key".to_owned(), Value::String(api_key.to_owned()));
            if let Some(expiry) = &normalized_expiry {
                section.insert("key_expires_on".to_owned(), Value::String(expiry.clone()));
            }
            config.insert("garmin".to_owned(), Value::Object(section));
        } else {
            config.remove("garmin");
        }
        JournalConfigMutation {
            changed: true,
            value: (),
        }
    })
    .map_err(storage)?;

    let mut state = read_state(root).unwrap_or_default();
    state.attention_code = None;
    state.consecutive_failures = 0;
    save_state(root, &state)?;

    if let Some(api_key) = api_key {
        let digest = key_sha256(api_key);
        if read_token_cache(root).is_some_and(|cached| cached.key_sha256 != digest) {
            clear_token_cache(root);
        }
    } else {
        clear_token_cache(root);
    }
    Ok(())
}

/// Probe a candidate key against Garmin's token-exchange endpoint (20s timeout)
/// without mutating journal config. Returns the minted [`TokenCache`] on success.
pub fn check_key(api_key: &str) -> Result<TokenCache, Error> {
    validate_key(api_key)?;
    let mut http = LiveGarminHttp::probe();
    exchange_key_with(&mut http, api_key, Timestamp::now().as_second())
}

/// Exchange an API key for a 24h bearer JWT through `http`.
pub fn exchange_key_with(
    http: &mut dyn GarminHttp,
    api_key: &str,
    now_epoch_s: i64,
) -> Result<TokenCache, Error> {
    validate_key(api_key)?;
    let response = http.exchange_token(api_key)?;
    match response.status {
        200 => {}
        401 => return Err(Error::Key),
        403 => return Err(Error::Access),
        429 => return Err(Error::Limited),
        500..=599 => return Err(Error::Unavailable),
        _ => return Err(Error::Response),
    }
    let token = std::str::from_utf8(&response.body)
        .map_err(|_| Error::Response)?
        .trim();
    // Garmin returns a bare JWT as `text/plain`, never JSON-wrapped.
    if token.is_empty()
        || token.len() > 16_384
        || !token.bytes().all(|byte| byte.is_ascii_graphic())
        || token.split('.').count() != 3
    {
        return Err(Error::Response);
    }
    let (iat, exp) = parse_jwt_claims(token).unwrap_or((None, now_epoch_s + 23 * 3_600));
    let key_expiring_within_24h = match iat {
        Some(issued) if exp >= issued => (exp - issued) < KEY_EXPIRING_TTL_THRESHOLD_SECS,
        _ => (exp - now_epoch_s) < KEY_EXPIRING_TTL_THRESHOLD_SECS,
    };
    Ok(TokenCache {
        key_sha256: key_sha256(api_key),
        access_token: token.to_owned(),
        expires_at_epoch_s: exp,
        key_expiring_within_24h,
    })
}

/// Read `health/garmin-bearer.json` if present and valid.
#[must_use]
pub fn read_token_cache(root: &Path) -> Option<TokenCache> {
    let bytes = fs::read(root.join(TOKEN_CACHE_REL)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Atomically persist `health/garmin-bearer.json` under its own file lock (`0o600`).
pub fn save_token_cache(root: &Path, cache: &TokenCache) -> Result<(), Error> {
    create_directory_with_mode(&root.join("health"), 0o700).map_err(storage)?;
    let path = root.join(TOKEN_CACHE_REL);
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )
    .map_err(storage)?;
    let bytes = serde_json::to_vec(cache).map_err(storage)?;
    atomic_replace(&path, &bytes, PRIVATE_FILE).map_err(storage)
}

/// Remove `health/garmin-bearer.json` if present.
pub fn clear_token_cache(root: &Path) {
    let path = root.join(TOKEN_CACHE_REL);
    if let Ok(_lock) = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    ) {
        let _ = fs::remove_file(&path);
    } else {
        let _ = fs::remove_file(&path);
    }
}

/// Return a valid bearer JWT for `api_key`, reading the `flock`-guarded cache
/// first and exchanging only when missing, mismatched, near expiry (`<= 300s`),
/// or when `force_refresh` is set after a `401`.
pub fn ensure_bearer_token(
    root: &Path,
    http: &mut dyn GarminHttp,
    api_key: &str,
    force_refresh: bool,
    now_epoch_s: i64,
) -> Result<TokenCache, Error> {
    create_directory_with_mode(&root.join("health"), 0o700).map_err(storage)?;
    let path = root.join(TOKEN_CACHE_REL);
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    )
    .map_err(storage)?;
    let digest = key_sha256(api_key);
    if !force_refresh
        && let Ok(bytes) = fs::read(&path)
        && let Ok(cached) = serde_json::from_slice::<TokenCache>(&bytes)
        && cached.key_sha256 == digest
        && cached.expires_at_epoch_s > now_epoch_s + TOKEN_REFRESH_LEAD_SECS
    {
        return Ok(cached);
    }
    match exchange_key_with(http, api_key, now_epoch_s) {
        Ok(fresh) => {
            let bytes = serde_json::to_vec(&fresh).map_err(storage)?;
            atomic_replace(&path, &bytes, PRIVATE_FILE).map_err(storage)?;
            Ok(fresh)
        }
        Err(Error::Key) => {
            let _ = fs::remove_file(&path);
            Err(Error::Key)
        }
        Err(error) => Err(error),
    }
}

/// Record a completed sync run's outcome in `imports/garmin-source.json`.
pub fn record_sync_success(
    root: &Path,
    now_iso: &str,
    run: Option<RunRecord>,
    backfill: BackfillState,
) -> Result<(), Error> {
    let mut state = read_state(root).unwrap_or_default();
    state.last_sync = Some(now_iso.to_owned());
    state.attention_code = None;
    state.consecutive_failures = 0;
    state.backfill = backfill;
    if let Some(run) = run
        && run.rows > 0
    {
        state.last_added = run.imported_at.clone().or_else(|| Some(now_iso.to_owned()));
        state.runs.retain(|existing| existing.id != run.id);
        state.runs.insert(0, run);
        state.runs.truncate(MAX_STORED_RUNS);
    }
    save_state(root, &state)
}

/// Record a failed sync attempt in `imports/garmin-source.json`, suppressing
/// isolated transient hourly failures until [`TRANSIENT_FAILURE_THRESHOLD`].
pub fn record_sync_failure(
    root: &Path,
    now_iso: &str,
    scheduled: bool,
    error: Error,
) -> Result<(), Error> {
    let mut state = read_state(root).unwrap_or_default();
    state.last_sync = Some(now_iso.to_owned());
    if error == Error::Key || error == Error::Access || error == Error::Storage {
        state.attention_code = Some(error.code().to_owned());
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    } else if error.is_transient() {
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if !scheduled || state.consecutive_failures >= TRANSIENT_FAILURE_THRESHOLD {
            state.attention_code = Some(error.code().to_owned());
        }
    } else {
        state.attention_code = Some(error.code().to_owned());
    }
    save_state(root, &state)
}

/// Compute Garmin's current status without reading any `body.jsonl` payloads
/// or exposing credentials.
pub fn status(
    root: &Path,
    syncing_in_process: bool,
    available: bool,
) -> Result<GarminStatus, Error> {
    let saved_key = key(root)?;
    let connected = saved_key.is_some();
    let expires_on = if connected {
        key_expires_on(root)?
    } else {
        None
    };
    let state = read_state(root)?;
    let key_expiring_soon =
        connected && is_key_expiring_soon(root, saved_key.as_deref(), expires_on.as_deref());
    let attention_code = if !connected {
        None
    } else if state.attention_code.is_some() {
        state.attention_code
    } else if key_expiring_soon {
        Some("key_expiring".to_owned())
    } else {
        None
    };
    let runs = live_runs(root, &state.runs);
    let workouts: u64 = runs.iter().map(|run| run.workouts).sum();
    let days_covered = count_live_garmin_days(root);
    Ok(GarminStatus {
        available,
        connected,
        syncing: syncing_in_process || is_sync_locked(root),
        key_expires_on: expires_on,
        key_expiring_soon,
        last_sync: state.last_sync,
        last_added: state.last_added,
        attention_code,
        days_covered,
        workouts,
        backfill: state.backfill,
        runs,
    })
}

fn is_key_expiring_soon(root: &Path, saved_key: Option<&str>, expires_on: Option<&str>) -> bool {
    if let Some(saved_key) = saved_key {
        let digest = key_sha256(saved_key);
        if read_token_cache(root)
            .is_some_and(|cache| cache.key_sha256 == digest && cache.key_expiring_within_24h)
        {
            return true;
        }
    }
    if let Some(expiry) = expires_on
        && let Ok(expiry_date) = Date::strptime("%Y-%m-%d", expiry)
    {
        let today = jiff::tz::Offset::UTC.to_datetime(Timestamp::now()).date();
        if let Ok(warning_start) =
            expiry_date.checked_sub(Span::new().days(KEY_EXPIRY_WARNING_DAYS))
            && today >= warning_start
        {
            return true;
        }
    }
    false
}

/// Filter recorded sync runs to those still live under `imports/<id>/`, unioning
/// any Garmin run directory on disk (e.g. restored from Trash or created via CLI).
fn live_runs(root: &Path, recorded: &[RunRecord]) -> Vec<RunRecord> {
    let mut by_id: BTreeMap<String, RunRecord> = BTreeMap::new();
    let imports_dir = root.join("imports");
    for run in recorded {
        if imports_dir.join(&run.id).is_dir() {
            by_id.insert(run.id.clone(), run.clone());
        }
    }
    if let Ok(entries) = fs::read_dir(&imports_dir) {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if by_id.contains_key(&name)
                || !crate::import_trash::is_import_id(&name)
                || !entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                continue;
            }
            let manifest_path = entry.path().join("manifest.json");
            let Ok(bytes) = fs::read(&manifest_path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if manifest.get("source_type").and_then(Value::as_str) != Some("garmin") {
                continue;
            }
            let days_affected: BTreeSet<String> = manifest
                .get("days_affected")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let days = days_affected
                .first()
                .zip(days_affected.last())
                .map(|(first, last)| [first.clone(), last.clone()]);
            let rows = manifest
                .get("entry_count")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let imported_at = manifest
                .get("imported_at")
                .and_then(Value::as_str)
                .map(str::to_owned);
            by_id.insert(
                name.clone(),
                RunRecord {
                    id: name,
                    imported_at,
                    days,
                    days_count: days_affected.len(),
                    rows,
                    workouts: 0,
                },
            );
        }
    }
    let mut runs: Vec<RunRecord> = by_id.into_values().collect();
    runs.sort_by(|left, right| right.id.cmp(&left.id));
    runs.truncate(MAX_STORED_RUNS);
    runs
}

/// Directory-only count of distinct days holding at least one live `import.garmin` window.
fn count_live_garmin_days(root: &Path) -> usize {
    let Ok(days) = solstone_core_journal_io::day_dirs(root) else {
        return 0;
    };
    let mut count = 0;
    for day in days.into_keys() {
        let dir = root.join("chronicle").join(&day).join("import.garmin");
        let Ok(windows) = fs::read_dir(&dir) else {
            continue;
        };
        let has_live = windows.flatten().any(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            !name.starts_with('.')
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
                && !entry.path().join("tombstone.json").exists()
        });
        if has_live {
            count += 1;
        }
    }
    count
}

/// Decode unverified JWT payload `iat` and `exp` claims using base64url.
fn parse_jwt_claims(token: &str) -> Option<(Option<i64>, i64)> {
    let payload_b64 = token.split('.').nth(1)?;
    let bytes = decode_base64url(payload_b64)?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = claims.get("exp")?.as_i64()?;
    let iat = claims.get("iat").and_then(Value::as_i64);
    Some((iat, exp))
}

fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Serialize a [`GarminStatus`] to its wire JSON shape (with caller-localized `attention`).
#[must_use]
pub fn status_json(status: &GarminStatus, attention: Option<String>) -> Value {
    json!({
        "available": status.available,
        "connected": status.connected,
        "syncing": status.syncing,
        "key_expires_on": status.key_expires_on,
        "key_expiring_soon": status.key_expiring_soon,
        "last_sync": status.last_sync,
        "last_added": status.last_added,
        "attention": attention,
        "attention_code": status.attention_code,
        "days_covered": status.days_covered,
        "workouts": status.workouts,
        "backfill": {
            "complete": status.backfill.complete,
            "reached_day": status.backfill.reached_day,
        },
        "runs": status.runs.iter().map(|run| json!({
            "id": run.id,
            "imported_at": run.imported_at,
            "days": run.days,
            "days_count": run.days_count,
            "rows": run.rows,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    struct StubHttp {
        token_responses: std::collections::VecDeque<Result<RawResponse, Error>>,
        token_calls: usize,
    }

    impl GarminHttp for StubHttp {
        fn exchange_token(&mut self, _api_key: &str) -> Result<RawResponse, Error> {
            self.token_calls += 1;
            self.token_responses
                .pop_front()
                .unwrap_or(Err(Error::Unavailable))
        }

        fn get_json(
            &mut self,
            _bearer: &str,
            _path: &str,
            _query: &[(&str, String)],
        ) -> Result<RawResponse, Error> {
            Err(Error::Unavailable)
        }
    }

    fn encode_base64url(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = u32::from(chunk[0]);
            let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
            let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
            let triple = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
            if chunk.len() > 1 {
                out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
            }
            if chunk.len() > 2 {
                out.push(TABLE[(triple & 0x3f) as usize] as char);
            }
        }
        out
    }

    fn make_jwt(iat: i64, exp: i64) -> String {
        let header = encode_base64url(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = encode_base64url(format!(r#"{{"iat":{iat},"exp":{exp}}}"#).as_bytes());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn validates_key_and_expiry_formats() {
        assert!(validate_key("GCDC_test_token_12345").is_ok());
        assert_eq!(validate_key("GCDC_"), Err(Error::InvalidKey));
        assert_eq!(validate_key("grn_abc"), Err(Error::InvalidKey));
        assert_eq!(validate_key("GCDC_has space"), Err(Error::InvalidKey));
        assert_eq!(validate_expiry("2027-02-22"), Ok("2027-02-22".to_owned()));
        assert_eq!(validate_expiry("2027-02-30"), Err(Error::InvalidExpiry));
        assert_eq!(validate_expiry("not-a-date"), Err(Error::InvalidExpiry));
    }

    #[test]
    fn token_cache_reuses_valid_jwt_detects_short_ttl_and_clears_on_401() {
        let root = tempfile::tempdir().unwrap();
        let now = 1_800_000_000_i64;
        let full_jwt = make_jwt(now, now + 86_400);
        let short_jwt = make_jwt(now, now + 36_000);
        let mut http = StubHttp {
            token_responses: [
                Ok(RawResponse {
                    status: 200,
                    body: full_jwt.clone().into_bytes(),
                    retry_after_secs: None,
                }),
                Ok(RawResponse {
                    status: 200,
                    body: short_jwt.clone().into_bytes(),
                    retry_after_secs: None,
                }),
                Ok(RawResponse {
                    status: 401,
                    body: Vec::new(),
                    retry_after_secs: None,
                }),
            ]
            .into(),
            token_calls: 0,
        };
        let first =
            ensure_bearer_token(root.path(), &mut http, "GCDC_secret_1", false, now).unwrap();
        assert_eq!(first.access_token, full_jwt);
        assert!(!first.key_expiring_within_24h);
        assert_eq!(http.token_calls, 1);

        // Within cache lifetime: reuses cached token without calling HTTP.
        let cached =
            ensure_bearer_token(root.path(), &mut http, "GCDC_secret_1", false, now + 3_600)
                .unwrap();
        assert_eq!(cached.access_token, full_jwt);
        assert_eq!(http.token_calls, 1);

        // Near expiry (<= 300s left): re-exchanges and catches Garmin's shortened <24h TTL.
        let refreshed =
            ensure_bearer_token(root.path(), &mut http, "GCDC_secret_1", false, now + 86_200)
                .unwrap();
        assert_eq!(refreshed.access_token, short_jwt);
        assert!(refreshed.key_expiring_within_24h);
        assert_eq!(http.token_calls, 2);

        // Forced refresh returning 401 deletes cache and returns Error::Key.
        let err = ensure_bearer_token(root.path(), &mut http, "GCDC_secret_1", true, now + 86_201)
            .unwrap_err();
        assert_eq!(err, Error::Key);
        assert!(read_token_cache(root.path()).is_none());
    }

    #[test]
    fn transient_hourly_failures_suppress_until_third_consecutive_failure() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(root.path().join("config/journal.json"), "{}").unwrap();
        set_key(root.path(), Some("GCDC_secret_key"), None).unwrap();

        record_sync_failure(
            root.path(),
            "2026-11-24T10:00:00Z",
            true,
            Error::Unavailable,
        )
        .unwrap();
        assert_eq!(read_state(root.path()).unwrap().attention_code, None);

        record_sync_failure(
            root.path(),
            "2026-11-24T11:00:00Z",
            true,
            Error::Unavailable,
        )
        .unwrap();
        assert_eq!(read_state(root.path()).unwrap().attention_code, None);

        record_sync_failure(
            root.path(),
            "2026-11-24T12:00:00Z",
            true,
            Error::Unavailable,
        )
        .unwrap();
        assert_eq!(
            read_state(root.path()).unwrap().attention_code.as_deref(),
            Some("unavailable")
        );

        // A 401 key failure surfaces immediately even on the first failure.
        set_key(root.path(), Some("GCDC_secret_key_2"), None).unwrap();
        record_sync_failure(root.path(), "2026-11-24T13:00:00Z", true, Error::Key).unwrap();
        assert_eq!(
            read_state(root.path()).unwrap().attention_code.as_deref(),
            Some("key")
        );
        assert!(paused_for_key_error(root.path()));
    }
}
