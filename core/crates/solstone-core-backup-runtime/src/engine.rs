// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Backup, prune, verification, and archive operations.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_backup::{
    BackupKeys, Destination, HostedBinding, assemble_backend_env, format_recovery_key_display,
    get_backup_config, record_backup_result, record_prune_result, record_restore_result,
    record_verification_result,
};

use crate::hosted_runtime::{
    HttpTransport, RuntimeResolution, fetch_hosted_credentials, hosted_append_only_session,
    hosted_session, resolve_runtime,
};
use crate::runner::{ToolRunner, reason_for_returncode, run_restic, select_summary};

pub const ARCHIVE_TAG: &str = "solstone-archive";
pub const ARCHIVE_BACKUP_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;
pub const ARCHIVE_LS_TIMEOUT_SECONDS: u64 = 30 * 60;
pub const ARCHIVE_RETRY_LOCK: &str = "30m";
pub const BACKUP_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;
pub const INITIAL_BACKUP_TIMEOUT_SECONDS: u64 = 48 * 60 * 60;
pub const PRUNE_TIMEOUT_SECONDS: u64 = 2 * 60 * 60;
pub const PRUNE_MAX_REPACK_SIZE: &str = "1G";
pub const UNLOCK_TIMEOUT_SECONDS: u64 = 5 * 60;
pub const VERIFY_TIMEOUT_SECONDS: u64 = 60 * 60;

// Restic matches a no-slash pattern by basename at ANY depth, so bare `health`
// was removed because it dropped the durable deletion audit (retention.log,
// pruning-runs/) and per-day talent-provenance/ from every snapshot.
pub const BACKUP_EXCLUDES: [&str; 20] = [
    "*.sqlite*",
    "indexer",
    "cache",
    ".cache",
    ".removing_*",
    "*.sock",
    "*.pid",
    "*.port",
    "*.lock",
    "*.tmp",
    ".tmp*",
    "brain.json",
    "brain.log",
    "brain-fingerprint.key",
    "brain-refresh.lease",
    "supervisor.ready",
    "supervisor.start_time",
    "parakeet-cpp.placement",
    "scheduler.json",
    "health/sync",
];

/// Clock boundary used for state records and the rotating verification bucket.
pub trait Clock {
    fn now_unix(&self) -> i64;
    fn iso_week(&self) -> u8;
}

/// Write boundary for whole-journal restore attempts.
pub trait RestoreRecorder {
    #[allow(clippy::too_many_arguments)] // Mirrors backup's owner-owned record API.
    fn record(
        &self,
        journal: &Path,
        status: &str,
        time: Value,
        reason: Value,
        scope: &str,
        day: Value,
        segments_selected: Value,
        segments_restored: Value,
        files_expected: Value,
        files_restored: Value,
        bytes_expected: Value,
        bytes_restored: Value,
    ) -> Result<(), solstone_core_backup::BackupError>;
}

/// Production restore-attempt writer. Backup remains the sole state owner.
pub struct NativeRestoreRecorder;
impl RestoreRecorder for NativeRestoreRecorder {
    fn record(
        &self,
        journal: &Path,
        status: &str,
        time: Value,
        reason: Value,
        scope: &str,
        day: Value,
        segments_selected: Value,
        segments_restored: Value,
        files_expected: Value,
        files_restored: Value,
        bytes_expected: Value,
        bytes_restored: Value,
    ) -> Result<(), solstone_core_backup::BackupError> {
        record_restore_result(
            journal,
            status,
            time,
            reason,
            scope,
            day,
            segments_selected,
            segments_restored,
            files_expected,
            files_restored,
            bytes_expected,
            bytes_restored,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JournalMaintenanceError;
impl fmt::Display for JournalMaintenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("journal maintenance failed")
    }
}
impl std::error::Error for JournalMaintenanceError {}

/// Body-history rebuild and full journal scan boundary used after a restore.
pub trait JournalMaintenance {
    fn rebuild_body_history(&self, journal: &Path) -> Result<(), JournalMaintenanceError>;
    fn full_scan(&self, journal: &Path) -> Result<(), JournalMaintenanceError>;
}

/// Production implementation of post-restore journal maintenance.
pub struct NativeJournalMaintenance;
impl JournalMaintenance for NativeJournalMaintenance {
    fn rebuild_body_history(&self, journal: &Path) -> Result<(), JournalMaintenanceError> {
        solstone_core_body_rebuild::rebuild_body_store(journal)
            .map(|_| ())
            .map_err(|_| JournalMaintenanceError)
    }
    fn full_scan(&self, journal: &Path) -> Result<(), JournalMaintenanceError> {
        solstone_core_indexer_store::scan::scan_journal(journal, true)
            .map(|_| ())
            .map_err(|_| JournalMaintenanceError)
    }
}

/// Explicit dependencies for native backup runtime operations.
pub struct BackupServices<'a> {
    pub runner: &'a dyn ToolRunner,
    pub http: &'a dyn HttpTransport,
    pub clock: &'a dyn Clock,
    /// A ready pinned restic binary; readiness/install remains phase 1 authority.
    pub restic_path: Option<&'a Path>,
    /// Required only for operated append-only backup/archive sessions.
    pub rclone_path: Option<&'a Path>,
    pub version: &'a str,
    pub journal_maintenance: &'a dyn JournalMaintenance,
}

impl BackupServices<'_> {
    pub fn restic_path(&self) -> Result<&Path, String> {
        self.restic_path
            .ok_or_else(|| "restic_unavailable".to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupResult {
    pub status: String,
    pub snapshot_id: Option<String>,
    pub error_reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneResult {
    pub status: String,
    pub error_reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationResult {
    pub status: String,
    pub reason: Option<String>,
    pub checked_subset: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveFileVerdict {
    pub path: String,
    pub confirmed: bool,
    pub expected_size: u64,
    pub observed_size: Option<u64>,
    pub snapshot_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveCheckResult {
    pub status: String,
    pub error_reason: Option<String>,
    pub verdicts: Option<Vec<ArchiveFileVerdict>>,
}

struct Runtime {
    destination: Destination,
    password: String,
    backend_env: BTreeMap<String, Option<String>>,
    global_options: Vec<String>,
}

struct AdmittedBackupRun {
    resolved_journal: PathBuf,
    timeout_seconds: u64,
    keys: BackupKeys,
    mode: AdmittedBackupMode,
}

enum AdmittedBackupMode {
    Byo { destination: Destination },
    Operated { binding: HostedBinding },
}

enum BackupAdmissionTerminal {
    Skip,
    Unresolved,
    Error {
        record_journal: PathBuf,
        reason: &'static str,
    },
}

fn value_env(destination: &Destination) -> Option<BTreeMap<String, Option<String>>> {
    assemble_backend_env(destination).ok().map(|env| {
        env.into_iter()
            .map(|(key, value)| (key, value.as_str().map(str::to_owned)))
            .collect()
    })
}

fn runtime(
    journal: &Path,
    services: &BackupServices<'_>,
    scope: &str,
    append_only: bool,
) -> Result<Option<Runtime>, String> {
    match resolve_runtime(
        services.http,
        journal,
        if scope == "backup" { "operated" } else { scope },
        services.version,
    ) {
        Ok(RuntimeResolution::Skip) => Ok(None),
        Ok(RuntimeResolution::Byo { destination, keys }) => value_env(&destination)
            .map(|backend_env| {
                Some(Runtime {
                    destination,
                    password: keys.daily_key,
                    backend_env,
                    global_options: vec![],
                })
            })
            .ok_or_else(|| "failed".into()),
        Ok(RuntimeResolution::Operated {
            binding,
            credentials,
        }) => {
            let session = if append_only {
                let rclone = services
                    .rclone_path
                    .ok_or_else(|| "rclone_unavailable".to_owned())?;
                hosted_append_only_session(&binding, &credentials, rclone)
                    .map_err(|_| "rclone_unavailable")?
            } else {
                hosted_session(&binding, &credentials).map_err(|_| "failed")?
            };
            Ok(Some(Runtime {
                destination: session.destination,
                password: resolve_keys(journal)?,
                backend_env: session
                    .backend_env
                    .into_iter()
                    .map(|(key, value)| (key, Some(value)))
                    .collect(),
                global_options: session.global_options,
            }))
        }
        Err(error) => Err(error.reason_code.to_owned()),
    }
}

fn resolve_keys(journal: &Path) -> Result<String, String> {
    solstone_core_backup::get_keys(journal)
        .ok()
        .flatten()
        .map(|keys| keys.daily_key)
        .ok_or_else(|| "failed".into())
}

fn restic(
    services: &BackupServices<'_>,
    runtime: &Runtime,
    mut args: Vec<String>,
    json_output: bool,
    timeout: u64,
    max_repack_size: Option<&str>,
) -> Result<crate::runner::ResticResult, String> {
    let restic_path = services.restic_path()?;
    let mut full = Vec::new();
    full.append(&mut runtime.global_options.clone());
    full.append(&mut args);
    run_restic(
        services.runner,
        &full,
        &runtime.destination.repository,
        &runtime.password,
        restic_path,
        Some(&runtime.backend_env),
        json_output,
        max_repack_size,
        Some(Duration::from_secs(timeout)),
        &[],
    )
    .map_err(|_| "failed".into())
}

fn unlock(services: &BackupServices<'_>, runtime: &Runtime) {
    let _ = restic(
        services,
        runtime,
        vec!["unlock".into()],
        false,
        UNLOCK_TIMEOUT_SECONDS,
        None,
    );
}
fn backup_args(resolved_journal: &Path) -> Vec<String> {
    let mut args = vec!["backup".into(), resolved_journal.display().to_string()];
    for excluded in BACKUP_EXCLUDES {
        args.extend(["--exclude".into(), excluded.into()]);
    }
    args.extend([
        "--exclude".into(),
        resolved_journal.join("mcp-endpoint").display().to_string(),
    ]);
    args
}

fn resolve_backup_journal(journal: &Path) -> std::io::Result<PathBuf> {
    #[cfg(any(test, feature = "test-hooks"))]
    BACKUP_PATH_RESOLUTION_ATTEMPTS.with(|attempts| attempts.set(attempts.get() + 1));
    let resolved = fs::canonicalize(journal)?;
    if !fs::metadata(&resolved)?.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "backup journal must be a directory",
        ));
    }
    Ok(resolved)
}

fn keys_from_backup_config(config: &Map<String, Value>) -> Result<Option<BackupKeys>, ()> {
    match (config.get("daily_key"), config.get("recovery_key")) {
        (Some(Value::Null) | None, _) | (_, Some(Value::Null) | None) => Ok(None),
        (Some(Value::String(daily_key)), Some(Value::String(recovery_key))) => {
            format_recovery_key_display(recovery_key).map_err(|_| ())?;
            Ok(Some(BackupKeys {
                daily_key: daily_key.clone(),
                recovery_key: recovery_key.clone(),
            }))
        }
        _ => Err(()),
    }
}

fn destination_from_backup_config(config: &Map<String, Value>) -> Option<Destination> {
    let destination = config.get("destination")?.as_object()?;
    let repository = destination.get("repository")?.as_str()?;
    let backend = destination.get("backend")?.as_str()?;
    let credentials = match destination.get("credentials") {
        None => Map::new(),
        Some(Value::Object(credentials)) => credentials.clone(),
        Some(_) => return None,
    };
    Some(Destination {
        repository: repository.to_owned(),
        backend: backend.to_owned(),
        credentials,
    })
}

fn backup_timeout_seconds(config: &Map<String, Value>) -> u64 {
    config
        .get("last_backup")
        .and_then(Value::as_object)
        .filter(|last| {
            last.get("status") == Some(&Value::String("ok".into()))
                && last
                    .get("snapshot_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty())
        })
        .map_or(INITIAL_BACKUP_TIMEOUT_SECONDS, |_| BACKUP_TIMEOUT_SECONDS)
}

fn prepare_backup_run(journal: &Path) -> Result<AdmittedBackupRun, BackupAdmissionTerminal> {
    let resolved_journal =
        resolve_backup_journal(journal).map_err(|_| BackupAdmissionTerminal::Unresolved)?;
    #[cfg(test)]
    run_on_backup_journal_resolved_hook();

    let config =
        get_backup_config(&resolved_journal).map_err(|_| BackupAdmissionTerminal::Error {
            record_journal: resolved_journal.clone(),
            reason: "broker_error",
        })?;
    if config.get("enabled") != Some(&Value::Bool(true)) {
        return Err(BackupAdmissionTerminal::Skip);
    }
    let Some(keys) =
        keys_from_backup_config(&config).map_err(|_| BackupAdmissionTerminal::Error {
            record_journal: resolved_journal.clone(),
            reason: "broker_error",
        })?
    else {
        return Err(BackupAdmissionTerminal::Skip);
    };
    let mode = if config.get("mode") == Some(&Value::String("operated".into())) {
        let Some(binding) = solstone_core_backup::load_hosted_binding(&resolved_journal) else {
            return Err(BackupAdmissionTerminal::Skip);
        };
        AdmittedBackupMode::Operated { binding }
    } else {
        let Some(destination) = destination_from_backup_config(&config) else {
            return Err(BackupAdmissionTerminal::Skip);
        };
        AdmittedBackupMode::Byo { destination }
    };
    Ok(AdmittedBackupRun {
        resolved_journal,
        timeout_seconds: backup_timeout_seconds(&config),
        keys,
        mode,
    })
}

#[cfg(test)]
thread_local! {
    static ON_BACKUP_JOURNAL_RESOLVED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static BACKUP_PATH_RESOLUTION_ATTEMPTS: std::cell::Cell<u32> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(test)]
fn run_on_backup_journal_resolved_hook() {
    let hook = ON_BACKUP_JOURNAL_RESOLVED.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(test)]
fn run_with_backup_journal_resolved_hook<T>(
    hook: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> T {
    ON_BACKUP_JOURNAL_RESOLVED.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "backup journal resolved hook is already active"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
    let result = op();
    ON_BACKUP_JOURNAL_RESOLVED.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "backup journal resolved hook was not reached"
        );
    });
    result
}

/// Test-only instrumentation: reset the per-thread `resolve_backup_journal` attempt count.
#[cfg(any(test, feature = "test-hooks"))]
pub fn reset_backup_path_resolution_attempts() {
    BACKUP_PATH_RESOLUTION_ATTEMPTS.with(|attempts| attempts.set(0));
}

/// Test-only instrumentation: read the per-thread `resolve_backup_journal` attempt count.
#[cfg(any(test, feature = "test-hooks"))]
pub fn backup_path_resolution_attempts() -> u32 {
    BACKUP_PATH_RESOLUTION_ATTEMPTS.with(std::cell::Cell::get)
}
fn snapshot_id(value: Option<&Value>) -> Option<String> {
    select_summary(value?)
        .and_then(|summary| summary.get("snapshot_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}
fn record_backup(journal: &Path, clock: &dyn Clock, result: &BackupResult) {
    let _ = record_backup_result(
        journal,
        &result.status,
        json!(clock.now_unix()),
        result
            .snapshot_id
            .clone()
            .map_or(Value::Null, Value::String),
        result
            .error_reason
            .clone()
            .map_or(Value::Null, Value::String),
    );
}

/// Persist a backup-run failure that happened before `run_backup` was invoked.
pub fn record_backup_error(journal: &Path, clock: &dyn Clock, reason: &str) -> BackupResult {
    let result = BackupResult {
        status: "error".into(),
        snapshot_id: None,
        error_reason: Some(reason.to_owned()),
    };
    record_backup(journal, clock, &result);
    result
}

fn record_prune(journal: &Path, clock: &dyn Clock, result: &PruneResult) {
    let _ = record_prune_result(
        journal,
        &result.status,
        json!(clock.now_unix()),
        result
            .error_reason
            .clone()
            .map_or(Value::Null, Value::String),
    );
}
fn record_verify(journal: &Path, clock: &dyn Clock, result: &VerificationResult) {
    let _ = record_verification_result(
        journal,
        &result.status,
        json!(clock.now_unix()),
        result.reason.clone().map_or(Value::Null, Value::String),
        result
            .checked_subset
            .clone()
            .map_or(Value::Null, Value::String),
    );
}

fn consume_admitted_backup_run(
    admitted: AdmittedBackupRun,
    services: &BackupServices<'_>,
) -> BackupResult {
    let AdmittedBackupRun {
        resolved_journal,
        timeout_seconds,
        keys,
        mode,
    } = admitted;
    let runtime = match mode {
        AdmittedBackupMode::Byo { destination } => value_env(&destination)
            .map(|backend_env| Runtime {
                destination,
                password: keys.daily_key,
                backend_env,
                global_options: vec![],
            })
            .ok_or_else(|| "failed".to_owned()),
        AdmittedBackupMode::Operated { binding } => (|| {
            let credentials =
                fetch_hosted_credentials(services.http, &binding, "operated", services.version)
                    .map_err(|error| error.reason_code.to_owned())?;
            let rclone = services
                .rclone_path
                .ok_or_else(|| "rclone_unavailable".to_owned())?;
            let session = hosted_append_only_session(&binding, &credentials, rclone)
                .map_err(|_| "rclone_unavailable")?;
            Ok(Runtime {
                destination: session.destination,
                password: keys.daily_key,
                backend_env: session
                    .backend_env
                    .into_iter()
                    .map(|(key, value)| (key, Some(value)))
                    .collect(),
                global_options: session.global_options,
            })
        })(),
    };
    let result = match runtime {
        Ok(runtime) => {
            unlock(services, &runtime);
            match restic(
                services,
                &runtime,
                backup_args(&resolved_journal),
                true,
                timeout_seconds,
                None,
            ) {
                Ok(output) => {
                    let id = snapshot_id(output.json.as_ref());
                    if output.returncode == 0 && id.is_some() {
                        BackupResult {
                            status: "ok".into(),
                            snapshot_id: id,
                            error_reason: None,
                        }
                    } else {
                        BackupResult {
                            status: "error".into(),
                            snapshot_id: if output.returncode == 3 { id } else { None },
                            error_reason: Some(if output.returncode == 0 {
                                "unknown".into()
                            } else {
                                reason_for_returncode(output.returncode).into()
                            }),
                        }
                    }
                }
                Err(reason) => BackupResult {
                    status: "error".into(),
                    snapshot_id: None,
                    error_reason: Some(reason),
                },
            }
        }
        Err(reason) => BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some(reason),
        },
    };
    record_backup(&resolved_journal, services.clock, &result);
    result
}

pub fn run_backup(journal: &Path, services: &BackupServices<'_>) -> BackupResult {
    match prepare_backup_run(journal) {
        Ok(admitted) => consume_admitted_backup_run(admitted, services),
        Err(BackupAdmissionTerminal::Skip) => BackupResult {
            status: "skipped".into(),
            snapshot_id: None,
            error_reason: None,
        },
        Err(BackupAdmissionTerminal::Unresolved) => BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some("journal_path_unresolved".into()),
        },
        Err(BackupAdmissionTerminal::Error {
            record_journal,
            reason,
        }) => record_backup_error(&record_journal, services.clock, reason),
    }
}

pub fn run_prune(journal: &Path, services: &BackupServices<'_>) -> PruneResult {
    let runtime = match runtime(journal, services, "maintenance", false) {
        Ok(None) => {
            return PruneResult {
                status: "skipped".into(),
                error_reason: None,
            };
        }
        Ok(Some(runtime)) => runtime,
        Err(reason) => {
            let result = PruneResult {
                status: "error".into(),
                error_reason: Some(reason),
            };
            record_prune(journal, services.clock, &result);
            return result;
        }
    };
    unlock(services, &runtime);
    let retention = get_backup_config(journal)
        .ok()
        .and_then(|config| config.get("retention").and_then(Value::as_object).cloned())
        .unwrap_or_default();
    let args = vec![
        "forget".into(),
        "--keep-hourly".into(),
        retention
            .get("hourly")
            .and_then(Value::as_u64)
            .unwrap_or(24)
            .to_string(),
        "--keep-daily".into(),
        retention
            .get("daily")
            .and_then(Value::as_u64)
            .unwrap_or(7)
            .to_string(),
        "--keep-weekly".into(),
        retention
            .get("weekly")
            .and_then(Value::as_u64)
            .unwrap_or(4)
            .to_string(),
        "--keep-monthly".into(),
        retention
            .get("monthly")
            .and_then(Value::as_u64)
            .unwrap_or(12)
            .to_string(),
        "--keep-tag".into(),
        ARCHIVE_TAG.into(),
        "--prune".into(),
    ];
    let result = match restic(
        services,
        &runtime,
        args,
        false,
        PRUNE_TIMEOUT_SECONDS,
        Some(PRUNE_MAX_REPACK_SIZE),
    ) {
        Ok(output) if output.returncode == 0 => PruneResult {
            status: "ok".into(),
            error_reason: None,
        },
        Ok(output) => PruneResult {
            status: "error".into(),
            error_reason: Some(reason_for_returncode(output.returncode).into()),
        },
        Err(reason) => PruneResult {
            status: "error".into(),
            error_reason: Some(reason),
        },
    };
    record_prune(journal, services.clock, &result);
    result
}

pub fn verification_subset_for_week(week: u8) -> String {
    format!("{}/52", ((week.saturating_sub(1) % 52) + 1))
}
fn verify_reason(code: i32) -> String {
    if code == 1 {
        "integrity_failed".into()
    } else if code == 3 {
        "failed".into()
    } else {
        reason_for_returncode(code).into()
    }
}
pub fn run_verification(
    journal: &Path,
    services: &BackupServices<'_>,
    clock: &dyn Clock,
) -> VerificationResult {
    let runtime = match runtime(journal, services, "backup", false) {
        Ok(None) => {
            return VerificationResult {
                status: "skipped".into(),
                reason: None,
                checked_subset: None,
            };
        }
        Ok(Some(runtime)) => runtime,
        Err(reason) => {
            let result = VerificationResult {
                status: "error".into(),
                reason: Some(reason),
                checked_subset: None,
            };
            record_verify(journal, clock, &result);
            return result;
        }
    };
    let subset = verification_subset_for_week(clock.iso_week());
    let result = match restic(
        services,
        &runtime,
        vec!["check".into(), "--read-data-subset".into(), subset.clone()],
        false,
        VERIFY_TIMEOUT_SECONDS,
        None,
    ) {
        Ok(output) if output.returncode == 0 => VerificationResult {
            status: "ok".into(),
            reason: None,
            checked_subset: Some(subset),
        },
        Ok(output) => VerificationResult {
            status: "error".into(),
            reason: Some(verify_reason(output.returncode)),
            checked_subset: None,
        },
        Err(reason) => VerificationResult {
            status: "error".into(),
            reason: Some(reason),
            checked_subset: None,
        },
    };
    record_verify(journal, clock, &result);
    result
}

pub fn run_archive_backup(
    journal: &Path,
    services: &BackupServices<'_>,
    paths: &[PathBuf],
) -> BackupResult {
    let runtime = match runtime(journal, services, "backup", true) {
        Ok(None) => {
            return BackupResult {
                status: "skipped".into(),
                snapshot_id: None,
                error_reason: None,
            };
        }
        Ok(Some(runtime)) => runtime,
        Err(reason) => {
            return BackupResult {
                status: "error".into(),
                snapshot_id: None,
                error_reason: Some(reason),
            };
        }
    };
    unlock(services, &runtime);
    let mut args = vec![
        "--retry-lock".into(),
        ARCHIVE_RETRY_LOCK.into(),
        "backup".into(),
    ];
    args.extend(paths.iter().map(|path| path.display().to_string()));
    args.extend(["--tag".into(), ARCHIVE_TAG.into()]);
    match restic(
        services,
        &runtime,
        args,
        true,
        ARCHIVE_BACKUP_TIMEOUT_SECONDS,
        None,
    ) {
        Ok(output) if output.returncode == 0 => snapshot_id(output.json.as_ref()).map_or(
            BackupResult {
                status: "error".into(),
                snapshot_id: None,
                error_reason: Some("unknown".into()),
            },
            |id| BackupResult {
                status: "ok".into(),
                snapshot_id: Some(id),
                error_reason: None,
            },
        ),
        Ok(output) => BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some(reason_for_returncode(output.returncode).into()),
        },
        Err(reason) => BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some(reason),
        },
    }
}

pub fn check_archive_snapshot_files(
    journal: &Path,
    services: &BackupServices<'_>,
    snapshot: &str,
    expected: &BTreeMap<PathBuf, u64>,
) -> ArchiveCheckResult {
    let runtime = match runtime(journal, services, "backup", false) {
        Ok(None) => {
            return ArchiveCheckResult {
                status: "skipped".into(),
                error_reason: None,
                verdicts: None,
            };
        }
        Ok(Some(runtime)) => runtime,
        Err(reason) => {
            return ArchiveCheckResult {
                status: "error".into(),
                error_reason: Some(reason),
                verdicts: None,
            };
        }
    };
    let output = match restic(
        services,
        &runtime,
        vec!["ls".into(), "--long".into(), snapshot.into()],
        true,
        ARCHIVE_LS_TIMEOUT_SECONDS,
        None,
    ) {
        Ok(output) => output,
        Err(reason) => {
            return ArchiveCheckResult {
                status: "error".into(),
                error_reason: Some(reason),
                verdicts: None,
            };
        }
    };
    if output.returncode != 0 {
        return ArchiveCheckResult {
            status: "error".into(),
            error_reason: Some(reason_for_returncode(output.returncode).into()),
            verdicts: None,
        };
    }
    let Some(Value::Array(records)) = output.json else {
        return ArchiveCheckResult {
            status: "error".into(),
            error_reason: Some("failed".into()),
            verdicts: None,
        };
    };
    let id = records
        .iter()
        .find(|record| record.get("message_type") == Some(&Value::String("snapshot".into())))
        .and_then(|record| record.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    let Some(id) = id else {
        return ArchiveCheckResult {
            status: "error".into(),
            error_reason: Some("failed".into()),
            verdicts: None,
        };
    };
    let observed: BTreeMap<_, _> = records
        .iter()
        .filter(|record| record.get("message_type") == Some(&Value::String("node".into())))
        .filter_map(|record| {
            Some((
                record.get("path")?.as_str()?.to_owned(),
                record.get("size")?.as_u64()?,
            ))
        })
        .collect();
    ArchiveCheckResult {
        status: "ok".into(),
        error_reason: None,
        verdicts: Some(
            expected
                .iter()
                .map(|(path, size)| {
                    let got = observed.get(&path.display().to_string()).copied();
                    ArchiveFileVerdict {
                        path: path.display().to_string(),
                        confirmed: got == Some(*size),
                        expected_size: *size,
                        observed_size: got,
                        snapshot_id: id.into(),
                    }
                })
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosted_runtime::{HttpError, HttpRequest, HttpResponse};
    use crate::runner::{ToolOutput, ToolRequest};
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, VecDeque};
    use std::io;
    use std::rc::Rc;
    use std::sync::{LazyLock, Mutex};
    use std::time::Duration;

    static CURRENT_DIRECTORY: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct CurrentDirectoryGuard(PathBuf);

    impl CurrentDirectoryGuard {
        fn change_to(path: &Path) -> Self {
            let original = std::env::current_dir().expect("working directory reads");
            std::env::set_current_dir(path).expect("working directory sets");
            Self(original)
        }
    }

    impl Drop for CurrentDirectoryGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    struct Script {
        outputs: RefCell<VecDeque<ToolOutput>>,
        commands: RefCell<Vec<Vec<String>>>,
    }
    impl ToolRunner for Script {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.commands.borrow_mut().push(
                request
                    .argv
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect(),
            );
            Ok(self.outputs.borrow_mut().pop_front().expect("output"))
        }
    }
    struct ObservedScript {
        outputs: RefCell<VecDeque<ToolOutput>>,
        requests: RefCell<Vec<ObservedRequest>>,
        after_unlock: RefCell<Option<Box<dyn FnOnce()>>>,
    }
    struct ObservedRequest {
        argv: Vec<String>,
        env: BTreeMap<String, String>,
        timeout: Option<Duration>,
    }
    impl ToolRunner for ObservedScript {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            let is_unlock = request.argv.iter().any(|argument| argument == "unlock");
            self.requests.borrow_mut().push(ObservedRequest {
                argv: request
                    .argv
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect(),
                env: request
                    .env
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.to_string_lossy().into_owned(),
                            value.to_string_lossy().into_owned(),
                        )
                    })
                    .collect(),
                timeout: request.timeout,
            });
            if is_unlock && let Some(hook) = self.after_unlock.borrow_mut().take() {
                hook();
            }
            Ok(self.outputs.borrow_mut().pop_front().expect("output"))
        }
    }
    struct Http;
    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("BYO must not fetch broker credentials")
        }
    }
    struct PanicHttp;
    impl HttpTransport for PanicHttp {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("HTTP must not be reached")
        }
    }
    struct PanicRunner;
    impl ToolRunner for PanicRunner {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            panic!("runner must not be reached")
        }
    }
    struct ScriptedHttp {
        responses: RefCell<VecDeque<Result<HttpResponse, HttpError>>>,
        requests: RefCell<Vec<HttpRequest>>,
    }
    impl HttpTransport for ScriptedHttp {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.requests.borrow_mut().push(request.clone());
            self.responses.borrow_mut().pop_front().expect("response")
        }
    }
    struct FixedClock;
    impl Clock for FixedClock {
        fn now_unix(&self) -> i64 {
            50
        }
        fn iso_week(&self) -> u8 {
            7
        }
    }
    struct MutableClock(Rc<Cell<i64>>);
    impl Clock for MutableClock {
        fn now_unix(&self) -> i64 {
            self.0.get()
        }
        fn iso_week(&self) -> u8 {
            7
        }
    }
    struct Maintenance;
    impl JournalMaintenance for Maintenance {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }
        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }
    }
    fn output(code: i32, stdout: &str) -> ToolOutput {
        ToolOutput {
            returncode: code,
            stdout: stdout.as_bytes().to_vec(),
            stderr: vec![],
        }
    }
    fn json_backup_args(resolved_journal: &Path) -> Vec<String> {
        let mut args = backup_args(resolved_journal);
        args.push("--json".into());
        args
    }
    fn configure_journal(journal: &Path) {
        let destination = Destination {
            repository: "s3:repo".into(),
            backend: "s3".into(),
            credentials: serde_json::json!({"access_key_id":"access","secret_access_key":"secret"})
                .as_object()
                .unwrap()
                .clone(),
        };
        solstone_core_backup::set_destination(journal, &destination).unwrap();
        solstone_core_backup::generate_and_store_keys(journal).unwrap();
        solstone_core_backup::set_enabled(journal, true).unwrap();
    }
    fn configured_journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        configure_journal(journal.path());
        journal
    }
    fn services<'a>(
        runner: &'a dyn ToolRunner,
        http: &'a dyn HttpTransport,
        clock: &'a dyn Clock,
        maintenance: &'a Maintenance,
    ) -> BackupServices<'a> {
        BackupServices {
            runner,
            http,
            clock,
            restic_path: Some(Path::new("/fixture/bin/restic")),
            rclone_path: None,
            version: "test",
            journal_maintenance: maintenance,
        }
    }
    fn valid_backup_section(destination: Value) -> Value {
        json!({
            "enabled": true,
            "mode": "byo",
            "daily_key": "daily",
            "recovery_key": "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ",
            "destination": destination,
        })
    }
    fn valid_destination() -> Value {
        json!({
            "repository": "s3:repo",
            "backend": "s3",
            "credentials": {"access_key_id": "access", "secret_access_key": "secret"},
        })
    }
    fn write_backup_section(journal: &Path, backup: Value) {
        let path = journal.join("config/journal.json");
        fs::create_dir_all(path.parent().expect("config parent")).expect("config parent creates");
        fs::write(
            path,
            serde_json::to_vec(&json!({"backup": backup})).expect("config encodes"),
        )
        .expect("config writes");
    }
    fn hosted_binding(endpoint: &str) -> HostedBinding {
        HostedBinding {
            broker_endpoint: endpoint.into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "prefix".into(),
            broker_token: "token".into(),
        }
    }
    fn credentials_response() -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&serde_json::json!({
                "access_key_id": "access",
                "secret_access_key": "secret",
                "session_token": "session",
                "endpoint": "https://s3.example",
                "expires_at": "later",
            }))
            .expect("credentials encode"),
        }
    }
    fn assert_one_resolution_attempt() {
        assert_eq!(backup_path_resolution_attempts(), 1);
    }
    #[test]
    fn excludes_are_exact_and_keep_durable_health() {
        assert_eq!(
            BACKUP_EXCLUDES,
            [
                "*.sqlite*",
                "indexer",
                "cache",
                ".cache",
                ".removing_*",
                "*.sock",
                "*.pid",
                "*.port",
                "*.lock",
                "*.tmp",
                ".tmp*",
                "brain.json",
                "brain.log",
                "brain-fingerprint.key",
                "brain-refresh.lease",
                "supervisor.ready",
                "supervisor.start_time",
                "parakeet-cpp.placement",
                "scheduler.json",
                "health/sync"
            ]
        );
        assert!(!BACKUP_EXCLUDES.contains(&"health"));
    }
    #[test]
    fn verification_bucket_wraps_reference_weeks() {
        assert_eq!(verification_subset_for_week(1), "1/52");
        assert_eq!(verification_subset_for_week(52), "52/52");
        assert_eq!(verification_subset_for_week(53), "1/52");
    }
    #[test]
    fn backup_unlocks_unconditionally_then_records_snapshot_success() {
        let journal = configured_journal();
        reset_backup_path_resolution_attempts();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(11, ""),
                output(0, "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}"),
            ])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = FixedClock;
        let maintenance = Maintenance;
        assert_eq!(
            run_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance)
            )
            .status,
            "ok"
        );
        let commands = runner.commands.borrow();
        assert_eq!(commands[0][0], "unlock");
        assert_eq!(commands[1][0], "backup");
        assert!(
            commands[1]
                .windows(2)
                .any(|pair| pair == ["--exclude", ".removing_*"])
        );
        assert!(
            commands[1]
                .windows(2)
                .any(|pair| pair == ["--exclude", "health/sync"])
        );
        assert!(
            !commands[1]
                .windows(2)
                .any(|pair| pair == ["--exclude", "health"])
        );
        assert_one_resolution_attempt();
    }
    #[test]
    fn backup_argv_uses_one_resolved_path_for_source_and_endpoint_exclusion() {
        let journal = configured_journal();
        reset_backup_path_resolution_attempts();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(11, ""),
                output(0, "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}"),
            ])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = FixedClock;
        let maintenance = Maintenance;

        assert_eq!(
            run_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance)
            )
            .status,
            "ok"
        );

        let resolved = fs::canonicalize(journal.path()).expect("configured journal resolves");
        let commands = runner.commands.borrow();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], ["unlock"]);
        assert_eq!(commands[1], json_backup_args(&resolved));
        assert_eq!(commands[1][1], resolved.display().to_string());
        assert_eq!(
            commands[1]
                .windows(2)
                .find(|pair| pair[0] == "--exclude" && pair[1].ends_with("/mcp-endpoint")),
            Some(
                [
                    "--exclude".to_owned(),
                    resolved.join("mcp-endpoint").display().to_string()
                ]
                .as_slice()
            )
        );
        assert!(
            !commands[1]
                .windows(2)
                .any(|pair| pair == ["--exclude", "mcp-endpoint"]),
            "only the top-level absolute endpoint path may be excluded"
        );
        assert_one_resolution_attempt();
    }

    #[cfg(unix)]
    #[test]
    fn backup_argv_keeps_a_symlink_operand_bound_to_its_original_resolution() {
        use std::os::unix::fs::symlink;

        let source = configured_journal();
        let sandbox = tempfile::tempdir().expect("test sandbox creates");
        let link = sandbox.path().join("journal-link");
        symlink(source.path(), &link).expect("journal link creates");
        let replacement = tempfile::tempdir().expect("replacement journal creates");
        let replacement_path = replacement.path().to_path_buf();
        let callback_link = link.clone();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(11, ""),
                output(0, "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}"),
            ])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = FixedClock;
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let result = run_with_backup_journal_resolved_hook(
            move || {
                fs::remove_file(&callback_link).expect("original journal link removes");
                symlink(&replacement_path, &callback_link).expect("journal link retargets");
            },
            || run_backup(&link, &services(&runner, &http, &clock, &maintenance)),
        );

        assert_eq!(result.status, "ok");
        let original = fs::canonicalize(source.path()).expect("source journal resolves");
        assert_eq!(runner.commands.borrow()[1], json_backup_args(&original));
        assert_one_resolution_attempt();
    }

    #[cfg(unix)]
    #[test]
    fn backup_argv_keeps_a_relative_operand_bound_after_the_working_directory_changes() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        let sandbox = tempfile::tempdir().expect("test sandbox creates");
        let journal = sandbox.path().join("journal");
        fs::create_dir(&journal).expect("journal directory creates");
        configure_journal(&journal);
        let other = sandbox.path().join("other");
        fs::create_dir(&other).expect("other directory creates");
        let _directory = CurrentDirectoryGuard::change_to(sandbox.path());
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(11, ""),
                output(0, "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}"),
            ])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = FixedClock;
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let result = run_with_backup_journal_resolved_hook(
            move || {
                std::env::set_current_dir(&other).expect("working directory changes mid-run");
            },
            || {
                run_backup(
                    Path::new("journal"),
                    &services(&runner, &http, &clock, &maintenance),
                )
            },
        );

        assert_eq!(result.status, "ok");
        let resolved = fs::canonicalize(&journal).expect("journal resolves");
        assert_eq!(runner.commands.borrow()[1], json_backup_args(&resolved));
        assert_one_resolution_attempt();
    }

    #[cfg(unix)]
    #[test]
    fn enabled_journal_path_resolution_failure_does_not_invoke_the_runner() {
        use std::os::unix::fs::symlink;

        let sandbox = tempfile::tempdir().expect("test sandbox creates");
        let link = sandbox.path().join("journal-link");
        symlink("missing-journal", &link).expect("dangling journal link creates");
        let runner = PanicRunner;
        let http = PanicHttp;
        let clock = FixedClock;
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let result = run_backup(&link, &services(&runner, &http, &clock, &maintenance));

        assert_eq!(result.status, "error");
        assert_eq!(
            result.error_reason.as_deref(),
            Some("journal_path_unresolved")
        );
        assert!(
            !sandbox.path().join("missing-journal").exists(),
            "a dangling journal path must not create its missing target"
        );
        assert_one_resolution_attempt();
    }

    #[test]
    fn disabled_backup_resolves_journal_before_skipping() {
        let journal = tempfile::tempdir().expect("test journal creates");
        let runner = PanicRunner;
        let http = PanicHttp;
        let clock = FixedClock;
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let result = run_with_backup_journal_resolved_hook(
            || {},
            || {
                run_backup(
                    journal.path(),
                    &services(&runner, &http, &clock, &maintenance),
                )
            },
        );

        assert_eq!(result.status, "skipped");
        assert_eq!(result.error_reason, None);
        assert_one_resolution_attempt();
    }

    #[test]
    fn unresolved_relative_and_absolute_journals_do_not_reach_runtime_dependencies() {
        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        let sandbox = tempfile::tempdir().expect("test sandbox creates");
        let _directory = CurrentDirectoryGuard::change_to(sandbox.path());
        let absolute_missing = sandbox.path().join("missing");
        let relative_missing = Path::new("missing-relative");
        let runner = PanicRunner;
        let http = PanicHttp;
        let clock = FixedClock;
        let maintenance = Maintenance;

        for journal in [&absolute_missing, relative_missing] {
            reset_backup_path_resolution_attempts();
            let result = run_backup(journal, &services(&runner, &http, &clock, &maintenance));
            assert_eq!(result.status, "error");
            assert_eq!(
                result.error_reason.as_deref(),
                Some("journal_path_unresolved")
            );
            assert_one_resolution_attempt();
            assert!(
                !absolute_missing.exists(),
                "an unresolved absolute journal must not gain config artifacts"
            );
        }
        assert!(
            !relative_missing.exists(),
            "an unresolved relative journal must not gain config artifacts"
        );
    }

    // Regression coverage for non-directory journal roots (regular file, FIFO, Unix
    // socket) lives in `tests/backup_runtime_process.rs` — binding a Unix socket trips
    // the routine unit harness's hard-boundary ("network") topology check, so that case
    // runs in the crate's `test-hooks`-gated integration harness instead.

    #[test]
    fn admission_skip_boundaries_do_not_reach_runtime_dependencies() {
        let mut missing_daily = valid_backup_section(valid_destination());
        missing_daily
            .as_object_mut()
            .expect("backup object")
            .remove("daily_key");
        let mut missing_binding = valid_backup_section(valid_destination());
        missing_binding["mode"] = Value::String("operated".into());
        let mut missing_destination = valid_backup_section(valid_destination());
        missing_destination
            .as_object_mut()
            .expect("backup object")
            .remove("destination");
        let cases = vec![
            ("disabled", serde_json::json!({"enabled": false})),
            ("missing_daily_key", missing_daily),
            ("missing_operated_binding", missing_binding),
            ("missing_destination", missing_destination),
            (
                "non_object_destination",
                valid_backup_section(Value::String("no".into())),
            ),
            (
                "non_string_repository",
                valid_backup_section(serde_json::json!({
                    "repository": 7,
                    "backend": "s3",
                    "credentials": {},
                })),
            ),
            (
                "non_string_backend",
                valid_backup_section(serde_json::json!({
                    "repository": "repo",
                    "backend": 7,
                    "credentials": {},
                })),
            ),
            (
                "null_credentials",
                valid_backup_section(serde_json::json!({
                    "repository": "repo",
                    "backend": "s3",
                    "credentials": null,
                })),
            ),
            (
                "scalar_credentials",
                valid_backup_section(serde_json::json!({
                    "repository": "repo",
                    "backend": "s3",
                    "credentials": "oops",
                })),
            ),
        ];

        for (name, backup) in cases {
            let journal = tempfile::tempdir().expect("test journal creates");
            write_backup_section(journal.path(), backup);
            let runner = PanicRunner;
            let http = PanicHttp;
            let clock = FixedClock;
            let maintenance = Maintenance;
            reset_backup_path_resolution_attempts();

            let result = run_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance),
            );

            assert_eq!(result.status, "skipped", "{name}");
            assert_eq!(result.error_reason, None, "{name}");
            assert_one_resolution_attempt();
        }
    }

    #[test]
    fn omitted_byo_credentials_are_admitted_then_fail_without_runner() {
        let journal = tempfile::tempdir().expect("test journal creates");
        write_backup_section(
            journal.path(),
            valid_backup_section(serde_json::json!({
                "repository": "s3:repo",
                "backend": "s3",
            })),
        );
        let runner = PanicRunner;
        let http = PanicHttp;
        let clock = FixedClock;
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let result = run_backup(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.error_reason.as_deref(), Some("failed"));
        assert_one_resolution_attempt();
        assert_eq!(
            solstone_core_backup::get_backup_config(journal.path()).expect("state reads")["last_backup"]
                ["error_reason"],
            "failed"
        );
    }

    #[test]
    fn corrupt_or_unreadable_config_returns_broker_error_without_runner() {
        let journal = tempfile::tempdir().expect("test journal creates");
        let config = journal.path().join("config/journal.json");
        fs::create_dir_all(config.parent().expect("config parent")).expect("config parent creates");
        fs::write(&config, b"{not json").expect("corrupt config writes");
        let before_bytes = fs::read(&config).expect("config reads");
        let before_metadata = fs::metadata(&config).expect("metadata reads");
        let runner = PanicRunner;
        let http = PanicHttp;
        let clock = FixedClock;
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let result = run_backup(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.error_reason.as_deref(), Some("broker_error"));
        assert_one_resolution_attempt();
        assert_eq!(fs::read(&config).expect("config rereads"), before_bytes);
        let after_metadata = fs::metadata(&config).expect("metadata rereads");
        assert_eq!(after_metadata.len(), before_metadata.len());
        assert_eq!(
            after_metadata.permissions().readonly(),
            before_metadata.permissions().readonly()
        );

        let unreadable = tempfile::tempdir().expect("unreadable journal creates");
        fs::create_dir_all(unreadable.path().join("config/journal.json"))
            .expect("config path directory creates");
        reset_backup_path_resolution_attempts();
        let result = run_backup(
            unreadable.path(),
            &services(&runner, &http, &clock, &maintenance),
        );
        assert_eq!(result.status, "error");
        assert_eq!(result.error_reason.as_deref(), Some("broker_error"));
        assert_one_resolution_attempt();
    }

    #[test]
    fn malformed_keys_return_broker_error_and_record_at_the_resolved_journal() {
        let cases = [
            (
                "daily_type",
                Value::Number(7.into()),
                Value::String(
                    "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ".into(),
                ),
            ),
            (
                "recovery_type",
                Value::String("daily".into()),
                Value::Number(7.into()),
            ),
            (
                "recovery_short",
                Value::String("daily".into()),
                Value::String("short".into()),
            ),
            (
                "recovery_invalid_character",
                Value::String("daily".into()),
                Value::String("!".repeat(64)),
            ),
        ];
        for (name, daily_key, recovery_key) in cases {
            let journal = tempfile::tempdir().expect("test journal creates");
            let mut backup = valid_backup_section(valid_destination());
            backup["daily_key"] = daily_key;
            backup["recovery_key"] = recovery_key;
            write_backup_section(journal.path(), backup);
            let runner = PanicRunner;
            let http = PanicHttp;
            let clock = FixedClock;
            let maintenance = Maintenance;
            reset_backup_path_resolution_attempts();

            let result = run_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance),
            );

            assert_eq!(result.status, "error", "{name}");
            assert_eq!(
                result.error_reason.as_deref(),
                Some("broker_error"),
                "{name}"
            );
            assert_one_resolution_attempt();
            assert_eq!(
                solstone_core_backup::get_backup_config(journal.path()).expect("state reads")["last_backup"]
                    ["error_reason"],
                "broker_error",
                "{name}"
            );
        }
    }

    #[test]
    fn valid_operated_backup_reaches_broker_and_runner() {
        let journal = tempfile::tempdir().expect("test journal creates");
        let mut backup = valid_backup_section(valid_destination());
        backup["mode"] = Value::String("operated".into());
        write_backup_section(journal.path(), backup);
        solstone_core_backup::save_hosted_binding(
            journal.path(),
            &hosted_binding("https://broker"),
        )
        .expect("binding writes");
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([
                output(11, ""),
                output(0, "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}"),
            ])),
            commands: RefCell::new(vec![]),
        };
        let http = ScriptedHttp {
            responses: RefCell::new(VecDeque::from([Ok(credentials_response())])),
            requests: RefCell::new(vec![]),
        };
        let clock = FixedClock;
        let maintenance = Maintenance;
        let mut services = services(&runner, &http, &clock, &maintenance);
        services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
        reset_backup_path_resolution_attempts();

        let result = run_backup(journal.path(), &services);

        assert_eq!(result.status, "ok");
        assert_eq!(http.requests.borrow().len(), 1);
        assert_eq!(runner.commands.borrow()[1][0], "-o");
        assert_one_resolution_attempt();
    }

    #[cfg(unix)]
    #[test]
    fn admitted_operated_run_stays_bound_to_original_alias_after_retargets() {
        use std::os::unix::fs::symlink;

        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        let source = tempfile::tempdir().expect("source journal creates");
        let mut source_backup = valid_backup_section(valid_destination());
        source_backup["mode"] = Value::String("operated".into());
        source_backup["last_backup"] = serde_json::json!({"status": "ok", "snapshot_id": "old"});
        write_backup_section(source.path(), source_backup);
        solstone_core_backup::save_hosted_binding(
            source.path(),
            &hosted_binding("https://source-broker"),
        )
        .expect("source binding writes");
        let replacement = tempfile::tempdir().expect("replacement journal creates");
        write_backup_section(replacement.path(), serde_json::json!({"enabled": false}));
        let later = tempfile::tempdir().expect("later journal creates");
        let sandbox = tempfile::tempdir().expect("test sandbox creates");
        let alias = sandbox.path().join("journal");
        symlink(source.path(), &alias).expect("alias creates");
        let replacement_path = replacement.path().to_path_buf();
        let later_path = later.path().to_path_buf();
        let hook_alias = alias.clone();
        let after_unlock_alias = alias.clone();
        let runner = ObservedScript {
            outputs: RefCell::new(VecDeque::from([
                output(11, ""),
                output(0, "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}"),
            ])),
            requests: RefCell::new(vec![]),
            after_unlock: RefCell::new(Some(Box::new(move || {
                fs::remove_file(&after_unlock_alias).expect("retargeted alias removes");
                symlink(&later_path, &after_unlock_alias).expect("later alias creates");
                std::env::set_current_dir("/").expect("working directory changes");
            }))),
        };
        let http = ScriptedHttp {
            responses: RefCell::new(VecDeque::from([Ok(credentials_response())])),
            requests: RefCell::new(vec![]),
        };
        let clock = FixedClock;
        let maintenance = Maintenance;
        let mut services = services(&runner, &http, &clock, &maintenance);
        services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
        let _directory = CurrentDirectoryGuard::change_to(sandbox.path());
        reset_backup_path_resolution_attempts();

        let result = run_with_backup_journal_resolved_hook(
            move || {
                fs::remove_file(&hook_alias).expect("source alias removes");
                symlink(&replacement_path, &hook_alias).expect("replacement alias creates");
            },
            || run_backup(Path::new("journal"), &services),
        );

        assert_eq!(result.status, "ok");
        let source_path = fs::canonicalize(source.path()).expect("source resolves");
        let requests = runner.requests.borrow();
        assert_eq!(requests.len(), 2);
        let backup_index = requests[1]
            .argv
            .iter()
            .position(|argument| argument == "backup")
            .expect("backup argument present");
        assert_eq!(
            &requests[1].argv[backup_index..],
            json_backup_args(&source_path).as_slice()
        );
        assert_eq!(
            requests[1].timeout,
            Some(Duration::from_secs(BACKUP_TIMEOUT_SECONDS))
        );
        assert_eq!(
            requests[1].env.get("RCLONE_CONFIG_SPB_ACCESS_KEY_ID"),
            Some(&"access".to_owned())
        );
        assert_eq!(
            http.requests.borrow()[0].url,
            "https://source-broker/backup/credentials"
        );
        assert_eq!(
            solstone_core_backup::get_backup_config(source.path()).expect("source state reads")["last_backup"]
                ["snapshot_id"],
            "snap"
        );
        let replacement_raw: Value = serde_json::from_slice(
            &fs::read(replacement.path().join("config/journal.json"))
                .expect("replacement config reads"),
        )
        .expect("replacement config parses");
        assert!(replacement_raw["backup"].get("last_backup").is_none());
        assert_one_resolution_attempt();
    }

    #[cfg(unix)]
    #[test]
    fn operated_credential_failure_after_alias_retarget_records_only_original_journal() {
        use std::os::unix::fs::symlink;

        let source = tempfile::tempdir().expect("source journal creates");
        let mut source_backup = valid_backup_section(valid_destination());
        source_backup["mode"] = Value::String("operated".into());
        write_backup_section(source.path(), source_backup);
        solstone_core_backup::save_hosted_binding(
            source.path(),
            &hosted_binding("https://source-broker"),
        )
        .expect("source binding writes");
        let replacement = tempfile::tempdir().expect("replacement journal creates");
        write_backup_section(replacement.path(), serde_json::json!({"enabled": false}));
        let sandbox = tempfile::tempdir().expect("test sandbox creates");
        let alias = sandbox.path().join("journal");
        symlink(source.path(), &alias).expect("alias creates");
        let replacement_path = replacement.path().to_path_buf();
        let hook_alias = alias.clone();
        let runner = PanicRunner;
        let http = ScriptedHttp {
            responses: RefCell::new(VecDeque::from([Err(HttpError::Unreachable)])),
            requests: RefCell::new(vec![]),
        };
        let clock = FixedClock;
        let maintenance = Maintenance;
        let mut services = services(&runner, &http, &clock, &maintenance);
        services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
        reset_backup_path_resolution_attempts();

        let result = run_with_backup_journal_resolved_hook(
            move || {
                fs::remove_file(&hook_alias).expect("source alias removes");
                symlink(&replacement_path, &hook_alias).expect("replacement alias creates");
            },
            || run_backup(&alias, &services),
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.error_reason.as_deref(), Some("broker_unreachable"));
        assert_eq!(
            http.requests.borrow()[0].url,
            "https://source-broker/backup/credentials"
        );
        assert_eq!(
            solstone_core_backup::get_backup_config(source.path()).expect("source state reads")["last_backup"]
                ["error_reason"],
            "broker_unreachable"
        );
        let replacement_raw: Value = serde_json::from_slice(
            &fs::read(replacement.path().join("config/journal.json"))
                .expect("replacement config reads"),
        )
        .expect("replacement config parses");
        assert!(replacement_raw["backup"].get("last_backup").is_none());
        assert_one_resolution_attempt();
    }

    #[cfg(unix)]
    #[test]
    fn runner_failure_after_alias_retarget_records_only_original_journal() {
        use std::os::unix::fs::symlink;

        let source = configured_journal();
        let replacement = tempfile::tempdir().expect("replacement journal creates");
        write_backup_section(replacement.path(), serde_json::json!({"enabled": false}));
        let sandbox = tempfile::tempdir().expect("test sandbox creates");
        let alias = sandbox.path().join("journal");
        symlink(source.path(), &alias).expect("alias creates");
        let replacement_path = replacement.path().to_path_buf();
        let hook_alias = alias.clone();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([output(11, ""), output(1, "")])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = FixedClock;
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let result = run_with_backup_journal_resolved_hook(
            move || {
                fs::remove_file(&hook_alias).expect("source alias removes");
                symlink(&replacement_path, &hook_alias).expect("replacement alias creates");
            },
            || run_backup(&alias, &services(&runner, &http, &clock, &maintenance)),
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.error_reason.as_deref(), Some("failed"));
        let source_path = fs::canonicalize(source.path()).expect("source resolves");
        assert_eq!(runner.commands.borrow()[1], json_backup_args(&source_path));
        assert_eq!(
            solstone_core_backup::get_backup_config(source.path()).expect("source state reads")["last_backup"]
                ["error_reason"],
            "failed"
        );
        let replacement_raw: Value = serde_json::from_slice(
            &fs::read(replacement.path().join("config/journal.json"))
                .expect("replacement config reads"),
        )
        .expect("replacement config parses");
        assert!(replacement_raw["backup"].get("last_backup").is_none());
        assert_one_resolution_attempt();
    }

    #[test]
    fn insufficient_or_unsupported_byo_destination_fails_without_restic() {
        let cases = [
            (
                "insufficient",
                serde_json::json!({
                    "repository": "s3:repo",
                    "backend": "s3",
                    "credentials": {"access_key_id": "access"},
                }),
            ),
            (
                "empty",
                serde_json::json!({
                    "repository": "s3:repo",
                    "backend": "s3",
                    "credentials": {},
                }),
            ),
            (
                "unsupported",
                serde_json::json!({
                    "repository": "wat:repo",
                    "backend": "wat",
                    "credentials": {},
                }),
            ),
        ];
        for (name, destination) in cases {
            let journal = tempfile::tempdir().expect("test journal creates");
            write_backup_section(journal.path(), valid_backup_section(destination));
            let runner = PanicRunner;
            let http = PanicHttp;
            let clock = FixedClock;
            let maintenance = Maintenance;
            reset_backup_path_resolution_attempts();

            let result = run_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance),
            );

            assert_eq!(result.status, "error", "{name}");
            assert_eq!(result.error_reason.as_deref(), Some("failed"), "{name}");
            assert_one_resolution_attempt();
        }
    }

    #[test]
    fn admitted_backup_uses_one_snapshot_until_a_fresh_run() {
        let journal = tempfile::tempdir().expect("test journal creates");
        let config = journal.path().join("config/journal.json");
        fs::create_dir_all(config.parent().expect("config parent")).expect("config parent creates");
        fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "outside": {"keep": true},
                "backup": valid_backup_section(valid_destination()),
            }))
            .expect("config encodes"),
        )
        .expect("config writes");
        solstone_core_backup::save_hosted_binding(
            journal.path(),
            &hosted_binding("https://before"),
        )
        .expect("initial binding writes");
        let journal_path = journal.path().to_path_buf();
        let runner = ObservedScript {
            outputs: RefCell::new(VecDeque::from([
                output(11, ""),
                output(0, "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}"),
            ])),
            requests: RefCell::new(vec![]),
            after_unlock: RefCell::new(Some(Box::new(move || {
                let mut replacement = valid_backup_section(serde_json::json!({
                    "repository": "b2:repo",
                    "backend": "b2",
                    "credentials": {"account_id": "next", "account_key": "next"},
                }));
                replacement["enabled"] = Value::Bool(false);
                replacement["mode"] = Value::String("operated".into());
                replacement["daily_key"] = Value::String("next-daily".into());
                replacement["recovery_key"] = Value::String(
                    "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ".into(),
                );
                replacement["last_backup"] = serde_json::json!({
                    "status": "ok",
                    "snapshot_id": "next",
                });
                let config = journal_path.join("config/journal.json");
                fs::write(
                    config,
                    serde_json::to_vec(&serde_json::json!({
                        "outside": {"keep": true},
                        "backup": replacement,
                    }))
                    .expect("replacement config encodes"),
                )
                .expect("replacement config writes");
                solstone_core_backup::save_hosted_binding(
                    &journal_path,
                    &hosted_binding("https://after"),
                )
                .expect("replacement binding writes");
            }))),
        };
        let http = Http;
        let clock = FixedClock;
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let result = run_backup(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            runner.requests.borrow()[1].timeout,
            Some(Duration::from_secs(INITIAL_BACKUP_TIMEOUT_SECONDS))
        );
        let raw: Value = serde_json::from_slice(&fs::read(&config).expect("config reads"))
            .expect("config parses");
        assert_eq!(raw["outside"]["keep"], true);
        assert_eq!(raw["backup"]["enabled"], false);
        assert_eq!(raw["backup"]["last_backup"]["snapshot_id"], "snap");
        assert_one_resolution_attempt();

        let fresh_runner = PanicRunner;
        let fresh_http = PanicHttp;
        reset_backup_path_resolution_attempts();
        let fresh = run_backup(
            journal.path(),
            &services(&fresh_runner, &fresh_http, &clock, &maintenance),
        );
        assert_eq!(fresh.status, "skipped");
        assert_one_resolution_attempt();
    }

    #[test]
    fn backup_records_with_the_clock_value_at_consumption_time() {
        let journal = configured_journal();
        let clock_value = Rc::new(Cell::new(50));
        let callback_clock = Rc::clone(&clock_value);
        let runner = ObservedScript {
            outputs: RefCell::new(VecDeque::from([
                output(11, ""),
                output(0, "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}"),
            ])),
            requests: RefCell::new(vec![]),
            after_unlock: RefCell::new(Some(Box::new(move || callback_clock.set(99)))),
        };
        let http = Http;
        let clock = MutableClock(clock_value);
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let result = run_backup(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
        );

        assert_eq!(result.status, "ok");
        assert_eq!(
            solstone_core_backup::get_backup_config(journal.path()).expect("state reads")["last_backup"]
                ["time"],
            99
        );
        assert_one_resolution_attempt();
    }

    #[cfg(unix)]
    #[test]
    fn record_uses_consumption_time_and_lock_failures_preserve_runner_results() {
        use std::os::unix::fs::symlink;

        for (returncode, stdout, status, reason) in [
            (
                0,
                "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}",
                "ok",
                None,
            ),
            (1, "", "error", Some("failed")),
        ] {
            let journal = tempfile::tempdir().expect("test journal creates");
            let config = journal.path().join("config/journal.json");
            fs::create_dir_all(config.parent().expect("config parent"))
                .expect("config parent creates");
            fs::write(
                &config,
                serde_json::to_vec(&serde_json::json!({
                    "outside": "keep",
                    "backup": valid_backup_section(valid_destination()),
                }))
                .expect("config encodes"),
            )
            .expect("config writes");
            let before = fs::read(&config).expect("config reads");
            let clock_value = Rc::new(Cell::new(50));
            let callback_clock = Rc::clone(&clock_value);
            let lock = journal.path().join("config/journal.json.lock");
            let sentinel = journal.path().join("sentinel");
            fs::write(&sentinel, b"sentinel").expect("sentinel writes");
            let runner = ObservedScript {
                outputs: RefCell::new(VecDeque::from([output(11, ""), output(returncode, stdout)])),
                requests: RefCell::new(vec![]),
                after_unlock: RefCell::new(Some(Box::new(move || {
                    callback_clock.set(99);
                    symlink(&sentinel, &lock).expect("lock sentinel links");
                }))),
            };
            let http = Http;
            let clock = MutableClock(clock_value);
            let maintenance = Maintenance;
            reset_backup_path_resolution_attempts();

            let result = run_backup(
                journal.path(),
                &services(&runner, &http, &clock, &maintenance),
            );

            assert_eq!(result.status, status);
            assert_eq!(result.error_reason.as_deref(), reason);
            assert_eq!(clock.0.get(), 99);
            assert_eq!(fs::read(&config).expect("config rereads"), before);
            assert_eq!(
                serde_json::from_slice::<Value>(&fs::read(&config).expect("config rereads"))
                    .expect("config parses")["outside"],
                "keep"
            );
            assert_eq!(runner.requests.borrow().len(), 2);
            assert_one_resolution_attempt();
        }
    }
    #[test]
    fn absent_restic_path_records_existing_unavailable_reason_without_runner_call() {
        let journal = configured_journal();
        reset_backup_path_resolution_attempts();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = FixedClock;
        let maintenance = Maintenance;
        let mut services = services(&runner, &http, &clock, &maintenance);
        services.restic_path = None;

        let result = run_backup(journal.path(), &services);

        assert_eq!(result.status, "error");
        assert_eq!(result.error_reason.as_deref(), Some("restic_unavailable"));
        assert!(runner.commands.borrow().is_empty());
        assert_one_resolution_attempt();
    }
    #[test]
    fn verification_replaces_even_a_later_last_ok_and_clears_on_error() {
        let journal = configured_journal();
        solstone_core_backup::record_verification_result(
            journal.path(),
            "ok",
            serde_json::json!(999),
            Value::Null,
            serde_json::json!("old"),
        )
        .unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([output(0, "")])),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = FixedClock;
        let maintenance = Maintenance;
        let result = run_verification(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            &clock,
        );
        assert_eq!(result.checked_subset, Some("7/52".into()));
        let state = solstone_core_backup::get_backup_config(journal.path()).unwrap();
        assert_eq!(state["last_verification"]["last_ok_time"], 50);
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([output(1, "")])),
            commands: RefCell::new(vec![]),
        };
        let result = run_verification(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            &clock,
        );
        assert_eq!(result.status, "error");
        let state = solstone_core_backup::get_backup_config(journal.path()).unwrap();
        assert_eq!(state["last_verification"]["last_ok_time"], 50);
        assert!(state["last_verification"]["checked_subset"].is_null());
    }
}
