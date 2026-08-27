// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Backup, prune, verification, and archive operations.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_backup::{
    Destination, assemble_backend_env, get_backup_config, record_backup_result,
    record_prune_result, record_restore_result, record_verification_result,
};

use crate::hosted_runtime::{
    HttpTransport, RuntimeResolution, hosted_append_only_session, hosted_session, resolve_runtime,
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
fn backup_args(journal: &Path) -> Vec<String> {
    let mut args = vec!["backup".into(), journal.display().to_string()];
    for excluded in BACKUP_EXCLUDES {
        args.extend(["--exclude".into(), excluded.into()]);
    }
    args
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

pub fn run_backup(journal: &Path, services: &BackupServices<'_>) -> BackupResult {
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
            let result = BackupResult {
                status: "error".into(),
                snapshot_id: None,
                error_reason: Some(reason),
            };
            record_backup(journal, services.clock, &result);
            return result;
        }
    };
    unlock(services, &runtime);
    let timeout = get_backup_config(journal)
        .ok()
        .and_then(|config| {
            config
                .get("last_backup")
                .and_then(Value::as_object)
                .cloned()
        })
        .filter(|last| {
            last.get("status") == Some(&Value::String("ok".into()))
                && last
                    .get("snapshot_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty())
        })
        .map_or(INITIAL_BACKUP_TIMEOUT_SECONDS, |_| BACKUP_TIMEOUT_SECONDS);
    let output = restic(
        services,
        &runtime,
        backup_args(journal),
        true,
        timeout,
        None,
    );
    let result = match output {
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
    };
    record_backup(journal, services.clock, &result);
    result
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
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;

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
    struct Http;
    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("BYO must not fetch broker credentials")
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
    fn configured_journal() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        let destination = Destination {
            repository: "s3:repo".into(),
            backend: "s3".into(),
            credentials: serde_json::json!({"access_key_id":"access","secret_access_key":"secret"})
                .as_object()
                .unwrap()
                .clone(),
        };
        solstone_core_backup::set_destination(journal.path(), &destination).unwrap();
        solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
        solstone_core_backup::set_enabled(journal.path(), true).unwrap();
        journal
    }
    fn services<'a>(
        runner: &'a Script,
        http: &'a Http,
        clock: &'a FixedClock,
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
    }
    #[test]
    fn absent_restic_path_records_existing_unavailable_reason_without_runner_call() {
        let journal = configured_journal();
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
