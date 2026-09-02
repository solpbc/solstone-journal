// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

const STATUS_MODE: u32 = 0o600;
pub const PROGRESS_COALESCE_SECONDS: Duration = Duration::from_secs(1);
static ATTEMPT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The install-status domain's complete provider allowlist, matching
/// Python's `install_state.PROVIDERS`. `read_status` and `write_status`
/// validate against this and build their rejection message from it, so the
/// accepted set and the message describing it can never drift apart.
pub const PROVIDERS: &[&str] = &["local", "parakeet"];

fn unknown_provider() -> StatusError {
    let mut sorted = PROVIDERS.to_vec();
    sorted.sort_unstable();
    let listed = sorted
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    StatusError::Malformed(format!(
        "provider install status must be one of: [{listed}]"
    ))
}

#[derive(Debug, Error)]
pub enum StatusError {
    #[error("malformed install status: {0}")]
    Malformed(String),
    #[error("install status conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("status lock error: {0}")]
    Lock(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallStatus {
    pub schema_version: u64,
    pub provider: String,
    pub revision: u64,
    pub install_state: String,
    pub attempt_id: Option<String>,
    pub target_fingerprint_json: Option<String>,
    pub target_fingerprint_sha256: Option<String>,
    pub started_at: Option<String>,
    pub last_transition_at: Option<String>,
    pub last_progress_at: Option<String>,
    pub completed_at: Option<String>,
    pub progress_bytes_received: Option<u64>,
    pub progress_bytes_total: Option<u64>,
    pub install_error: Option<String>,
    pub error_code: Option<String>,
    pub owner: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObserveAttempt {
    Terminal(InstallStatus),
    DifferentTarget(InstallStatus),
    TimedOut,
}

pub fn status_path(journal: &Path, provider: &str) -> PathBuf {
    journal
        .join("health/providers")
        .join(format!("{provider}.json"))
}

pub fn idle_status(provider: &str) -> InstallStatus {
    InstallStatus {
        schema_version: 1,
        provider: provider.to_owned(),
        revision: 0,
        install_state: "idle".to_owned(),
        attempt_id: None,
        target_fingerprint_json: None,
        target_fingerprint_sha256: None,
        started_at: None,
        last_transition_at: None,
        last_progress_at: None,
        completed_at: None,
        progress_bytes_received: None,
        progress_bytes_total: None,
        install_error: None,
        error_code: None,
        owner: None,
    }
}

pub fn is_in_flight(state: &str) -> bool {
    matches!(
        state,
        "resolving" | "downloading" | "verifying" | "installing"
    )
}

pub fn is_terminal(state: &str) -> bool {
    matches!(state, "idle" | "installed" | "failed")
}

fn valid_state(state: &str) -> bool {
    is_in_flight(state) || is_terminal(state)
}

fn validate(status: &InstallStatus, provider: &str) -> Result<(), StatusError> {
    if status.schema_version != 1
        || status.provider != provider
        || !valid_state(&status.install_state)
    {
        return Err(StatusError::Malformed(format!(
            "invalid status for {provider}"
        )));
    }
    if status
        .owner
        .as_ref()
        .is_some_and(|owner| !owner.is_object())
    {
        return Err(StatusError::Malformed(
            "owner must be an object or null".to_owned(),
        ));
    }
    Ok(())
}

pub fn read_status(journal: &Path, provider: &str) -> Result<InstallStatus, StatusError> {
    if !PROVIDERS.contains(&provider) {
        return Err(unknown_provider());
    }
    let path = status_path(journal, provider);
    if !path.exists() {
        return Ok(idle_status(provider));
    }
    let status: InstallStatus = serde_json::from_slice(&fs::read(&path)?)
        .map_err(|_| StatusError::Malformed(format!("{}", path.display())))?;
    validate(&status, provider)?;
    Ok(status)
}

pub fn begin(
    journal: &Path,
    fingerprint_json: String,
    fingerprint_sha256: String,
    owner: Option<Value>,
    state: &str,
) -> Result<InstallStatus, StatusError> {
    if !is_in_flight(state) {
        return Err(StatusError::Malformed(
            "initial install attempt state must be in-flight".to_owned(),
        ));
    }
    let mut next = read_status(journal, "local")?;
    next.target_fingerprint_json = Some(fingerprint_json);
    next.target_fingerprint_sha256 = Some(fingerprint_sha256);
    next.attempt_id = Some(new_attempt_id());
    next.owner = owner;
    write_status(journal, transition(next, state, None, None)?)
}

pub fn assert_current(
    journal: &Path,
    attempt: &InstallStatus,
) -> Result<InstallStatus, StatusError> {
    let current = read_status(journal, &attempt.provider)?;
    if !is_in_flight(&current.install_state) {
        return Err(StatusError::Conflict(
            "install attempt is no longer in-flight".to_owned(),
        ));
    }
    if current.attempt_id != attempt.attempt_id {
        return Err(StatusError::Conflict(
            "install attempt id changed".to_owned(),
        ));
    }
    if current.target_fingerprint_sha256 != attempt.target_fingerprint_sha256 {
        return Err(StatusError::Conflict(
            "install target fingerprint changed".to_owned(),
        ));
    }
    Ok(current)
}

pub fn observe_attempt<F>(
    journal: &Path,
    provider: &str,
    target_fingerprint_sha256: &str,
    poll_interval: Duration,
    timeout: Duration,
    progress_interval: Duration,
    mut progress: F,
) -> Result<ObserveAttempt, StatusError>
where
    F: FnMut(&InstallStatus),
{
    let deadline = Instant::now() + timeout;
    let mut last_progress_at = None;
    let mut last_progress_key = None;
    loop {
        let current = read_status(journal, provider)?;
        if current.target_fingerprint_sha256.as_deref() != Some(target_fingerprint_sha256) {
            return Ok(ObserveAttempt::DifferentTarget(current));
        }
        let key = (
            current.install_state.clone(),
            current.progress_bytes_received,
            current.progress_bytes_total,
            current.install_error.clone(),
            current.error_code.clone(),
        );
        let now = Instant::now();
        if last_progress_key.as_ref() != Some(&key)
            || last_progress_at.is_none_or(|last| now.duration_since(last) >= progress_interval)
        {
            progress(&current);
            last_progress_key = Some(key);
            last_progress_at = Some(now);
        }
        if is_terminal(&current.install_state) {
            return Ok(ObserveAttempt::Terminal(current));
        }
        if now >= deadline {
            return Ok(ObserveAttempt::TimedOut);
        }
        std::thread::sleep(poll_interval);
    }
}

pub fn record_interrupted(
    journal: &Path,
    attempt_id: &str,
    target_fingerprint_sha256: Option<&str>,
) -> Result<InstallStatus, StatusError> {
    let current = read_status(journal, "local")?;
    if !is_in_flight(&current.install_state) {
        return Err(StatusError::Conflict(
            "only in-flight installs can be interrupted".to_owned(),
        ));
    }
    if current.attempt_id.as_deref() != Some(attempt_id) {
        return Err(StatusError::Conflict(
            "interrupted attempt id does not match".to_owned(),
        ));
    }
    if current.target_fingerprint_sha256.as_deref() != target_fingerprint_sha256 {
        return Err(StatusError::Conflict(
            "interrupted target fingerprint does not match".to_owned(),
        ));
    }
    write_status(
        journal,
        transition(
            current,
            "failed",
            Some("install_interrupted".to_owned()),
            Some("install_interrupted".to_owned()),
        )?,
    )
}

fn status_lock(path: &Path) -> Result<File, StatusError> {
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    set_mode(&file)?;
    file.lock()
        .map_err(|err| StatusError::Lock(err.to_string()))?;
    Ok(file)
}

fn set_mode(_file: &File) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        _file.set_permissions(fs::Permissions::from_mode(STATUS_MODE))?;
    }
    Ok(())
}

fn atomic_replace(path: &Path, status: &InstallStatus) -> Result<(), StatusError> {
    let parent = path.parent().expect("status path has parent");
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        ATTEMPT_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    set_mode(&file)?;
    let content = serde_json::to_string_pretty(status)?;
    file.write_all(content.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn read_unlocked(path: &Path, provider: &str) -> Result<InstallStatus, StatusError> {
    if !path.exists() {
        return Ok(idle_status(provider));
    }
    let status: InstallStatus = serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| StatusError::Malformed(format!("{}", path.display())))?;
    validate(&status, provider)?;
    Ok(status)
}

pub fn write_status(journal: &Path, incoming: InstallStatus) -> Result<InstallStatus, StatusError> {
    let provider = incoming.provider.clone();
    if !PROVIDERS.contains(&provider.as_str()) {
        return Err(unknown_provider());
    }
    validate(&incoming, &provider)?;
    let path = status_path(journal, &provider);
    let _lock = status_lock(&path)?;
    let current = read_unlocked(&path, &provider)?;
    let accepted = accept_transition(&current, &incoming)?;
    if accepted == current {
        return Ok(current);
    }
    let mut stored = accepted;
    stored.revision = current.revision + 1;
    atomic_replace(&path, &stored)?;
    Ok(stored)
}

fn accept_transition(
    current: &InstallStatus,
    incoming: &InstallStatus,
) -> Result<InstallStatus, StatusError> {
    let same_attempt = incoming.attempt_id.is_some() && incoming.attempt_id == current.attempt_id;
    if is_terminal(&current.install_state) && same_attempt {
        return Ok(current.clone());
    }
    if is_terminal(&current.install_state) {
        if incoming.revision != current.revision {
            return Err(StatusError::Conflict(
                "stale install status revision".to_owned(),
            ));
        }
        if incoming.install_state == "idle" {
            return Ok(incoming.clone());
        }
        if incoming.attempt_id.is_none() {
            return Err(StatusError::Conflict(
                "non-idle install status requires attempt id".to_owned(),
            ));
        }
        if is_in_flight(&incoming.install_state) && incoming.attempt_id == current.attempt_id {
            return Err(StatusError::Conflict(
                "new in-flight attempt reused attempt id".to_owned(),
            ));
        }
        if is_in_flight(&incoming.install_state)
            || matches!(incoming.install_state.as_str(), "installed" | "failed")
        {
            return Ok(incoming.clone());
        }
    }
    if is_in_flight(&current.install_state) {
        if !same_attempt {
            return Err(StatusError::Conflict(
                "different attempt while install in-flight".to_owned(),
            ));
        }
        if incoming.revision != current.revision {
            return Err(StatusError::Conflict(
                "stale install status revision".to_owned(),
            ));
        }
        if is_in_flight(&incoming.install_state) {
            if current.install_state == incoming.install_state
                && current.progress_bytes_total == incoming.progress_bytes_total
                && current.progress_bytes_received == incoming.progress_bytes_received
            {
                return Ok(current.clone());
            }
            if current.install_state == incoming.install_state
                && current.progress_bytes_total == incoming.progress_bytes_total
                && current.progress_bytes_received == incoming.progress_bytes_received
                && current.last_progress_at == incoming.last_progress_at
            {
                return Ok(current.clone());
            }
            return Ok(incoming.clone());
        }
        if matches!(incoming.install_state.as_str(), "installed" | "failed") {
            return Ok(incoming.clone());
        }
    }
    Err(StatusError::Conflict(
        "illegal install status transition".to_owned(),
    ))
}

pub fn transition(
    mut current: InstallStatus,
    state: &str,
    error: Option<String>,
    error_code: Option<String>,
) -> Result<InstallStatus, StatusError> {
    if !valid_state(state) {
        return Err(StatusError::Malformed(format!(
            "unknown install state: {state}"
        )));
    }
    let timestamp = now();
    let was_terminal = is_terminal(&current.install_state);
    if was_terminal && is_in_flight(state) {
        current.attempt_id = Some(new_attempt_id());
    }
    if current.attempt_id.is_none() && state != "idle" {
        current.attempt_id = Some(new_attempt_id());
    }
    current.install_state = state.to_owned();
    if state == "idle" {
        current.attempt_id = None;
    }
    if was_terminal && is_in_flight(state) {
        current.started_at = Some(timestamp.clone());
    }
    current.last_transition_at = Some(timestamp.clone());
    current.last_progress_at = is_in_flight(state).then_some(timestamp.clone());
    current.completed_at = (is_terminal(state) && state != "idle").then_some(timestamp);
    if is_terminal(state) {
        current.progress_bytes_received = None;
        current.progress_bytes_total = None;
    }
    current.install_error = (state == "failed").then_some(error).flatten();
    current.error_code = (state == "failed").then_some(error_code).flatten();
    Ok(current)
}

pub fn begin_or_replace(
    journal: &Path,
    provider: &str,
    fingerprint_json: String,
    fingerprint_sha256: String,
    owner: Option<Value>,
    state: &str,
) -> Result<InstallStatus, StatusError> {
    let current = read_status(journal, provider)?;
    let mut next = current;
    if is_in_flight(&next.install_state) {
        next = transition(
            next,
            "failed",
            Some("install_interrupted".to_owned()),
            Some("install_interrupted".to_owned()),
        )?;
        next = write_status(journal, next)?;
    }
    next.target_fingerprint_json = Some(fingerprint_json);
    next.target_fingerprint_sha256 = Some(fingerprint_sha256);
    next.owner = owner;
    next.attempt_id = Some(new_attempt_id());
    next = transition(next, state, None, None)?;
    write_status(journal, next)
}

pub fn bump_progress(
    mut status: InstallStatus,
    received: Option<u64>,
    total: Option<u64>,
    last_write: &mut Instant,
) -> Result<Option<InstallStatus>, StatusError> {
    if !is_in_flight(&status.install_state) {
        return Err(StatusError::Conflict(
            "install progress can only be bumped for in-flight states".to_owned(),
        ));
    }
    let total_changed = total.is_some_and(|value| Some(value) != status.progress_bytes_total);
    if received.is_some() {
        status.progress_bytes_received = received;
    }
    if total.is_some() {
        status.progress_bytes_total = total;
    }
    status.last_progress_at = Some(now());
    if total_changed || last_write.elapsed() >= PROGRESS_COALESCE_SECONDS {
        *last_write = Instant::now();
        return Ok(Some(status));
    }
    Ok(None)
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}
fn new_attempt_id() -> String {
    format!(
        "{:016x}{:016x}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default() as u64,
        ATTEMPT_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
