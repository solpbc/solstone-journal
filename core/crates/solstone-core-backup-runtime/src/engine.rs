// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Backup, prune, verification, and archive operations.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_backup::{
    BackupKeys, Destination, HostedBinding, assemble_backend_env, clear_backup_config,
    delete_hosted_binding, format_recovery_key_display, get_backup_config, record_backup_result,
    record_prune_result, record_restore_result, record_verification_result,
};
use solstone_core_journal_io::{JournalRoot, media::ArchiveSource};

use crate::hosted_runtime::{
    HttpTransport, RuntimeResolution, UreqHttpTransport, fetch_hosted_credentials,
    hosted_append_only_session, hosted_session, resolve_runtime,
};
use crate::runner::{ToolRunner, reason_for_returncode, run_restic, select_summary};

pub const ARCHIVE_TAG: &str = "solstone-archive";
use solstone_core_backup::archive_activity::{
    ArchiveOperationPreparationFailure, OwnedArchivePreparationFailure, OwnedPreparedArchiveHold,
    OwnedPreparedArchiveOperation, OwnedUnusedArchiveRetirement, PreparedArchiveOperation,
    UnusedArchiveRelease,
};
pub use solstone_core_backup::archive_admission::{
    ARCHIVE_BACKUP_TIMEOUT_SECONDS, ARCHIVE_LS_TIMEOUT_SECONDS, Clock,
};
use solstone_core_backup::archive_admission::{AdmissionError, ArchiveBudget, ArchiveCancellation};
use solstone_core_backup::archive_readback::{
    ArchiveEndpointCapture, ArchivePredecessorCapture, ArchiveWriterObservation,
    ArchiveWriterSourceObservation, DirectArchiveEffects, DirectReadCompletion,
    OFFLOAD_ARCHIVE_TAG, OwnedArchiveCreator, OwnedDirectReadHold, OwnedReadOperation,
    OwnedReadbackAdmissionError, OwnedSettledDirectRead, ReadAdmission, ReadOperation,
    ReadbackAdmissionError, ReadbackError, RefusedDirectRead, SettledDirectRead,
};
// Repository contention is coordinated by Restic, within each operation’s deadline.
const REPOSITORY_RETRY_LOCK: &str = "30m";
pub const BACKUP_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;
pub const INITIAL_BACKUP_TIMEOUT_SECONDS: u64 = 48 * 60 * 60;
pub const PRUNE_TIMEOUT_SECONDS: u64 = 2 * 60 * 60;
pub const PRUNE_MAX_REPACK_SIZE: &str = "1G";
pub const KEY_LABEL_TIMEOUT_SECONDS: u64 = 5 * 60;
pub const UNLOCK_TIMEOUT_SECONDS: u64 = 5 * 60;
pub const VERIFY_TIMEOUT_SECONDS: u64 = 60 * 60;

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
        solstone_core_indexer_store::todo::enqueue_token(
            journal,
            solstone_core_indexer_store::todo::Token::ScanFull,
        )
        .map_err(|_| JournalMaintenanceError)
    }
}

/// Explicit dependencies for native backup runtime operations.
pub struct BackupServices<'a> {
    pub runner: &'a dyn ToolRunner,
    pub http: &'a dyn HttpTransport,
    pub clock: &'a dyn Clock,
    /// Captured execution authority; readiness/install remains resolution authority.
    pub restic: Option<crate::runner::ResticInvocation<'a>>,
    /// Required only for operated append-only backup/archive sessions.
    pub rclone_path: Option<&'a Path>,
    pub version: &'a str,
    pub journal_maintenance: &'a dyn JournalMaintenance,
}

impl BackupServices<'_> {
    pub fn restic_invocation(&self) -> Result<crate::runner::ResticInvocation<'_>, String> {
        self.restic.ok_or_else(|| "restic_unavailable".to_owned())
    }

    /// Linked and admitted archive selection. Offload and ordinary backup stay
    /// on `restic_path`, which remains executable-only.
    pub fn archive_invocation(&self) -> Result<crate::runner::ResticInvocation<'_>, String> {
        self.restic_invocation()
    }

    /// Offload and ordinary backup stay executable-only.
    pub fn restic_path(&self) -> Result<&Path, String> {
        self.restic_invocation()?
            .executable_path()
            .ok_or_else(|| "restic_unavailable".to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupResult {
    pub status: String,
    pub snapshot_id: Option<String>,
    pub error_reason: Option<String>,
    /// What restic said it could not read, when it said anything.
    ///
    /// 🔴 The tool already hands us the reason per file, on its JSON stderr, and
    /// the runner already captures it. Discarding it and telling the owner to
    /// go check their permissions is sending them elsewhere for an answer this
    /// line is holding -- and it is a guess: a vanished file, an I/O error and a
    /// mode problem all land here and only one of them is a permission problem.
    pub unreadable: Option<UnreadableSources>,
    pub detail: Option<String>,
}

/// The source-read failures behind a partial backup, as restic reported them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnreadableSources {
    /// How many source files restic reported it could not read.
    pub count: usize,
    /// The first reason verbatim, for an owner who has one thing to fix.
    pub first_reason: Option<String>,
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

/// An admitted backup run bound to one resolved journal and captured configuration.
///
/// ```compile_fail,E0451
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// let _ = AdmittedCapability { _private: () };
/// ```
///
/// ```compile_fail,E0599
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// let _ = AdmittedCapability::default();
/// ```
///
/// ```compile_fail,E0277
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// let _ = serde_json::from_str::<AdmittedCapability>("{}");
/// ```
///
/// ```compile_fail,E0277
/// use std::path::PathBuf;
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_convert(capability: AdmittedCapability) {
///     let _: PathBuf = capability.into();
/// }
/// ```
///
/// ```compile_fail,E0277
/// use std::path::Path;
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn needs_path<T: AsRef<Path>>(_: T) {}
///
/// fn cannot_as_ref(capability: AdmittedCapability) {
///     needs_path(capability);
/// }
/// ```
///
/// ```compile_fail,E0277
/// use std::borrow::Borrow;
/// use std::path::Path;
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_borrow(capability: AdmittedCapability) {
///     let _: &Path = Borrow::<Path>::borrow(&capability);
/// }
/// ```
///
/// ```compile_fail,E0277
/// use std::ops::Deref;
/// use std::path::Path;
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_deref(capability: AdmittedCapability) {
///     let _: &Path = Deref::deref(&capability);
/// }
/// ```
///
/// ```compile_fail,E0308
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_clone(capability: AdmittedCapability) {
///     let _: AdmittedCapability = capability.clone();
/// }
/// ```
///
/// ```compile_fail,E0382
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn consume(_: AdmittedCapability) {}
///
/// fn cannot_copy(capability: AdmittedCapability) {
///     consume(capability);
///     consume(capability);
/// }
/// ```
///
/// ```compile_fail,E0277
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_debug(capability: &AdmittedCapability) {
///     let _ = format!("{capability:?}");
/// }
/// ```
///
/// ```compile_fail,E0277
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_display(capability: &AdmittedCapability) {
///     let _ = format!("{capability}");
/// }
/// ```
///
/// ```compile_fail,E0277
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn requires_serialize<T: serde::Serialize>(_: T) {}
///
/// fn cannot_serialize(capability: AdmittedCapability) {
///     requires_serialize(capability);
/// }
/// ```
///
/// ```compile_fail,E0616
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_read_journal(capability: AdmittedCapability) {
///     let _ = capability.resolved_journal;
/// }
/// ```
///
/// ```compile_fail,E0616
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_read_timeout(capability: AdmittedCapability) {
///     let _ = capability.timeout_seconds;
/// }
/// ```
///
/// ```compile_fail,E0616
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_read_keys(capability: AdmittedCapability) {
///     let _ = capability.keys;
/// }
/// ```
///
/// ```compile_fail,E0616
/// use solstone_core_backup_runtime::AdmittedCapability;
///
/// fn cannot_read_mode(capability: AdmittedCapability) {
///     let _ = capability.mode;
/// }
/// ```
///
/// ```compile_fail,E0382
/// use solstone_core_backup_runtime::{
///     AdmittedCapability, BackupServices, Clock, ClosedToolError,
/// };
///
/// fn cannot_execute_then_record(
///     capability: AdmittedCapability,
///     services: &BackupServices<'_>,
///     clock: &dyn Clock,
/// ) {
///     let _ = capability.execute(services);
///     let _ = capability.record_tool_error(
///         clock,
///         ClosedToolError::ResticUnavailable {
///             detail: "d".into(),
///             guidance: "g".into(),
///         },
///     );
/// }
/// ```
///
/// ```compile_fail,E0382
/// use solstone_core_backup_runtime::{
///     AdmittedCapability, BackupServices, Clock, ClosedToolError,
/// };
///
/// fn cannot_record_then_execute(
///     capability: AdmittedCapability,
///     services: &BackupServices<'_>,
///     clock: &dyn Clock,
/// ) {
///     let _ = capability.record_tool_error(
///         clock,
///         ClosedToolError::RcloneUnavailable {
///             detail: "d".into(),
///             guidance: "g".into(),
///         },
///     );
///     let _ = capability.execute(services);
/// }
/// ```
///
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use std::path::Path;
/// use solstone_core_backup_runtime::{prepare, AdmittedCapability, Clock};
///
/// fn drops_the_capability(journal: &Path, clock: &dyn Clock) {
///     let capability: AdmittedCapability = prepare(journal, clock).unwrap();
///     capability;
/// }
/// ```
#[must_use = "an admitted backup capability must be executed or recorded as a tool-resolution error"]
pub struct AdmittedCapability {
    pub(crate) resolved_journal: PathBuf,
    pub(crate) timeout_seconds: u64,
    pub(crate) keys: BackupKeys,
    pub(crate) mode: AdmittedBackupMode,
    setup_fingerprint: Option<String>,
    pub(crate) _private: (),
}

pub(crate) enum AdmittedBackupMode {
    Byo { destination: Destination },
    Operated { binding: HostedBinding },
}

/// Tool-resolution failure closed to restic or rclone unavailability.
///
/// The vocabulary is externally exhaustive:
///
/// ```
/// use solstone_core_backup_runtime::ClosedToolError;
///
/// fn reason(error: ClosedToolError) -> &'static str {
///     match error {
///         ClosedToolError::ResticUnavailable { .. } => "restic_unavailable",
///         ClosedToolError::RcloneUnavailable { .. } => "rclone_unavailable",
///     }
/// }
/// # assert_eq!(reason(ClosedToolError::ResticUnavailable { detail: "missing".into(), guidance: solstone_core_installed_payload::guidance::MANIFEST_MISSING.into() }), "restic_unavailable");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClosedToolError {
    ResticUnavailable { detail: String, guidance: String },
    RcloneUnavailable { detail: String, guidance: String },
}

impl ClosedToolError {
    pub fn detail(&self) -> &str {
        match self {
            Self::ResticUnavailable { detail, .. } => detail,
            Self::RcloneUnavailable { detail, .. } => detail,
        }
    }

    pub fn guidance(&self) -> &str {
        match self {
            Self::ResticUnavailable { guidance, .. } => guidance,
            Self::RcloneUnavailable { guidance, .. } => guidance,
        }
    }
}

impl fmt::Display for ClosedToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ResticUnavailable { .. } => "restic_unavailable",
            Self::RcloneUnavailable { .. } => "rclone_unavailable",
        })
    }
}

impl std::error::Error for ClosedToolError {}

enum BackupAdmissionTerminal {
    Skip,
    Unresolved,
    Error {
        record_journal: PathBuf,
        reason: &'static str,
    },
}

pub(crate) fn value_env(destination: &Destination) -> Option<BTreeMap<String, Option<String>>> {
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
            // iOS's operated destination is itself create-only through the
            // linked transport, so it needs no append-only rclone session.
            let session = if append_only && !cfg!(target_os = "ios") {
                let rclone = services
                    .rclone_path
                    .ok_or_else(|| "rclone_unavailable".to_owned())?;
                hosted_append_only_session(&binding, &credentials, rclone)
                    .map_err(|_| "rclone_unavailable")?
            } else {
                hosted_session(&binding, &credentials).map_err(|_| "failed")?
            };
            let runtime = Runtime {
                destination: session.destination,
                password: resolve_keys(journal)?,
                backend_env: session
                    .backend_env
                    .into_iter()
                    .map(|(key, value)| (key, Some(value)))
                    .collect(),
                global_options: session.global_options,
            };
            // Only the operated maintenance credential response supplies this
            // writable session. A BYO destination's append-only intent stays intact.
            #[cfg(target_os = "ios")]
            let runtime = if scope == "maintenance" && !append_only {
                without_ios_append_only(runtime)
            } else {
                runtime
            };
            Ok(Some(runtime))
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
    let restic_path = services.restic_invocation()?;
    let mut full = Vec::new();
    full.append(&mut runtime.global_options.clone());
    let waits_for_repository = matches!(
        args.first().map(String::as_str),
        Some("backup" | "forget" | "check")
    );
    full.append(&mut args);
    if waits_for_repository {
        full.extend(["--retry-lock".into(), REPOSITORY_RETRY_LOCK.into()]);
    }
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
    let mut args = vec![
        "backup".into(),
        crate::restic_filesystem_path(resolved_journal),
    ];
    for excluded in solstone_core_journal_io::copy_excludes::COPY_EXCLUDES {
        args.extend([
            "--exclude".into(),
            crate::restic_paths::rooted_exclude(resolved_journal, excluded),
        ]);
    }
    args
}

struct PreparedBackup {
    args: Vec<String>,
    // The child reads this file while running; keep it private and alive throughout.
    _exclude_file: tempfile::NamedTempFile,
}

fn prepare_backup_args(root: &Path) -> std::io::Result<PreparedBackup> {
    use solstone_core_journal_io::copy_excludes::{
        BODY_DATABASES, LEGACY_DAY_DATABASES, SQLITE_SUFFIXES,
    };
    use std::io::Write;

    let mut excludes = tempfile::NamedTempFile::new()?;
    let mut add_slots = |parent: &str, names: &[&str]| -> std::io::Result<()> {
        for name in names {
            for suffix in SQLITE_SUFFIXES {
                let member = format!("{parent}/{name}{suffix}");
                match fs::symlink_metadata(root.join(&member)) {
                    Ok(metadata) if metadata.is_dir() => continue,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                let pattern = crate::restic_paths::rooted_exclude(root, &member);
                if pattern.contains(['\r', '\n']) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "journal path contains a line break",
                    ));
                }
                // restic expands environment variables in exclude files.
                writeln!(excludes, "{}", pattern.replace('$', "$$"))?;
            }
        }
        Ok(())
    };
    add_slots("imports", BODY_DATABASES)?;
    match fs::read_dir(root.join("chronicle")) {
        Ok(days) => {
            for day in days {
                let day = day?;
                let name = day.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.len() == 8
                    && name.bytes().all(|byte| byte.is_ascii_digit())
                    && day.file_type()?.is_dir()
                {
                    add_slots(&format!("chronicle/{name}/indexer"), LEGACY_DAY_DATABASES)?;
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    excludes.flush()?;
    let mut args = backup_args(root);
    args.extend([
        "--exclude-file".into(),
        crate::restic_filesystem_path(excludes.path()),
    ]);
    Ok(PreparedBackup {
        args,
        _exclude_file: excludes,
    })
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

fn prepare_backup_run(journal: &Path) -> Result<AdmittedCapability, BackupAdmissionTerminal> {
    let resolved_journal = match resolve_backup_journal(journal) {
        Ok(resolved_journal) => resolved_journal,
        Err(_) => {
            #[cfg(any(test, feature = "test-hooks"))]
            clear_backup_journal_resolved_hook();
            return Err(BackupAdmissionTerminal::Unresolved);
        }
    };
    #[cfg(any(test, feature = "test-hooks"))]
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
    Ok(AdmittedCapability {
        resolved_journal,
        timeout_seconds: backup_timeout_seconds(&config),
        setup_fingerprint: solstone_core_backup::backup_setup_fingerprint(
            &config,
            match &mode {
                AdmittedBackupMode::Operated { binding } => Some(binding),
                AdmittedBackupMode::Byo { .. } => None,
            },
        ),
        keys,
        mode,
        _private: (),
    })
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static ON_BACKUP_JOURNAL_RESOLVED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(all(test, not(windows)), feature = "test-hooks"))]
type BackupToolResolutionStartedHook = Box<dyn FnOnce(&Path)>;

#[cfg(any(all(test, not(windows)), feature = "test-hooks"))]
thread_local! {
    static ON_BACKUP_TOOL_RESOLUTION_STARTED: std::cell::RefCell<Option<BackupToolResolutionStartedHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    #[cfg(target_os = "android")]
    static BACKUP_PATH_RESOLUTION_ATTEMPTS: Box<std::cell::Cell<u32>> = Box::new(std::cell::Cell::new(0));
    #[cfg(not(target_os = "android"))]
    static BACKUP_PATH_RESOLUTION_ATTEMPTS: std::cell::Cell<u32> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    #[cfg(target_os = "android")]
    static BACKUP_RECORD_FAILURE_TARGET: Box<std::cell::RefCell<Option<PathBuf>>> = Box::new(std::cell::RefCell::new(None));
    #[cfg(not(target_os = "android"))]
    static BACKUP_RECORD_FAILURE_TARGET: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    #[cfg(target_os = "android")]
    static BACKUP_RECORD_FAILURE_CONSUMED_TARGET: Box<std::cell::RefCell<Option<PathBuf>>> = Box::new(std::cell::RefCell::new(None));
    #[cfg(not(target_os = "android"))]
    static BACKUP_RECORD_FAILURE_CONSUMED_TARGET: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
fn run_on_backup_journal_resolved_hook() {
    let hook = ON_BACKUP_JOURNAL_RESOLVED.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(all(not(windows), any(test, feature = "test-hooks")))]
pub(crate) fn run_backup_tool_resolution_started_hook(resolved_journal: &Path) {
    let hook = ON_BACKUP_TOOL_RESOLUTION_STARTED.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook(resolved_journal);
    }
}

#[cfg(feature = "test-hooks")]
fn arm_backup_tool_resolution_started_hook(hook: impl FnOnce(&Path) + 'static) {
    ON_BACKUP_TOOL_RESOLUTION_STARTED.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "backup tool resolution started hook is already active"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(feature = "test-hooks")]
fn clear_backup_tool_resolution_started_hook() {
    ON_BACKUP_TOOL_RESOLUTION_STARTED.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(any(test, feature = "test-hooks"))]
fn arm_backup_journal_resolved_hook(hook: impl FnOnce() + 'static) {
    ON_BACKUP_JOURNAL_RESOLVED.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "backup journal resolved hook is already active"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(any(test, feature = "test-hooks"))]
fn clear_backup_journal_resolved_hook() {
    ON_BACKUP_JOURNAL_RESOLVED.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(any(test, feature = "test-hooks"))]
fn arm_backup_record_failure_hook(expected_target: PathBuf) {
    BACKUP_RECORD_FAILURE_TARGET.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "backup record failure hook is already active"
        );
        *slot.borrow_mut() = Some(expected_target);
    });
    BACKUP_RECORD_FAILURE_CONSUMED_TARGET.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(any(test, feature = "test-hooks"))]
fn run_backup_record_failure_hook(journal: &Path) -> bool {
    let expected_target = BACKUP_RECORD_FAILURE_TARGET.with(|slot| slot.borrow_mut().take());
    let Some(expected_target) = expected_target else {
        return false;
    };
    assert!(
        journal == expected_target,
        "backup record failure hook target mismatch: expected {}, actual {}",
        expected_target.display(),
        journal.display()
    );
    BACKUP_RECORD_FAILURE_CONSUMED_TARGET.with(|slot| {
        *slot.borrow_mut() = Some(journal.to_path_buf());
    });
    true
}

#[cfg(any(test, feature = "test-hooks"))]
fn clear_backup_record_failure_hook() {
    BACKUP_RECORD_FAILURE_TARGET.with(|slot| {
        slot.borrow_mut().take();
    });
    BACKUP_RECORD_FAILURE_CONSUMED_TARGET.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(test)]
fn run_with_backup_journal_resolved_hook<T>(
    hook: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> T {
    arm_backup_journal_resolved_hook(hook);
    let result = op();
    ON_BACKUP_JOURNAL_RESOLVED.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "backup journal resolved hook was not reached"
        );
    });
    result
}

/// Test-only instrumentation: arm a one-shot callback after journal path resolution.
///
/// The callback runs before backup configuration is read. Arming a second callback
/// on the same thread fails visibly instead of replacing the first callback.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn install_backup_journal_resolved_hook(hook: impl FnOnce() + 'static) {
    arm_backup_journal_resolved_hook(hook);
}

/// Test-only instrumentation: discard any armed resolver-completion callback.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn reset_backup_journal_resolved_hook() {
    clear_backup_journal_resolved_hook();
}

/// Test-only instrumentation: report whether this thread has an armed callback.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn backup_journal_resolved_hook_armed() -> bool {
    ON_BACKUP_JOURNAL_RESOLVED.with(|slot| slot.borrow().is_some())
}

/// Test-only instrumentation: arm a one-shot callback at tool-resolution entry.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn install_backup_tool_resolution_started_hook(hook: impl FnOnce(&Path) + 'static) {
    arm_backup_tool_resolution_started_hook(hook);
}

/// Test-only instrumentation: discard any armed tool-resolution callback.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn reset_backup_tool_resolution_started_hook() {
    clear_backup_tool_resolution_started_hook();
}

/// Test-only instrumentation: report whether this thread has an armed tool-resolution callback.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn backup_tool_resolution_started_hook_armed() -> bool {
    ON_BACKUP_TOOL_RESOLUTION_STARTED.with(|slot| slot.borrow().is_some())
}

/// Test-only instrumentation: arm a one-shot backup-record failure seam.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn install_backup_record_failure_hook(expected_target: PathBuf) {
    arm_backup_record_failure_hook(expected_target);
}

/// Test-only instrumentation: discard backup-record failure seam state.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn reset_backup_record_failure_hook() {
    clear_backup_record_failure_hook();
}

/// Test-only instrumentation: report whether this thread has an armed backup-record seam.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn backup_record_failure_hook_armed() -> bool {
    BACKUP_RECORD_FAILURE_TARGET.with(|slot| slot.borrow().is_some())
}

/// Test-only instrumentation: read the target consumed by the backup-record seam.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn backup_record_failure_hook_consumed_target() -> Option<PathBuf> {
    BACKUP_RECORD_FAILURE_CONSUMED_TARGET.with(|slot| slot.borrow().clone())
}

/// Test-only instrumentation: reset the per-thread `resolve_backup_journal` attempt count.
#[cfg(any(test, feature = "test-hooks"))]
pub fn reset_backup_path_resolution_attempts() {
    BACKUP_PATH_RESOLUTION_ATTEMPTS.with(|attempts| attempts.set(0));
}

/// Test-only instrumentation: read the per-thread `resolve_backup_journal` attempt count.
#[cfg(any(test, feature = "test-hooks"))]
pub fn backup_path_resolution_attempts() -> u32 {
    BACKUP_PATH_RESOLUTION_ATTEMPTS.with(|count| count.get())
}
fn snapshot_id(value: Option<&Value>) -> Option<String> {
    select_summary(value?)
        .and_then(|summary| summary.get("snapshot_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}
/// Count restic's own per-file read failures and keep the first reason verbatim.
///
/// restic emits one `{"message_type":"error", …}` record per unreadable source.
/// 🔴 It writes them to **stderr**, not to the stdout stream the summary
/// arrives on (measured on Windows, restic `backup --json`: every error record
/// on stderr, none on stdout), so both streams are read; stdout first, for any
/// build that puts them there. A stderr line that is not JSON is skipped, never
/// a reason to drop the others. ⛔ The reason is NOT interpreted here: a
/// vanished file, an I/O error and a mode problem all arrive through this
/// record and only the tool knows which one it hit.
fn unreadable_sources(output: &crate::runner::ResticResult) -> Option<UnreadableSources> {
    let stdout: &[Value] = match output.json.as_ref() {
        Some(Value::Array(records)) => records,
        Some(single) => std::slice::from_ref(single),
        None => &[],
    };
    let stderr: Vec<Value> = output
        .stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect();
    let mut count = 0;
    let mut first_reason = None;
    for record in stdout.iter().chain(&stderr) {
        if record.get("message_type").and_then(Value::as_str) != Some("error") {
            continue;
        }
        count += 1;
        if first_reason.is_none() {
            first_reason = record
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .filter(|message| !message.is_empty())
                .map(str::to_owned);
        }
    }
    (count > 0).then_some(UnreadableSources {
        count,
        first_reason,
    })
}

fn record_backup(
    journal: &Path,
    clock: &dyn Clock,
    result: &BackupResult,
    setup_fingerprint: Option<&str>,
) {
    #[cfg(any(test, feature = "test-hooks"))]
    if run_backup_record_failure_hook(journal) {
        return;
    }
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
        result.detail.as_deref(),
        setup_fingerprint,
    );
}

/// Whether this journal has backup turned on. A journal without it needs no
/// backup tool: its maintenance and prune commands skip before running one.
pub fn backup_enabled(journal: &Path) -> bool {
    get_backup_config(journal).is_ok_and(|config| config.get("enabled") == Some(&Value::Bool(true)))
}

/// Persist a backup-run failure that happened before `run_backup` was invoked.
pub fn record_backup_error(journal: &Path, clock: &dyn Clock, reason: &str) -> BackupResult {
    let result = BackupResult {
        status: "error".into(),
        snapshot_id: None,
        error_reason: Some(reason.to_owned()),
        unreadable: None,
        detail: None,
    };
    record_backup(journal, clock, &result, None);
    result
}

/// Admit a backup configuration and bind it to its canonical journal path.
///
/// ```
/// use std::path::Path;
/// use solstone_core_backup_runtime::{
///     prepare, resolve_tools, Clock, ToolRunner,
/// };
///
/// fn prepares_then_borrows_for_tool_resolution(
///     journal: &Path,
///     clock: &dyn Clock,
///     runner: &dyn ToolRunner,
///     executable: &Path,
/// ) {
///     if let Ok(capability) = prepare(journal, clock) {
///         let _ = resolve_tools(
///             &capability,
///             runner,
///             executable,
///         );
///     }
/// }
/// ```
#[allow(clippy::result_large_err)] // The refusal record is the Err. It carries the run status and the resolver code.
pub fn prepare(journal: &Path, clock: &dyn Clock) -> Result<AdmittedCapability, BackupResult> {
    match prepare_backup_run(journal) {
        Ok(capability) => Ok(capability),
        Err(BackupAdmissionTerminal::Skip) => Err(BackupResult {
            status: "skipped".into(),
            snapshot_id: None,
            error_reason: None,
            unreadable: None,
            detail: None,
        }),
        Err(BackupAdmissionTerminal::Unresolved) => Err(BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some("journal_path_unresolved".into()),
            unreadable: None,
            detail: None,
        }),
        Err(BackupAdmissionTerminal::Error {
            record_journal,
            reason,
        }) => Err(record_backup_error(&record_journal, clock, reason)),
    }
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

impl AdmittedCapability {
    /// Consume the capability and run the admitted backup, recording the outcome at its resolved journal.
    pub fn execute(self, services: &BackupServices<'_>) -> BackupResult {
        let AdmittedCapability {
            resolved_journal,
            timeout_seconds,
            keys,
            mode,
            setup_fingerprint,
            _private: (),
        } = self;
        let prepared = match prepare_backup_args(&resolved_journal) {
            Ok(prepared) => prepared,
            Err(error) => {
                let result = BackupResult {
                    status: "error".into(),
                    snapshot_id: None,
                    error_reason: Some("failed".into()),
                    unreadable: None,
                    detail: Some(error.to_string()),
                };
                record_backup(
                    &resolved_journal,
                    services.clock,
                    &result,
                    setup_fingerprint.as_deref(),
                );
                return result;
            }
        };
        let runtime = match mode {
            AdmittedBackupMode::Byo { destination }
                if !solstone_core_backup::repository_names_backend(&destination) =>
            {
                Err("repository_unsupported".to_owned())
            }
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
                // iOS's operated destination is itself create-only.
                #[cfg(target_os = "ios")]
                let session = hosted_session(&binding, &credentials).map_err(|_| "failed")?;
                #[cfg(not(target_os = "ios"))]
                let session = {
                    let rclone = services
                        .rclone_path
                        .ok_or_else(|| "rclone_unavailable".to_owned())?;
                    hosted_append_only_session(&binding, &credentials, rclone)
                        .map_err(|_| "rclone_unavailable")?
                };
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
                    prepared.args.clone(),
                    true,
                    timeout_seconds,
                    None,
                ) {
                    Ok(output) => {
                        let id = snapshot_id(output.json.as_ref());
                        match (output.returncode, id) {
                            (0, Some(id)) => BackupResult {
                                status: "ok".into(),
                                snapshot_id: Some(id),
                                error_reason: None,
                                unreadable: None,
                                detail: None,
                            },
                            // 🔴 restic exit 3 means a snapshot WAS written and
                            // some source files could not be read. That is a
                            // backup with a caveat, not a failed backup, and
                            // calling it an error made the owner's own status
                            // surface read "failed" while their data was in
                            // fact backed up -- `status` is what drives that
                            // copy key. On Windows the running journal holds
                            // files open, so this is the ORDINARY outcome
                            // there: reporting it as failure would have meant a
                            // working BYO backup showing failed every day,
                            // which is how an owner learns to ignore the one
                            // signal that matters. `error_reason` keeps the
                            // caveat for anyone who looks.
                            (3, Some(id)) => BackupResult {
                                status: "ok".into(),
                                snapshot_id: Some(id),
                                error_reason: Some("incomplete".into()),
                                unreadable: unreadable_sources(&output),
                                detail: None,
                            },
                            // ⛔ No snapshot means no backup, whatever the code.
                            // ⛔ Reaching here on code 3 means the summary did
                            // not parse, which is "we do not have the id" and
                            // never "nothing was written" -- a source-read
                            // failure is exactly the condition that DOES write
                            // a snapshot. The copy must not claim an absence
                            // this branch cannot prove.
                            (code, _) => BackupResult {
                                status: "error".into(),
                                snapshot_id: None,
                                error_reason: Some(if code == 0 {
                                    "unknown".into()
                                } else {
                                    reason_for_returncode(code).into()
                                }),
                                unreadable: (code == 3)
                                    .then(|| unreadable_sources(&output))
                                    .flatten(),
                                detail: None,
                            },
                        }
                    }
                    Err(reason) => BackupResult {
                        status: "error".into(),
                        snapshot_id: None,
                        error_reason: Some(reason),
                        unreadable: None,
                        detail: None,
                    },
                }
            }
            Err(reason) => BackupResult {
                status: "error".into(),
                snapshot_id: None,
                error_reason: Some(reason),
                unreadable: None,
                detail: None,
            },
        };
        // 🔴 `binding_superseded` can only reach this branch through the
        // Operated mint call above (teardown.rs's identical string is the
        // only sibling producer, and that path never runs here). A
        // superseded binding cannot become valid again by itself -- only an
        // owner's new consent mints a new one -- so retrying it hourly,
        // forever, teaches the scheduler nothing and costs the broker a
        // refused mint every time. Clear local state the same way an
        // explicit `journal backup off` already does on this exact reason
        // (teardown.rs), so the next scheduled run finds nothing configured
        // and skips cleanly instead of repeating a doomed mint.
        let result = if result.status == "error"
            && result.error_reason.as_deref() == Some("binding_superseded")
        {
            match clear_backup_config(&resolved_journal)
                .and_then(|()| delete_hosted_binding(&resolved_journal))
            {
                Ok(()) => BackupResult {
                    status: "cleared_superseded".into(),
                    snapshot_id: None,
                    error_reason: Some("binding_superseded".into()),
                    unreadable: None,
                    detail: None,
                },
                // Clearing failed -- keep the original error so the run is
                // retried rather than silently going quiet on a state we
                // never actually cleared.
                Err(_) => result,
            }
        } else {
            result
        };
        record_backup(
            &resolved_journal,
            services.clock,
            &result,
            setup_fingerprint.as_deref(),
        );
        result
    }

    /// Consume the capability and record a tool-resolution failure at its resolved journal.
    pub fn record_tool_error(self, clock: &dyn Clock, error: ClosedToolError) -> BackupResult {
        let (reason, detail) = match error {
            ClosedToolError::ResticUnavailable { detail, .. } => ("restic_unavailable", detail),
            ClosedToolError::RcloneUnavailable { detail, .. } => ("rclone_unavailable", detail),
        };
        let result = BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some(reason.into()),
            unreadable: None,
            detail: Some(detail),
        };
        record_backup(
            &self.resolved_journal,
            clock,
            &result,
            self.setup_fingerprint.as_deref(),
        );
        result
    }
}

pub fn run_backup(journal: &Path, services: &BackupServices<'_>) -> BackupResult {
    match prepare(journal, services.clock) {
        Ok(capability) => capability.execute(services),
        Err(result) => result,
    }
}

#[cfg(target_os = "ios")]
fn without_ios_append_only(mut runtime: Runtime) -> Runtime {
    runtime.backend_env.remove("SOLSTONE_APPEND_ONLY");
    runtime
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
    let config = get_backup_config(journal).unwrap_or_default();
    if config.get("mode") == Some(&Value::String("operated".into())) {
        neutralize_operated_key_labels(services, &runtime, &config);
    }
    let retention = config
        .get("retention")
        .and_then(Value::as_object)
        .cloned()
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
        "--keep-tag".into(),
        OFFLOAD_ARCHIVE_TAG.into(),
        // A repository holds one journal, so rotation spans every machine name
        // and journal path its snapshots were taken under. restic's default
        // host+paths grouping leaves an older name's or path's snapshots in a
        // group that is never rotated again.
        "--group-by".into(),
        "tags".into(),
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

/// Prune holds the one credential that can delete, so it carries every operated
/// repository's key files to the neutral label. A failure leaves the repository
/// as it was and the next daily prune tries again; it never fails the prune.
fn neutralize_operated_key_labels(
    services: &BackupServices<'_>,
    runtime: &Runtime,
    config: &Map<String, Value>,
) {
    let (Ok(Some(keys)), Ok(restic_path)) =
        (keys_from_backup_config(config), services.restic_path())
    else {
        return;
    };
    crate::repo::log_key_label_outcome(crate::repo::neutralize_key_labels(
        services.runner,
        &runtime.destination,
        &keys.daily_key,
        &keys.recovery_key,
        restic_path,
        Some(Duration::from_secs(KEY_LABEL_TIMEOUT_SECONDS)),
    ));
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
                unreadable: None,
                detail: None,
            };
        }
        Ok(Some(runtime))
            if !solstone_core_backup::repository_names_backend(&runtime.destination) =>
        {
            return BackupResult {
                status: "error".into(),
                snapshot_id: None,
                error_reason: Some("repository_unsupported".into()),
                unreadable: None,
                detail: None,
            };
        }
        Ok(Some(runtime)) => runtime,
        Err(reason) => {
            return BackupResult {
                status: "error".into(),
                snapshot_id: None,
                error_reason: Some(reason),
                unreadable: None,
                detail: None,
            };
        }
    };
    unlock(services, &runtime);
    let mut args = vec!["backup".into()];
    args.extend(paths.iter().map(|path| crate::restic_filesystem_path(path)));
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
                unreadable: None,
                detail: None,
            },
            |id| BackupResult {
                status: "ok".into(),
                snapshot_id: Some(id),
                error_reason: None,
                unreadable: None,
                detail: None,
            },
        ),
        Ok(output) => BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some(reason_for_returncode(output.returncode).into()),
            unreadable: None,
            detail: None,
        },
        Err(reason) => BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some(reason),
            unreadable: None,
            detail: None,
        },
    }
}

/// Why a pinned archive operation could not proceed. No tool output or secrets
/// are carried into this public error boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveError {
    NotConfigured,
    ConfigurationUnavailable,
    ConfigurationChanged,
    JournalIdentityChanged,
    ToolUnavailable,
    CredentialsUnavailable { reason: &'static str },
    InvalidCredentialExpiry,
    CredentialsExpired,
    DeadlineExceeded,
    ClockChanged,
    Cancelled,
    RepositoryIdentityInvalid,
    RepositoryIdentityChanged,
    ToolFailed { reason: &'static str },
    InvalidSelection,
    SourceUnavailable,
    SnapshotIdentityInvalid,
    Readback(ReadbackError),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::NotConfigured => "archive backup is not configured",
            Self::ConfigurationUnavailable => "archive configuration could not be read",
            Self::ConfigurationChanged => "archive configuration changed",
            Self::JournalIdentityChanged => "your journal could not be confirmed",
            Self::ToolUnavailable => "an archive tool is unavailable",
            Self::CredentialsUnavailable { reason } | Self::ToolFailed { reason } => reason,
            Self::InvalidCredentialExpiry => "archive credentials have an invalid expiry",
            Self::CredentialsExpired => "archive credentials expired",
            Self::DeadlineExceeded => "the archive operation deadline was reached",
            Self::ClockChanged => "the archive clock moved backward",
            Self::Cancelled => "the archive operation was cancelled",
            Self::RepositoryIdentityInvalid => "the backup could not be identified",
            Self::RepositoryIdentityChanged => "the backup no longer matches",
            Self::InvalidSelection => {
                "archive sources must be a nonempty selection of absolute paths"
            }
            Self::SourceUnavailable => "the prepared archive files could not be confirmed",
            Self::SnapshotIdentityInvalid => "the archive did not return a full snapshot identity",
            Self::Readback(error) => return error.fmt(formatter),
        };
        formatter.write_str(reason)
    }
}

impl std::error::Error for ArchiveError {}

impl From<AdmissionError> for ArchiveError {
    fn from(error: AdmissionError) -> Self {
        match error {
            AdmissionError::ConfigurationUnavailable => Self::ConfigurationUnavailable,
            AdmissionError::ConfigurationChanged => Self::ConfigurationChanged,
            AdmissionError::JournalIdentityChanged => Self::JournalIdentityChanged,
            AdmissionError::DeadlineExceeded => Self::DeadlineExceeded,
            AdmissionError::ClockChanged => Self::ClockChanged,
            AdmissionError::CredentialsExpired => Self::CredentialsExpired,
            AdmissionError::Cancelled => Self::Cancelled,
        }
    }
}

/// Remote effects observed on a refused archive. An attempted request may have
/// written remotely even when no snapshot identity was returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArchiveRemoteEffects {
    NotAttempted,
    Attempted {
        returncode: Option<i32>,
        observed_snapshot_id: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveFailure {
    pub error: ArchiveError,
    pub remote_effects: ArchiveRemoteEffects,
}

impl fmt::Display for ArchiveFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for ArchiveFailure {}

/// A successful archive command's identities, not a content-verification claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveSnapshot {
    pub repository_id: String,
    pub snapshot_id: String,
}

// Keep the existing broker protocol while bounding its request by this operation.
struct ArchiveAdmissionHttp<'operation, 'clock> {
    inner: &'operation dyn HttpTransport,
    operation: &'operation PreparedArchiveOperation<'clock>,
}

impl HttpTransport for ArchiveAdmissionHttp<'_, '_> {
    fn execute(
        &self,
        request: &crate::hosted_runtime::HttpRequest,
    ) -> Result<crate::hosted_runtime::HttpResponse, crate::hosted_runtime::HttpError> {
        self.operation
            .revalidate()
            .map_err(|_| crate::hosted_runtime::HttpError::Timeout)?;
        let remaining = self
            .operation
            .budget()
            .remaining(self.operation.clock())
            .map_err(|_| crate::hosted_runtime::HttpError::Timeout)?;
        let mut request = request.clone();
        request.timeout = request.timeout.min(remaining);
        match self.operation.budget().cancellation() {
            Some(cancellation) => self.inner.execute_cancellable(&request, cancellation),
            None => self
                .inner
                .execute_cancellable(&request, &ArchiveCancellation::new()),
        }
    }
}

struct ArchiveRunner<'operation, 'clock> {
    inner: &'operation dyn ToolRunner,
    operation: &'operation PreparedArchiveOperation<'clock>,
}

impl ToolRunner for ArchiveRunner<'_, '_> {
    fn run(
        &self,
        request: &crate::runner::ToolRequest<'_>,
    ) -> std::io::Result<crate::runner::ToolOutput> {
        self.operation.revalidate().map_err(std::io::Error::other)?;
        let remaining = self
            .operation
            .budget()
            .remaining(self.operation.clock())
            .map_err(std::io::Error::other)?;
        let mut request = request.clone();
        request.timeout = Some(
            request
                .timeout
                .map_or(remaining, |timeout| timeout.min(remaining)),
        );
        match self.operation.budget().cancellation() {
            Some(cancellation) => self.inner.run_cancellable(&request, cancellation),
            None => self.inner.run(&request),
        }
    }
}

/// One operation's retained journal, captured configuration and paired archive
/// transports. Fields and services are private; this is neither serializable
/// evidence nor authority to remove media. Source ownership is the caller's job.
pub struct AdmittedArchive<'a> {
    operation: PreparedArchiveOperation<'a>,
    runner: &'a dyn ToolRunner,
    restic_path: PathBuf,
    write: Runtime,
    read: Runtime,
    repository_id: String,
    effects: ArchiveAdmissionEffects,
    write_repository_id: Option<String>,
    read_repository_id: Option<String>,
    archives: Vec<ArchiveWriterObservation>,
}

fn full_archive_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Actual attempted admission calls, independently of later retained ownership.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArchiveAdmissionEffects {
    pub broker_requested: bool,
    pub write_repository_requested: bool,
    pub read_repository_requested: bool,
}

#[must_use = "Retain the original archive activity after admission refusal"]
pub struct ArchiveAdmissionFailure<'a> {
    inner: Box<ArchiveAdmissionFailureInner<'a>>,
}

struct ArchiveAdmissionFailureInner<'a> {
    error: ArchiveError,
    effects: ArchiveAdmissionEffects,
    owned: ArchiveAdmissionFailureOwner<'a>,
}

enum ArchiveAdmissionFailureOwner<'a> {
    BeforePreparation(Arc<JournalRoot>),
    Prepared(Box<PreparedArchiveOperation<'a>>),
    Preparation(ArchiveOperationPreparationFailure<'a>),
    Readback(Box<ReadbackAdmissionError<'a>>),
    Admitted(Box<AdmittedArchive<'a>>),
}
impl fmt::Debug for ArchiveAdmissionFailure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArchiveAdmissionFailure")
            .field("error", &self.inner.error)
            .field("effects", &self.inner.effects)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for ArchiveAdmissionFailure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.error.fmt(f)
    }
}
impl std::error::Error for ArchiveAdmissionFailure<'_> {}
impl ArchiveAdmissionFailure<'_> {
    pub fn error(&self) -> ArchiveError {
        self.inner.error
    }
    pub fn effects(&self) -> &ArchiveAdmissionEffects {
        &self.inner.effects
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        match &self.inner.owned {
            ArchiveAdmissionFailureOwner::BeforePreparation(root) => root,
            ArchiveAdmissionFailureOwner::Prepared(operation) => operation.journal_root(),
            ArchiveAdmissionFailureOwner::Preparation(preparation) => preparation.journal_root(),
            ArchiveAdmissionFailureOwner::Readback(readback) => readback.journal_root(),
            ArchiveAdmissionFailureOwner::Admitted(admitted) => admitted.journal_root(),
        }
    }
    pub fn carrier(&self) -> Option<&Arc<solstone_core_journal_io::media::RootActivityCarrier>> {
        match &self.inner.owned {
            ArchiveAdmissionFailureOwner::BeforePreparation(_) => None,
            ArchiveAdmissionFailureOwner::Prepared(operation) => Some(operation.carrier()),
            ArchiveAdmissionFailureOwner::Preparation(preparation) => preparation.carrier(),
            ArchiveAdmissionFailureOwner::Readback(readback) => Some(readback.carrier()),
            ArchiveAdmissionFailureOwner::Admitted(admitted) => Some(admitted.carrier()),
        }
    }
    pub fn preparation(&self) -> Option<&ArchiveOperationPreparationFailure<'_>> {
        match &self.inner.owned {
            ArchiveAdmissionFailureOwner::Preparation(preparation) => Some(preparation),
            _ => None,
        }
    }
    pub fn readback(&self) -> Option<&ReadbackAdmissionError<'_>> {
        match &self.inner.owned {
            ArchiveAdmissionFailureOwner::Readback(readback) => Some(readback),
            _ => None,
        }
    }
    /// Close only an actual owned-clock refusal; borrowed owners are returned intact.
    pub fn into_owned_hold(self) -> Result<OwnedArchiveAdmissionFailure, Self> {
        let transferable = match &self.inner.owned {
            ArchiveAdmissionFailureOwner::BeforePreparation(_) => true,
            ArchiveAdmissionFailureOwner::Prepared(operation) => operation.owned_clock().is_some(),
            ArchiveAdmissionFailureOwner::Preparation(preparation) => {
                preparation.owned_clock().is_some()
            }
            ArchiveAdmissionFailureOwner::Readback(readback) => readback.owned_clock().is_some(),
            ArchiveAdmissionFailureOwner::Admitted(admitted) => {
                admitted.operation.owned_clock().is_some()
            }
        };
        if !transferable {
            return Err(self);
        }
        let ArchiveAdmissionFailureInner {
            error,
            effects,
            owned,
        } = *self.inner;
        let owned = match owned {
            ArchiveAdmissionFailureOwner::BeforePreparation(root) => {
                ClosedArchiveAdmissionOwner::BeforePreparation(root)
            }
            ArchiveAdmissionFailureOwner::Prepared(operation) => {
                ClosedArchiveAdmissionOwner::Prepared(Box::new(
                    operation
                        .into_owned_hold()
                        .ok()
                        .expect("checked owned clock"),
                ))
            }
            ArchiveAdmissionFailureOwner::Preparation(preparation) => {
                ClosedArchiveAdmissionOwner::Preparation(Box::new(
                    preparation.into_owned_hold().expect("checked owned clock"),
                ))
            }
            ArchiveAdmissionFailureOwner::Readback(readback) => {
                ClosedArchiveAdmissionOwner::Readback(Box::new(
                    readback.into_owned_hold().expect("checked owned clock"),
                ))
            }
            ArchiveAdmissionFailureOwner::Admitted(admitted) => match admitted.into_owned_hold() {
                Ok(hold) => ClosedArchiveAdmissionOwner::Admitted(Box::new(hold)),
                Err(admitted) => {
                    return Err(Self {
                        inner: Box::new(ArchiveAdmissionFailureInner {
                            error,
                            effects,
                            owned: ArchiveAdmissionFailureOwner::Admitted(admitted),
                        }),
                    });
                }
            },
        };
        Ok(OwnedArchiveAdmissionFailure {
            inner: Box::new(OwnedArchiveAdmissionFailureInner {
                error,
                effects,
                owned,
            }),
        })
    }
}

#[must_use = "Retain every original admission owner after refusal"]
pub struct OwnedArchiveAdmissionFailure {
    inner: Box<OwnedArchiveAdmissionFailureInner>,
}
struct OwnedArchiveAdmissionFailureInner {
    error: ArchiveError,
    effects: ArchiveAdmissionEffects,
    owned: ClosedArchiveAdmissionOwner,
}
enum ClosedArchiveAdmissionOwner {
    BeforePreparation(Arc<JournalRoot>),
    Prepared(Box<OwnedPreparedArchiveHold>),
    Preparation(Box<OwnedArchivePreparationFailure>),
    Readback(Box<OwnedReadbackAdmissionError>),
    Admitted(Box<OwnedAdmittedArchiveHold>),
    Retired(Box<OwnedUnusedArchiveRetirement>),
}
impl OwnedArchiveAdmissionFailure {
    pub fn unused_retirement(&self) -> Option<&OwnedUnusedArchiveRetirement> {
        match &self.inner.owned {
            ClosedArchiveAdmissionOwner::Retired(retirement) => Some(retirement),
            _ => None,
        }
    }
    #[cfg(all(windows, feature = "test-support"))]
    #[doc(hidden)]
    pub fn from_readback_refusal_for_test(readback: OwnedReadbackAdmissionError) -> Self {
        Self {
            inner: Box::new(OwnedArchiveAdmissionFailureInner {
                error: ArchiveError::Readback(readback.reason()),
                effects: ArchiveAdmissionEffects {
                    read_repository_requested: true,
                    ..ArchiveAdmissionEffects::default()
                },
                owned: ClosedArchiveAdmissionOwner::Readback(Box::new(readback)),
            }),
        }
    }

    pub fn error(&self) -> ArchiveError {
        self.inner.error
    }
    pub fn effects(&self) -> &ArchiveAdmissionEffects {
        &self.inner.effects
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        match &self.inner.owned {
            ClosedArchiveAdmissionOwner::BeforePreparation(root) => root,
            ClosedArchiveAdmissionOwner::Prepared(operation) => operation.journal_root(),
            ClosedArchiveAdmissionOwner::Preparation(preparation) => preparation.journal_root(),
            ClosedArchiveAdmissionOwner::Readback(readback) => readback.journal_root(),
            ClosedArchiveAdmissionOwner::Admitted(admitted) => admitted.journal_root(),
            ClosedArchiveAdmissionOwner::Retired(retirement) => retirement.journal_root(),
        }
    }
    pub fn carrier(&self) -> Option<&Arc<solstone_core_journal_io::media::RootActivityCarrier>> {
        match &self.inner.owned {
            ClosedArchiveAdmissionOwner::BeforePreparation(_) => None,
            ClosedArchiveAdmissionOwner::Prepared(operation) => Some(operation.carrier()),
            ClosedArchiveAdmissionOwner::Preparation(preparation) => preparation.carrier(),
            ClosedArchiveAdmissionOwner::Readback(readback) => Some(readback.carrier()),
            ClosedArchiveAdmissionOwner::Admitted(admitted) => Some(admitted.carrier()),
            ClosedArchiveAdmissionOwner::Retired(_) => None,
        }
    }
    pub fn owned_clock(&self) -> Option<&Arc<dyn Clock + Send + Sync>> {
        match &self.inner.owned {
            ClosedArchiveAdmissionOwner::BeforePreparation(_) => None,
            ClosedArchiveAdmissionOwner::Prepared(operation) => Some(operation.owned_clock()),
            ClosedArchiveAdmissionOwner::Preparation(preparation) => {
                Some(preparation.owned_clock())
            }
            ClosedArchiveAdmissionOwner::Readback(readback) => Some(readback.owned_clock()),
            ClosedArchiveAdmissionOwner::Admitted(admitted) => Some(admitted.owned_clock()),
            ClosedArchiveAdmissionOwner::Retired(retirement) => Some(retirement.owned_clock()),
        }
    }
    /// End a refused admission that leaves nothing running or written. Its
    /// broker request already returned; a read whose children are not all
    /// known reaped, or a captured archive of unknown outcome, still holds.
    pub fn release(self) -> Result<(), Box<Self>> {
        let OwnedArchiveAdmissionFailureInner {
            error,
            effects,
            owned,
        } = *self.inner;
        let held = |owned| {
            Box::new(Self {
                inner: Box::new(OwnedArchiveAdmissionFailureInner {
                    error,
                    effects,
                    owned,
                }),
            })
        };
        match owned {
            ClosedArchiveAdmissionOwner::BeforePreparation(_) => Ok(()),
            ClosedArchiveAdmissionOwner::Prepared(operation) => {
                operation.retire_unused().map_err(|retirement| {
                    held(ClosedArchiveAdmissionOwner::Retired(Box::new(retirement)))
                })
            }
            ClosedArchiveAdmissionOwner::Preparation(preparation) => {
                preparation.retire_unused().map_err(|retirement| {
                    held(ClosedArchiveAdmissionOwner::Retired(Box::new(retirement)))
                })
            }
            ClosedArchiveAdmissionOwner::Readback(readback) => {
                readback.release_retained().map_err(|refusal| {
                    held(match refusal {
                        UnusedArchiveRelease::Held(readback) => {
                            ClosedArchiveAdmissionOwner::Readback(readback)
                        }
                        UnusedArchiveRelease::Retired(retirement) => {
                            ClosedArchiveAdmissionOwner::Retired(Box::new(retirement))
                        }
                    })
                })
            }
            ClosedArchiveAdmissionOwner::Admitted(admitted) => {
                admitted.release_retained().map_err(|refusal| {
                    held(match refusal {
                        UnusedArchiveRelease::Held(admitted) => {
                            ClosedArchiveAdmissionOwner::Admitted(admitted)
                        }
                        UnusedArchiveRelease::Retired(retirement) => {
                            ClosedArchiveAdmissionOwner::Retired(Box::new(retirement))
                        }
                    })
                })
            }
            ClosedArchiveAdmissionOwner::Retired(retirement) => {
                Err(held(ClosedArchiveAdmissionOwner::Retired(retirement)))
            }
        }
    }
}

/// Prepare one original activity before admitting paired archive transports.
pub fn admit_archive<'a>(
    root: Arc<JournalRoot>,
    services: &BackupServices<'a>,
) -> Result<AdmittedArchive<'a>, ArchiveAdmissionFailure<'a>> {
    let budget = match ArchiveBudget::new(services.clock) {
        Ok(budget) => budget,
        Err(error) => {
            return Err(ArchiveAdmissionFailure {
                inner: Box::new(ArchiveAdmissionFailureInner {
                    error: error.into(),
                    effects: ArchiveAdmissionEffects::default(),
                    owned: ArchiveAdmissionFailureOwner::BeforePreparation(root),
                }),
            });
        }
    };
    admit_archive_with_budget(root, services, budget)
}

/// Fresh admission only. Already-owned operations use admit_prepared_archive.
pub fn admit_archive_with_budget<'a>(
    root: Arc<JournalRoot>,
    services: &BackupServices<'a>,
    budget: ArchiveBudget,
) -> Result<AdmittedArchive<'a>, ArchiveAdmissionFailure<'a>> {
    let operation =
        match PreparedArchiveOperation::prepare(Arc::clone(&root), budget, services.clock) {
            Ok(operation) => operation,
            Err(preparation) => {
                return Err(ArchiveAdmissionFailure {
                    inner: Box::new(ArchiveAdmissionFailureInner {
                        error: preparation.reason().into(),
                        effects: ArchiveAdmissionEffects::default(),
                        owned: ArchiveAdmissionFailureOwner::Preparation(preparation),
                    }),
                });
            }
        };
    admit_prepared_archive(operation, services)
}

fn capture_byo_archive_pair(
    config: &solstone_core_backup::archive_admission::ArchiveConfiguration,
    keys: BackupKeys,
) -> Result<(Runtime, Runtime), ArchiveError> {
    if config.mode() != Some("byo") {
        return Err(ArchiveError::NotConfigured);
    }
    let destination = config.destination().ok_or(ArchiveError::NotConfigured)?;
    let backend_env = value_env(&destination).ok_or(ArchiveError::ConfigurationUnavailable)?;
    let read = Runtime {
        destination: destination.clone(),
        password: keys.daily_key.clone(),
        backend_env: backend_env.clone(),
        global_options: vec![],
    };
    let write = Runtime {
        destination,
        password: keys.daily_key,
        backend_env,
        global_options: vec![],
    };
    Ok((write, read))
}

/// One broker session, bounded by the operation: the writer reaches operated
/// storage only through the append-only rclone transport, and the reader reads
/// the same repository directly. Returns the credential expiry to cap by.
fn capture_operated_archive_pair(
    operation: &PreparedArchiveOperation<'_>,
    binding: &HostedBinding,
    keys: BackupKeys,
    http: &dyn HttpTransport,
    version: &str,
    rclone: Option<&Path>,
    effects: &mut ArchiveAdmissionEffects,
) -> Result<(Runtime, Runtime, i64), ArchiveError> {
    let http = ArchiveAdmissionHttp {
        inner: http,
        operation,
    };
    effects.broker_requested = true;
    let credentials = fetch_hosted_credentials(&http, binding, "operated", version);
    operation.revalidate()?;
    let credentials = credentials.map_err(|error| ArchiveError::CredentialsUnavailable {
        reason: error.reason_code,
    })?;
    let expiry = chrono::DateTime::parse_from_rfc3339(&credentials.expires_at)
        .map_err(|_| ArchiveError::InvalidCredentialExpiry)?
        .timestamp();
    let rclone = rclone.ok_or(ArchiveError::ToolUnavailable)?;
    let write = hosted_append_only_session(binding, &credentials, rclone)
        .map_err(|_| ArchiveError::ToolUnavailable)?;
    let read = hosted_session(binding, &credentials)
        .map_err(|_| ArchiveError::ConfigurationUnavailable)?;
    let runtime = |session: crate::hosted_runtime::HostedResticSession| Runtime {
        destination: session.destination,
        password: keys.daily_key.clone(),
        backend_env: session
            .backend_env
            .into_iter()
            .map(|(key, value)| (key, Some(value)))
            .collect(),
        global_options: session.global_options,
    };
    Ok((runtime(write), runtime(read), expiry))
}

/// Operated iOS archive reaches storage through the linked create-only session.
/// Both legs are the same hosted session. An rclone path is not required.
#[cfg(target_os = "ios")]
fn capture_linked_operated_pair(
    operation: &PreparedArchiveOperation<'_>,
    binding: &HostedBinding,
    keys: BackupKeys,
    http: &dyn HttpTransport,
    version: &str,
    effects: &mut ArchiveAdmissionEffects,
) -> Result<(Runtime, Runtime, i64), ArchiveError> {
    let http = ArchiveAdmissionHttp {
        inner: http,
        operation,
    };
    effects.broker_requested = true;
    let credentials = fetch_hosted_credentials(&http, binding, "operated", version);
    operation.revalidate()?;
    let credentials = credentials.map_err(|error| ArchiveError::CredentialsUnavailable {
        reason: error.reason_code,
    })?;
    let expiry = chrono::DateTime::parse_from_rfc3339(&credentials.expires_at)
        .map_err(|_| ArchiveError::InvalidCredentialExpiry)?
        .timestamp();
    let write = hosted_session(binding, &credentials)
        .map_err(|_| ArchiveError::ConfigurationUnavailable)?;
    let read = hosted_session(binding, &credentials)
        .map_err(|_| ArchiveError::ConfigurationUnavailable)?;
    let runtime = |session: crate::hosted_runtime::HostedResticSession| Runtime {
        destination: session.destination,
        password: keys.daily_key.clone(),
        backend_env: session
            .backend_env
            .into_iter()
            .map(|(key, value)| (key, Some(value)))
            .collect(),
        global_options: session.global_options,
    };
    Ok((runtime(write), runtime(read), expiry))
}

#[cfg(target_os = "ios")]
fn discover_linked_repository_id(
    authority: &crate::ios_callable::AdmittedIosBackupAuthority,
    runtime: &Runtime,
    operation: &OwnedPreparedArchiveOperation,
) -> Result<String, ArchiveError> {
    if operation
        .budget()
        .cancellation()
        .is_some_and(ArchiveCancellation::is_cancelled)
    {
        return Err(ArchiveError::Cancelled);
    }
    let timeout = operation
        .budget()
        .remaining(operation.owned_clock().as_ref())?;
    let env = if runtime.backend_env.is_empty() {
        None
    } else {
        Some(&runtime.backend_env)
    };
    crate::ios_callable::open_repository_id(
        authority,
        &runtime.destination.repository,
        &runtime.password,
        env,
        Some(timeout),
        operation.budget().cancellation(),
    )
}

/// Consume the original operation without any replaceable lifetime inputs.
pub fn admit_prepared_archive<'a>(
    mut operation: PreparedArchiveOperation<'a>,
    services: &BackupServices<'a>,
) -> Result<AdmittedArchive<'a>, ArchiveAdmissionFailure<'a>> {
    let mut effects = ArchiveAdmissionEffects::default();
    let admission = (|| {
        operation.revalidate()?;
        let config = operation.configuration();
        if !config.enabled() {
            return Err(ArchiveError::NotConfigured);
        }
        let keys = config.keys()?.ok_or(ArchiveError::NotConfigured)?;
        let restic_path = services
            .restic
            .and_then(crate::runner::ResticInvocation::executable_path)
            .ok_or(ArchiveError::ToolUnavailable)?
            .to_path_buf();
        let (write, read) = if let Some(binding) = config.binding() {
            let (write, read, expiry) = capture_operated_archive_pair(
                &operation,
                binding,
                keys,
                services.http,
                services.version,
                services.rclone_path,
                &mut effects,
            )?;
            operation.cap_credential_expiry(expiry);
            operation.budget().remaining(operation.clock())?;
            (write, read)
        } else {
            capture_byo_archive_pair(config, keys)?
        };
        Ok((restic_path, write, read))
    })();
    let (restic_path, write, read) = match admission {
        Ok(admitted) => admitted,
        Err(error) => {
            return Err(ArchiveAdmissionFailure {
                inner: Box::new(ArchiveAdmissionFailureInner {
                    error,
                    effects,
                    owned: ArchiveAdmissionFailureOwner::Prepared(Box::new(operation)),
                }),
            });
        }
    };
    let mut admitted = AdmittedArchive {
        operation,
        runner: services.runner,
        restic_path,
        write,
        read,
        repository_id: String::new(),
        effects,
        write_repository_id: None,
        read_repository_id: None,
        archives: Vec::new(),
    };
    let repositories = (|| {
        effects.write_repository_requested = true;
        let write_id = admitted.read_repository_id(&admitted.write)?;
        admitted.write_repository_id = Some(write_id.clone());
        effects.read_repository_requested = true;
        let read_id = admitted.read_repository_id(&admitted.read)?;
        admitted.read_repository_id = Some(read_id.clone());
        if write_id != read_id {
            return Err(ArchiveError::RepositoryIdentityChanged);
        }
        Ok(write_id)
    })();
    admitted.effects = effects;
    match repositories {
        Ok(repository) => {
            admitted.repository_id = repository;
            Ok(admitted)
        }
        Err(error) => Err(ArchiveAdmissionFailure {
            inner: Box::new(ArchiveAdmissionFailureInner {
                error,
                effects,
                owned: ArchiveAdmissionFailureOwner::Admitted(Box::new(admitted)),
            }),
        }),
    }
}

/// Admit the original operation directly for an existing repository identity.
pub fn admit_prepared_archive_read<'a>(
    operation: PreparedArchiveOperation<'a>,
    expected_repository_id: String,
    services: &BackupServices<'a>,
) -> Result<ReadOperation<'a>, ArchiveAdmissionFailure<'a>> {
    admit_prepared_archive_read_with(
        operation,
        Some(expected_repository_id),
        services.http,
        services
            .restic
            .and_then(crate::runner::ResticInvocation::executable_path),
        services.version,
    )
}

struct CapturedArchiveRead {
    restic_path: PathBuf,
    destination: Destination,
    password: String,
    credential_expiry: Option<i64>,
    #[cfg(target_os = "ios")]
    native: Option<Arc<solstone_core_backup::archive_readback::ios::IosReadContext>>,
}

fn capture_archive_read(
    operation: &PreparedArchiveOperation<'_>,
    expected_repository_id: Option<&str>,
    http: &dyn HttpTransport,
    restic_path: Option<&Path>,
    version: &str,
    effects: &mut ArchiveAdmissionEffects,
) -> Result<CapturedArchiveRead, ArchiveError> {
    if expected_repository_id.is_some_and(|id| !full_archive_id(id)) {
        return Err(ArchiveError::RepositoryIdentityInvalid);
    }
    operation.revalidate()?;
    let config = operation.configuration();
    if !config.enabled() {
        return Err(ArchiveError::NotConfigured);
    }
    let keys = config.keys()?.ok_or(ArchiveError::NotConfigured)?;
    let restic_path = restic_path
        .ok_or(ArchiveError::ToolUnavailable)?
        .to_path_buf();
    let (destination, credential_expiry) = if let Some(binding) = config.binding() {
        let http = ArchiveAdmissionHttp {
            inner: http,
            operation,
        };
        effects.broker_requested = true;
        let credentials = fetch_hosted_credentials(&http, binding, "operated", version);
        operation.revalidate()?;
        let credentials = credentials.map_err(|error| ArchiveError::CredentialsUnavailable {
            reason: error.reason_code,
        })?;
        let expiry = chrono::DateTime::parse_from_rfc3339(&credentials.expires_at)
            .map_err(|_| ArchiveError::InvalidCredentialExpiry)?
            .timestamp();
        let session = hosted_session(binding, &credentials)
            .map_err(|_| ArchiveError::ConfigurationUnavailable)?;
        (session.destination, Some(expiry))
    } else {
        if config.mode() != Some("byo") {
            return Err(ArchiveError::NotConfigured);
        }
        (
            config.destination().ok_or(ArchiveError::NotConfigured)?,
            None,
        )
    };
    Ok(CapturedArchiveRead {
        restic_path,
        destination,
        password: keys.daily_key,
        credential_expiry,
        #[cfg(target_os = "ios")]
        native: None,
    })
}

#[cfg(target_os = "ios")]
fn capture_linked_archive_read(
    operation: &OwnedPreparedArchiveOperation,
    expected_repository_id: Option<&str>,
    authority: &crate::ios_callable::AdmittedIosBackupAuthority,
    http: &dyn HttpTransport,
    version: &str,
    effects: &mut ArchiveAdmissionEffects,
) -> Result<CapturedArchiveRead, ArchiveError> {
    if expected_repository_id.is_some_and(|id| !full_archive_id(id)) {
        return Err(ArchiveError::RepositoryIdentityInvalid);
    }
    operation.revalidate()?;
    authority
        .revalidate_archive_root(operation.carrier().journal_root())
        .map_err(|_| ArchiveError::JournalIdentityChanged)?;
    let config = operation.configuration();
    if !config.enabled() {
        return Err(ArchiveError::NotConfigured);
    }
    let keys = config.keys()?.ok_or(ArchiveError::NotConfigured)?;
    let (read, expiry) = if let Some(binding) = config.binding() {
        if config.mode() != Some("operated") {
            return Err(ArchiveError::NotConfigured);
        }
        // This maintained native pair derives both legs from the same single
        // ordinary hosted session. No writer or new grant is requested here.
        let (_, read, expiry) = capture_linked_operated_pair(
            operation.as_prepared(),
            binding,
            keys,
            http,
            version,
            effects,
        )?;
        (read, Some(expiry))
    } else {
        if config.mode() != Some("byo") || config.destination().is_none_or(|d| d.backend != "s3") {
            return Err(ArchiveError::Readback(ReadbackError::UnsupportedBackend));
        }
        let (_, read) = capture_byo_archive_pair(config, keys)?;
        (read, None)
    };
    operation.revalidate()?;
    let context = authority
        .archive_read_context(&read.destination, &read.password, &read.backend_env)
        .map_err(ArchiveError::Readback)?;
    Ok(CapturedArchiveRead {
        destination: read.destination,
        password: read.password,
        credential_expiry: expiry,
        native: Some(context),
        // Legacy executable field is unused when the actual native context is
        // selected; no path is resolved or accepted as callable authority.
        restic_path: PathBuf::new(),
    })
}

fn admit_prepared_archive_read_with<'a>(
    mut operation: PreparedArchiveOperation<'a>,
    expected_repository_id: Option<String>,
    http: &dyn HttpTransport,
    restic_path: Option<&Path>,
    version: &str,
) -> Result<ReadOperation<'a>, ArchiveAdmissionFailure<'a>> {
    let mut effects = ArchiveAdmissionEffects::default();
    let admission = capture_archive_read(
        &operation,
        expected_repository_id.as_deref(),
        http,
        restic_path,
        version,
        &mut effects,
    )
    .and_then(|captured| {
        if let Some(expiry) = captured.credential_expiry {
            operation.cap_credential_expiry(expiry);
            operation.revalidate()?;
        }
        Ok(captured)
    });
    let captured = match admission {
        Ok(captured) => captured,
        Err(error) => {
            return Err(ArchiveAdmissionFailure {
                inner: Box::new(ArchiveAdmissionFailureInner {
                    error,
                    effects,
                    owned: ArchiveAdmissionFailureOwner::Prepared(Box::new(operation)),
                }),
            });
        }
    };
    let input = match expected_repository_id {
        Some(id) => ReadAdmission::bind(
            operation,
            captured.destination,
            captured.password,
            id,
            captured.restic_path,
        ),
        None => ReadAdmission::capture_current(
            operation,
            captured.destination,
            captured.password,
            captured.restic_path,
        ),
    };
    effects.read_repository_requested = true;
    #[cfg(not(windows))]
    let read = ReadOperation::admit(input);
    #[cfg(windows)]
    let read = ReadOperation::admit(input, Some(Arc::new(crate::runner::SystemToolRunner)));
    read.map_err(|readback| ArchiveAdmissionFailure {
        inner: Box::new(ArchiveAdmissionFailureInner {
            error: ArchiveError::Readback(readback.reason()),
            effects,
            owned: ArchiveAdmissionFailureOwner::Readback(Box::new(readback)),
        }),
    })
}

/// Concrete installed admission with an independent original finish owner.
pub fn admit_prepared_archive_read_installed<'a>(
    operation: PreparedArchiveOperation<'a>,
    expected_repository_id: String,
    restic_path: &Path,
    version: &str,
    http: &UreqHttpTransport,
) -> Result<InstalledArchiveRead<'a>, ArchiveAdmissionFailure<'a>> {
    admit_prepared_archive_read_installed_with(
        operation,
        Some(expected_repository_id),
        restic_path,
        version,
        http,
    )
}

pub fn admit_prepared_archive_current_read_installed<'a>(
    operation: PreparedArchiveOperation<'a>,
    restic_path: &Path,
    version: &str,
    http: &UreqHttpTransport,
) -> Result<InstalledArchiveRead<'a>, ArchiveAdmissionFailure<'a>> {
    admit_prepared_archive_read_installed_with(operation, None, restic_path, version, http)
}

fn admit_prepared_archive_read_installed_with<'a>(
    operation: PreparedArchiveOperation<'a>,
    expected_repository_id: Option<String>,
    restic_path: &Path,
    version: &str,
    http: &UreqHttpTransport,
) -> Result<InstalledArchiveRead<'a>, ArchiveAdmissionFailure<'a>> {
    let Some(clock) = operation.owned_clock().map(Arc::clone) else {
        return Err(ArchiveAdmissionFailure {
            inner: Box::new(ArchiveAdmissionFailureInner {
                error: ArchiveError::Readback(ReadbackError::Unsupported),
                effects: ArchiveAdmissionEffects::default(),
                owned: ArchiveAdmissionFailureOwner::Prepared(Box::new(operation)),
            }),
        });
    };
    let carrier = Arc::clone(operation.carrier());
    let operated = operation.configuration().binding().is_some();
    let read = admit_prepared_archive_read_with(
        operation,
        expected_repository_id,
        http,
        Some(restic_path),
        version,
    )?;
    // Only this concrete call can record the returned, joined Ureq request.
    let broker = if operated {
        DirectBrokerCompletion::JoinedOperated
    } else {
        DirectBrokerCompletion::CapturedByo
    };
    Ok(InstalledArchiveRead {
        read,
        finish: DirectReadFinishOwner {
            carrier,
            clock,
            broker,
        },
    })
}

#[must_use = "Keep the original finish owner outside the consuming actor"]
pub struct InstalledArchiveRead<'a> {
    read: ReadOperation<'a>,
    finish: DirectReadFinishOwner,
}
impl<'a> InstalledArchiveRead<'a> {
    pub fn into_parts(self) -> (ReadOperation<'a>, DirectReadFinishOwner) {
        (self.read, self.finish)
    }
}

pub fn admit_prepared_archive_owned_read_installed<'a>(
    operation: OwnedPreparedArchiveOperation,
    expected_repository_id: String,
    restic: impl Into<crate::runner::ResticInvocation<'a>>,
    version: &str,
    http: &UreqHttpTransport,
) -> Result<OwnedInstalledArchiveRead, OwnedArchiveAdmissionFailure> {
    admit_prepared_archive_owned_read_installed_with(
        operation,
        Some(expected_repository_id),
        restic.into(),
        version,
        http,
    )
}

pub fn admit_prepared_archive_owned_current_read_installed<'a>(
    operation: OwnedPreparedArchiveOperation,
    restic: impl Into<crate::runner::ResticInvocation<'a>>,
    version: &str,
    http: &UreqHttpTransport,
) -> Result<OwnedInstalledArchiveRead, OwnedArchiveAdmissionFailure> {
    admit_prepared_archive_owned_read_installed_with(operation, None, restic.into(), version, http)
}

fn admit_prepared_archive_owned_read_installed_with(
    mut operation: OwnedPreparedArchiveOperation,
    expected_repository_id: Option<String>,
    restic: crate::runner::ResticInvocation<'_>,
    version: &str,
    http: &UreqHttpTransport,
) -> Result<OwnedInstalledArchiveRead, OwnedArchiveAdmissionFailure> {
    let clock = Arc::clone(operation.owned_clock());
    let carrier = Arc::clone(operation.carrier());
    let operated = operation.configuration().binding().is_some();
    let mut effects = ArchiveAdmissionEffects::default();
    let admission = match restic {
        crate::runner::ResticInvocation::Executable(path) => capture_archive_read(
            operation.as_prepared(),
            expected_repository_id.as_deref(),
            http,
            Some(path),
            version,
            &mut effects,
        ),
        #[cfg(target_os = "ios")]
        crate::runner::ResticInvocation::Callable(authority) => capture_linked_archive_read(
            &operation,
            expected_repository_id.as_deref(),
            authority,
            http,
            version,
            &mut effects,
        ),
    }
    .and_then(|captured| {
        if let Some(expiry) = captured.credential_expiry {
            operation.cap_credential_expiry(expiry);
            operation.revalidate()?;
        }
        Ok(captured)
    });
    let captured = match admission {
        Ok(captured) => captured,
        Err(error) => {
            return Err(OwnedArchiveAdmissionFailure {
                inner: Box::new(OwnedArchiveAdmissionFailureInner {
                    error,
                    effects,
                    owned: ClosedArchiveAdmissionOwner::Prepared(Box::new(
                        operation.into_owned_hold(),
                    )),
                }),
            });
        }
    };
    #[cfg(target_os = "ios")]
    let native = captured.native;
    let input = match expected_repository_id {
        Some(id) => operation.bind_read(
            captured.destination,
            captured.password,
            id,
            captured.restic_path,
        ),
        None => operation.capture_current_read(
            captured.destination,
            captured.password,
            captured.restic_path,
        ),
    };
    #[cfg(target_os = "ios")]
    let input = match native {
        Some(context) => input.with_ios_context(context),
        None => input,
    };
    effects.read_repository_requested = true;
    #[cfg(unix)]
    let read = input.admit();
    #[cfg(windows)]
    let read = input.admit(Some(Arc::new(crate::runner::SystemToolRunner)));
    let read = read.map_err(|readback| OwnedArchiveAdmissionFailure {
        inner: Box::new(OwnedArchiveAdmissionFailureInner {
            error: ArchiveError::Readback(readback.reason()),
            effects,
            owned: ClosedArchiveAdmissionOwner::Readback(Box::new(readback)),
        }),
    })?;
    #[cfg(feature = "test-support")]
    crate::test_support::after_owned_read_admission();
    Ok(OwnedInstalledArchiveRead {
        read,
        finish: DirectReadFinishOwner {
            carrier,
            clock,
            broker: if operated {
                DirectBrokerCompletion::JoinedOperated
            } else {
                DirectBrokerCompletion::CapturedByo
            },
        },
    })
}

#[must_use = "Keep the original finish owner outside the consuming actor"]
pub struct OwnedInstalledArchiveRead {
    read: OwnedReadOperation,
    finish: DirectReadFinishOwner,
}
impl OwnedInstalledArchiveRead {
    pub fn into_parts(self) -> (OwnedReadOperation, DirectReadFinishOwner) {
        (self.read, self.finish)
    }
    pub fn into_owned_hold(self) -> (OwnedDirectReadHold, DirectReadFinishOwner) {
        (self.read.into_owned_hold(), self.finish)
    }
}

#[derive(Clone, Copy)]
enum DirectBrokerCompletion {
    CapturedByo,
    JoinedOperated,
}

pub enum RecoveryBindingRefusal {
    Pairing(ReadbackError),
    Read(solstone_core_backup::archive_readback::ArchiveRecoveryReadRefusal),
    Changed,
}
impl std::fmt::Display for RecoveryBindingRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pairing(_) => "original recovery read pairing changed",
            Self::Read(_) => "original recovery read observation refused",
            Self::Changed => "original recovery observations changed",
        })
    }
}
impl std::fmt::Debug for RecoveryBindingRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for RecoveryBindingRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pairing(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Changed => None,
        }
    }
}

/// Original current input. Serialized DATA alone cannot construct this value.
#[must_use = "Retain this input while local effects borrow its original read"]
pub struct QualifiedArchiveRecoveryBinding {
    original: DirectReadFinishOwner,
    data: solstone_core_journal_io::archive_recovery::ArchiveRecoveryBinding,
}
impl QualifiedArchiveRecoveryBinding {
    pub fn data(&self) -> &solstone_core_journal_io::archive_recovery::ArchiveRecoveryBinding {
        &self.data
    }
    pub fn revalidate_for_read(
        &self,
        read: &OwnedReadOperation,
    ) -> Result<(), RecoveryBindingRefusal> {
        self.original
            .validate_identity(read.carrier(), Some(read.owned_clock()))
            .map_err(RecoveryBindingRefusal::Pairing)?;
        let current = read
            .observe_recovery_binding()
            .map_err(RecoveryBindingRefusal::Read)?;
        if current != self.data {
            return Err(RecoveryBindingRefusal::Changed);
        }
        Ok(())
    }
}

/// Outside custody for the actual root/carrier and original owned clock.
#[must_use = "Keep this owner outside the actor unwind boundary"]
pub struct DirectReadFinishOwner {
    carrier: Arc<solstone_core_journal_io::media::RootActivityCarrier>,
    clock: Arc<dyn Clock + Send + Sync>,
    broker: DirectBrokerCompletion,
}
impl DirectReadFinishOwner {
    /// Binds forward observations to this original installed read and broker owner.
    /// No recovery, settlement or retirement follows from minting this input.
    pub fn observe_recovery_binding(
        &self,
        read: &OwnedReadOperation,
    ) -> Result<QualifiedArchiveRecoveryBinding, RecoveryBindingRefusal> {
        self.validate_identity(read.carrier(), Some(read.owned_clock()))
            .map_err(RecoveryBindingRefusal::Pairing)?;
        match self.broker {
            DirectBrokerCompletion::CapturedByo | DirectBrokerCompletion::JoinedOperated => {}
        }
        let data = read
            .observe_recovery_binding()
            .map_err(RecoveryBindingRefusal::Read)?;
        Ok(QualifiedArchiveRecoveryBinding {
            original: DirectReadFinishOwner {
                carrier: Arc::clone(&self.carrier),
                clock: Arc::clone(&self.clock),
                broker: self.broker,
            },
            data,
        })
    }
    pub fn carrier(&self) -> &Arc<solstone_core_journal_io::media::RootActivityCarrier> {
        &self.carrier
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.carrier.journal_root()
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        &self.clock
    }

    /// Actor unwind leaves lost resources unresolved; this value can never retire.
    pub fn into_unknown(self) -> OwnedDirectReadFinish {
        OwnedDirectReadFinish {
            inner: Box::new(OwnedDirectReadFinishInner::Unknown(self)),
        }
    }

    fn validate_identity(
        &self,
        carrier: &Arc<solstone_core_journal_io::media::RootActivityCarrier>,
        clock: Option<&Arc<dyn Clock + Send + Sync>>,
    ) -> Result<(), ReadbackError> {
        if !Arc::ptr_eq(&self.carrier, carrier)
            || !clock.is_some_and(|clock| Arc::ptr_eq(&self.clock, clock))
        {
            return Err(ReadbackError::NativeFailure);
        }
        Ok(())
    }

    pub fn finish<'a>(self, settled: SettledDirectRead<'a>) -> DirectReadFinish<'a> {
        let validation = self
            .validate_identity(settled.carrier(), settled.owned_clock())
            .and_then(|()| settled.revalidate());
        if let Err(reason) = validation {
            return DirectReadFinish {
                inner: Box::new(DirectReadFinishInner::BeforeRelease {
                    owner: self,
                    settled: Box::new(settled),
                    reason,
                }),
            };
        }
        DirectReadFinish {
            inner: Box::new(DirectReadFinishInner::Owned(
                self.finish_released(settled.release()),
            )),
        }
    }

    pub fn finish_owned(self, settled: OwnedSettledDirectRead) -> OwnedDirectReadFinish {
        let validation = self
            .validate_identity(settled.carrier(), Some(settled.owned_clock()))
            .and_then(|()| settled.revalidate());
        if let Err(reason) = validation {
            return OwnedDirectReadFinish {
                inner: Box::new(OwnedDirectReadFinishInner::BeforeRelease {
                    owner: self,
                    _read: Box::new(settled.into_owned_hold()),
                    reason,
                }),
            };
        }
        self.finish_released(settled.release())
    }

    fn finish_released(self, completion: DirectReadCompletion) -> OwnedDirectReadFinish {
        if let Err(reason) = completion.revalidate() {
            return OwnedDirectReadFinish {
                inner: Box::new(OwnedDirectReadFinishInner::Shared {
                    owner: self,
                    _completion: completion,
                    reason,
                }),
            };
        }
        let Self {
            carrier,
            clock,
            broker,
        } = self;
        let inner = match Arc::try_unwrap(carrier) {
            Err(carrier) => OwnedDirectReadFinishInner::Shared {
                owner: Self {
                    carrier,
                    clock,
                    broker,
                },
                _completion: completion,
                reason: ReadbackError::NativeFailure,
            },
            Ok(carrier) => {
                let retirement = carrier.retire_after_settlement();
                let lifetime_failure = completion.revalidate().err();
                OwnedDirectReadFinishInner::Retired {
                    retirement,
                    clock,
                    _broker: broker,
                    _completion: completion,
                    lifetime_failure,
                }
            }
        };
        OwnedDirectReadFinish {
            inner: Box::new(inner),
        }
    }

    /// After its operation was released with nothing outstanding, retire the
    /// carrier if no other owner shares it. Anything left is retired by the
    /// next admission.
    pub fn release_unused(self) {
        if let Ok(carrier) = Arc::try_unwrap(self.carrier) {
            drop(carrier.retire_after_settlement());
        }
    }

    /// Retire the empty carrier left by a refused read whose children were all
    /// reaped. The caller has removed its own stages. This never completes.
    pub fn finish_refused(self, read: RefusedDirectRead) -> OwnedDirectReadFinish {
        let reason = read.reason();
        if !Arc::ptr_eq(self.carrier.journal_root(), read.journal_root())
            || !Arc::ptr_eq(&self.clock, read.owned_clock())
        {
            return OwnedDirectReadFinish {
                inner: Box::new(OwnedDirectReadFinishInner::Refused {
                    owner: self,
                    _read: read,
                    reason,
                }),
            };
        }
        let Self {
            carrier,
            clock,
            broker,
        } = self;
        let inner = match Arc::try_unwrap(carrier) {
            Err(carrier) => OwnedDirectReadFinishInner::Refused {
                owner: Self {
                    carrier,
                    clock,
                    broker,
                },
                _read: read,
                reason,
            },
            Ok(carrier) => OwnedDirectReadFinishInner::Released {
                retirement: carrier.retire_after_settlement(),
                clock,
                _broker: broker,
                _read: read,
                reason,
            },
        };
        OwnedDirectReadFinish {
            inner: Box::new(inner),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectReadFinishState {
    Completed,
    Held,
    Unknown,
}

#[must_use = "Retain the complete owning finish result through projection"]
pub struct DirectReadFinish<'a> {
    inner: Box<DirectReadFinishInner<'a>>,
}
enum DirectReadFinishInner<'a> {
    BeforeRelease {
        owner: DirectReadFinishOwner,
        settled: Box<SettledDirectRead<'a>>,
        reason: ReadbackError,
    },
    Owned(OwnedDirectReadFinish),
}
impl DirectReadFinish<'_> {
    pub fn state(&self) -> DirectReadFinishState {
        match &*self.inner {
            DirectReadFinishInner::BeforeRelease { .. } => DirectReadFinishState::Held,
            DirectReadFinishInner::Owned(owned) => owned.state(),
        }
    }
    pub fn into_owned_hold(self) -> Result<OwnedDirectReadFinish, Self> {
        match *self.inner {
            DirectReadFinishInner::Owned(owned) => Ok(owned),
            DirectReadFinishInner::BeforeRelease {
                owner,
                settled,
                reason,
            } => match (*settled).into_owned_hold() {
                Ok(read) => Ok(OwnedDirectReadFinish {
                    inner: Box::new(OwnedDirectReadFinishInner::BeforeRelease {
                        owner,
                        _read: Box::new(read),
                        reason,
                    }),
                }),
                Err(settled) => Err(Self {
                    inner: Box::new(DirectReadFinishInner::BeforeRelease {
                        owner,
                        settled,
                        reason,
                    }),
                }),
            },
        }
    }
}

#[must_use = "Closed owning custody; state observation does not release it"]
pub struct OwnedDirectReadFinish {
    inner: Box<OwnedDirectReadFinishInner>,
}
enum OwnedDirectReadFinishInner {
    Unknown(DirectReadFinishOwner),
    BeforeRelease {
        owner: DirectReadFinishOwner,
        _read: Box<OwnedDirectReadHold>,
        reason: ReadbackError,
    },
    Shared {
        owner: DirectReadFinishOwner,
        _completion: DirectReadCompletion,
        reason: ReadbackError,
    },
    Retired {
        retirement: solstone_core_journal_io::media::ActivityCarrierRetirement<
            solstone_core_journal_io::media::RootActivityCarrier,
        >,
        clock: Arc<dyn Clock + Send + Sync>,
        _broker: DirectBrokerCompletion,
        _completion: DirectReadCompletion,
        lifetime_failure: Option<ReadbackError>,
    },
    Refused {
        owner: DirectReadFinishOwner,
        _read: RefusedDirectRead,
        reason: ReadbackError,
    },
    Released {
        retirement: solstone_core_journal_io::media::ActivityCarrierRetirement<
            solstone_core_journal_io::media::RootActivityCarrier,
        >,
        clock: Arc<dyn Clock + Send + Sync>,
        _broker: DirectBrokerCompletion,
        _read: RefusedDirectRead,
        reason: ReadbackError,
    },
}
impl OwnedDirectReadFinish {
    pub fn state(&self) -> DirectReadFinishState {
        use solstone_core_journal_io::media::{
            ActivityCarrierRetirementEffect, Confirmation, Durability,
        };
        match &*self.inner {
            OwnedDirectReadFinishInner::Unknown(_) => DirectReadFinishState::Unknown,
            OwnedDirectReadFinishInner::Retired {
                retirement,
                lifetime_failure: None,
                ..
            } if retirement.receipt.effect() == ActivityCarrierRetirementEffect::Removed
                && retirement.receipt.root_durability()
                    == if cfg!(windows) {
                        Durability::Unproven
                    } else {
                        Durability::Synced
                    }
                && retirement.receipt.confirmation() == Confirmation::Confirmed
                && retirement.receipt.failures().is_empty() =>
            {
                DirectReadFinishState::Completed
            }
            _ => DirectReadFinishState::Held,
        }
    }
    /// A refused read ended and its carrier removal was confirmed. This is
    /// never completion; it only means nothing of this operation remains.
    pub fn released(&self) -> bool {
        use solstone_core_journal_io::media::{
            ActivityCarrierRetirementEffect, Confirmation, Durability,
        };
        matches!(&*self.inner, OwnedDirectReadFinishInner::Released { retirement, .. }
            if retirement.receipt.effect() == ActivityCarrierRetirementEffect::Removed
                && retirement.receipt.root_durability()
                    == if cfg!(windows) { Durability::Unproven } else { Durability::Synced }
                && retirement.receipt.confirmation() == Confirmation::Confirmed
                && retirement.receipt.failures().is_empty())
    }
    pub fn reason(&self) -> Option<ReadbackError> {
        match &*self.inner {
            OwnedDirectReadFinishInner::BeforeRelease { reason, .. } => Some(*reason),
            OwnedDirectReadFinishInner::Shared { reason, .. } => Some(*reason),
            OwnedDirectReadFinishInner::Refused { reason, .. }
            | OwnedDirectReadFinishInner::Released { reason, .. } => Some(*reason),
            OwnedDirectReadFinishInner::Retired {
                lifetime_failure: Some(reason),
                ..
            } => Some(*reason),
            OwnedDirectReadFinishInner::Retired { .. }
                if self.state() == DirectReadFinishState::Held =>
            {
                Some(ReadbackError::NativeFailure)
            }
            _ => None,
        }
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        match &*self.inner {
            OwnedDirectReadFinishInner::Unknown(owner)
            | OwnedDirectReadFinishInner::BeforeRelease { owner, .. }
            | OwnedDirectReadFinishInner::Shared { owner, .. }
            | OwnedDirectReadFinishInner::Refused { owner, .. } => owner.journal_root(),
            OwnedDirectReadFinishInner::Retired { retirement, .. }
            | OwnedDirectReadFinishInner::Released { retirement, .. } => {
                retirement.owned.journal_root()
            }
        }
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        match &*self.inner {
            OwnedDirectReadFinishInner::Unknown(owner)
            | OwnedDirectReadFinishInner::BeforeRelease { owner, .. }
            | OwnedDirectReadFinishInner::Shared { owner, .. }
            | OwnedDirectReadFinishInner::Refused { owner, .. } => owner.owned_clock(),
            OwnedDirectReadFinishInner::Retired { clock, .. }
            | OwnedDirectReadFinishInner::Released { clock, .. } => clock,
        }
    }
    pub fn retirement(
        &self,
    ) -> Option<
        &solstone_core_journal_io::media::ActivityCarrierRetirement<
            solstone_core_journal_io::media::RootActivityCarrier,
        >,
    > {
        match &*self.inner {
            OwnedDirectReadFinishInner::Retired { retirement, .. }
            | OwnedDirectReadFinishInner::Released { retirement, .. } => Some(retirement),
            _ => None,
        }
    }
}

struct ArchiveCaptureFacts {
    effects: ArchiveAdmissionEffects,
    write_repository_id: Option<String>,
    read_repository_id: Option<String>,
    repository_id: Option<String>,
    archives: Vec<ArchiveWriterObservation>,
}
fn capture_archive_predecessor(
    restic_path: PathBuf,
    write: Runtime,
    read: Runtime,
    facts: ArchiveCaptureFacts,
) -> ArchivePredecessorCapture {
    let endpoint = |runtime: Runtime| ArchiveEndpointCapture {
        destination: runtime.destination,
        password: runtime.password,
        backend_env: runtime.backend_env,
        global_options: runtime.global_options,
    };
    ArchivePredecessorCapture {
        restic_path,
        write: endpoint(write),
        read: endpoint(read),
        broker_requested: facts.effects.broker_requested,
        write_repository_requested: facts.effects.write_repository_requested,
        read_repository_requested: facts.effects.read_repository_requested,
        write_repository_id: facts.write_repository_id,
        read_repository_id: facts.read_repository_id,
        repository_id: facts.repository_id,
        archives: facts.archives,
        #[cfg(target_os = "ios")]
        linked: false,
    }
}

#[must_use = "Closed archive custody cannot execute or reopen the operation"]
pub struct OwnedAdmittedArchiveHold {
    inner: Box<OwnedAdmittedArchiveHoldInner>,
}
enum OwnedAdmittedArchiveHoldInner {
    Captured(Box<OwnedCapturedArchive>),
    Direct(Box<OwnedDirectReadHold>),
}
struct OwnedCapturedArchive {
    operation: OwnedPreparedArchiveHold,
    _capture: ArchivePredecessorCapture,
}
impl OwnedAdmittedArchiveHold {
    pub fn carrier(&self) -> &Arc<solstone_core_journal_io::media::RootActivityCarrier> {
        match &*self.inner {
            OwnedAdmittedArchiveHoldInner::Captured(capture) => capture.operation.carrier(),
            OwnedAdmittedArchiveHoldInner::Direct(read) => read.carrier(),
        }
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.carrier().journal_root()
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        match &*self.inner {
            OwnedAdmittedArchiveHoldInner::Captured(capture) => capture.operation.owned_clock(),
            OwnedAdmittedArchiveHoldInner::Direct(read) => read.owned_clock(),
        }
    }
    /// End an admitted creator that never submitted an archive writer and whose
    /// every child was reaped. A captured runner archive holds: its children
    /// carry no reap evidence.
    pub fn release_unused(self) -> Result<(), Box<Self>> {
        match *self.inner {
            OwnedAdmittedArchiveHoldInner::Direct(read) => {
                (*read).release_unused().map_err(|read| {
                    Box::new(Self {
                        inner: Box::new(OwnedAdmittedArchiveHoldInner::Direct(read)),
                    })
                })
            }
            inner => Err(Box::new(Self {
                inner: Box::new(inner),
            })),
        }
    }

    fn release_retained(self) -> Result<(), UnusedArchiveRelease<Box<Self>>> {
        match *self.inner {
            OwnedAdmittedArchiveHoldInner::Direct(read) => {
                (*read).release_retained().map_err(|refusal| match refusal {
                    UnusedArchiveRelease::Held(read) => {
                        UnusedArchiveRelease::Held(Box::new(Self {
                            inner: Box::new(OwnedAdmittedArchiveHoldInner::Direct(read)),
                        }))
                    }
                    UnusedArchiveRelease::Retired(retirement) => {
                        UnusedArchiveRelease::Retired(retirement)
                    }
                })
            }
            inner => Err(UnusedArchiveRelease::Held(Box::new(Self {
                inner: Box::new(inner),
            }))),
        }
    }

    /// Return a quiescent refused direct read to its original finish owner.
    pub fn release_refused(self) -> Result<RefusedDirectRead, Box<Self>> {
        match *self.inner {
            OwnedAdmittedArchiveHoldInner::Direct(read) => {
                (*read).release_refused().map_err(|read| {
                    Box::new(Self {
                        inner: Box::new(OwnedAdmittedArchiveHoldInner::Direct(read)),
                    })
                })
            }
            inner => Err(Box::new(Self {
                inner: Box::new(inner),
            })),
        }
    }
}

#[must_use = "Keep the original finish owner outside the creation actor"]
pub struct OwnedInstalledArchiveCreator {
    archive: OwnedAdmittedArchive,
    finish: DirectReadFinishOwner,
}
impl OwnedInstalledArchiveCreator {
    pub fn into_parts(self) -> (OwnedAdmittedArchive, DirectReadFinishOwner) {
        (self.archive, self.finish)
    }
    pub fn into_owned_hold(self) -> (OwnedAdmittedArchiveHold, DirectReadFinishOwner) {
        (self.archive.into_owned_hold(), self.finish)
    }
}

#[must_use = "Consume the same creator into read handoff or closed custody"]
pub struct OwnedAdmittedArchive {
    creator: OwnedArchiveCreator,
    #[cfg(target_os = "ios")]
    linked_authority: Option<crate::ios_callable::AdmittedIosBackupAuthority>,
}
impl OwnedAdmittedArchive {
    pub fn prepared_operation(&self) -> &PreparedArchiveOperation<'static> {
        self.creator.prepared_operation()
    }
    pub fn carrier(&self) -> &Arc<solstone_core_journal_io::media::RootActivityCarrier> {
        self.creator.carrier()
    }
    pub fn activity(&self) -> &Arc<solstone_core_journal_io::activity::ActivityLease> {
        self.creator.activity()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.creator.journal_root()
    }
    pub fn repository_id(&self) -> &str {
        self.creator.repository_id()
    }
    #[cfg(target_os = "ios")]
    pub(crate) fn linked_creator(
        &self,
    ) -> &solstone_core_backup::archive_readback::OwnedArchiveCreator {
        &self.creator
    }
    #[cfg(target_os = "ios")]
    pub(crate) fn linked_creator_mut(
        &mut self,
    ) -> &mut solstone_core_backup::archive_readback::OwnedArchiveCreator {
        &mut self.creator
    }
    pub fn current_time(&self) -> Result<i64, ArchiveError> {
        self.creator.current_time().map_err(ArchiveError::Readback)
    }
    pub fn revalidate_local(&self) -> Result<(), ArchiveError> {
        self.creator
            .revalidate_local()
            .map_err(ArchiveError::Readback)
    }
    pub fn archive_owned(
        &mut self,
        sources: &[ArchiveSource<'_>],
    ) -> Result<ArchiveSnapshot, ArchiveFailure> {
        #[cfg(target_os = "ios")]
        if let Some(authority) = self.linked_authority.clone() {
            // One stable logical directory in the snapshot; source basenames,
            // identities and bytes still come from the retained staging owner.
            return self.archive_retained_sources(&authority, "archive", sources);
        }
        self.creator
            .archive_owned(sources)
            .map(|snapshot| ArchiveSnapshot {
                repository_id: snapshot.repository_id,
                snapshot_id: snapshot.snapshot_id,
            })
            .map_err(|failure| ArchiveFailure {
                error: match failure.reason {
                    ReadbackError::Admission(reason) => reason.into(),
                    ReadbackError::NativeFailure => ArchiveError::SourceUnavailable,
                    ReadbackError::InvalidSelection => ArchiveError::InvalidSelection,
                    ReadbackError::RepositoryIdentityChanged => {
                        ArchiveError::RepositoryIdentityChanged
                    }
                    ReadbackError::SnapshotNodeUnavailable => ArchiveError::SnapshotIdentityInvalid,
                    reason => ArchiveError::Readback(reason),
                },
                remote_effects: match failure.effects {
                    DirectArchiveEffects::NotAttempted => ArchiveRemoteEffects::NotAttempted,
                    DirectArchiveEffects::Attempted {
                        returncode,
                        observed_snapshot_id,
                    } => ArchiveRemoteEffects::Attempted {
                        returncode,
                        observed_snapshot_id,
                    },
                },
            })
    }
    pub fn into_read_operation(self) -> Result<OwnedReadOperation, OwnedArchiveAdmissionFailure> {
        self.creator
            .into_read_operation()
            .map_err(creator_admission_failure)
    }
    pub fn into_owned_hold(self) -> OwnedAdmittedArchiveHold {
        OwnedAdmittedArchiveHold {
            inner: Box::new(OwnedAdmittedArchiveHoldInner::Direct(Box::new(
                self.creator.into_owned_hold(),
            ))),
        }
    }
}

fn creator_admission_failure(
    readback: OwnedReadbackAdmissionError,
) -> OwnedArchiveAdmissionFailure {
    let (write_repository_requested, read_repository_requested) = readback
        .archive_admission_effects()
        .unwrap_or((false, false));
    OwnedArchiveAdmissionFailure {
        inner: Box::new(OwnedArchiveAdmissionFailureInner {
            error: ArchiveError::Readback(readback.reason()),
            effects: ArchiveAdmissionEffects {
                broker_requested: readback.archive_broker_requested(),
                write_repository_requested,
                read_repository_requested,
            },
            owned: ClosedArchiveAdmissionOwner::Readback(Box::new(readback)),
        }),
    }
}

/// Operated journals also pass the installed rclone, the only operated writer
/// transport, and the broker transport that issues this operation's session.
/// A linked iOS authority is admitted without an rclone executable. Existing
/// executable paths stay executable-only.
pub fn admit_prepared_archive_owned_creator_installed<'a>(
    mut operation: OwnedPreparedArchiveOperation,
    restic: impl Into<crate::runner::ResticInvocation<'a>>,
    rclone_path: Option<&Path>,
    version: &str,
    http: &UreqHttpTransport,
) -> Result<OwnedInstalledArchiveCreator, OwnedArchiveAdmissionFailure> {
    let invocation = restic.into();
    #[cfg(target_os = "ios")]
    let linked_authority = match invocation {
        crate::runner::ResticInvocation::Callable(authority) => Some(authority.clone()),
        crate::runner::ResticInvocation::Executable(_) => None,
    };
    let mut effects = ArchiveAdmissionEffects::default();
    let operated = operation.configuration().binding().is_some();
    let captured = (|| {
        operation.revalidate()?;
        #[cfg(target_os = "ios")]
        if let Some(authority) = &linked_authority {
            authority
                .revalidate_archive_root(operation.carrier().journal_root())
                .map_err(|_| ArchiveError::JournalIdentityChanged)?;
        }
        let config = operation.configuration();
        if !config.enabled() {
            return Err(ArchiveError::NotConfigured);
        }
        let keys = config.keys()?.ok_or(ArchiveError::NotConfigured)?;
        crate::check_archive_creator_support(
            operation.carrier().journal_root(),
            config,
            invocation,
            rclone_path,
        )?;
        #[cfg(not(target_os = "ios"))]
        let restic_owned = invocation
            .executable_path()
            .ok_or(ArchiveError::ToolUnavailable)?
            .to_path_buf();
        #[cfg(target_os = "ios")]
        let (restic_owned, linked) = match invocation {
            crate::runner::ResticInvocation::Executable(path) => (path.to_path_buf(), None),
            crate::runner::ResticInvocation::Callable(authority) => {
                (PathBuf::new(), Some(authority))
            }
        };
        if let Some(binding) = config.binding() {
            #[cfg(target_os = "ios")]
            if let Some(authority) = linked {
                let (write, read, expiry) = capture_linked_operated_pair(
                    operation.as_prepared(),
                    binding,
                    keys,
                    http,
                    version,
                    &mut effects,
                )?;
                operation.cap_credential_expiry(expiry);
                operation.revalidate()?;
                let repository_id = discover_linked_repository_id(authority, &read, &operation)?;
                operation.revalidate()?;
                return Ok((restic_owned, write, read, Some(repository_id), true));
            }
            // The shared support check has admitted the append-only transport
            // before this first broker request.
            let (write, read, expiry) = capture_operated_archive_pair(
                operation.as_prepared(),
                binding,
                keys,
                http,
                version,
                rclone_path,
                &mut effects,
            )?;
            operation.cap_credential_expiry(expiry);
            operation.revalidate()?;
            return Ok((restic_owned, write, read, None, false));
        }
        #[cfg(target_os = "ios")]
        if let Some(authority) = linked {
            let (write, read) = capture_byo_archive_pair(config, keys)?;
            let repository_id = discover_linked_repository_id(authority, &read, &operation)?;
            operation.revalidate()?;
            return Ok((restic_owned, write, read, Some(repository_id), true));
        }
        let (write, read) = capture_byo_archive_pair(config, keys)?;
        Ok((restic_owned, write, read, None, false))
    })();
    let (restic_owned, write, read, repository_id, linked) = match captured {
        Ok(admitted) => admitted,
        Err(error) => {
            return Err(OwnedArchiveAdmissionFailure {
                inner: Box::new(OwnedArchiveAdmissionFailureInner {
                    error,
                    effects,
                    owned: ClosedArchiveAdmissionOwner::Prepared(Box::new(
                        operation.into_owned_hold(),
                    )),
                }),
            });
        }
    };
    let clock = Arc::clone(operation.owned_clock());
    let carrier = Arc::clone(operation.carrier());
    #[cfg(target_os = "ios")]
    let linked_read_context = match linked_authority.as_ref() {
        Some(authority) => match authority.archive_read_context(
            &read.destination,
            &read.password,
            &read.backend_env,
        ) {
            Ok(context) => Some(context),
            Err(reason) => {
                return Err(OwnedArchiveAdmissionFailure {
                    inner: Box::new(OwnedArchiveAdmissionFailureInner {
                        error: ArchiveError::Readback(reason),
                        effects,
                        owned: ClosedArchiveAdmissionOwner::Prepared(Box::new(
                            operation.into_owned_hold(),
                        )),
                    }),
                });
            }
        },
        None => None,
    };
    let capture = capture_archive_predecessor(
        restic_owned,
        write,
        read,
        ArchiveCaptureFacts {
            effects,
            write_repository_id: None,
            read_repository_id: None,
            repository_id,
            archives: Vec::new(),
        },
    );
    #[cfg(target_os = "ios")]
    let mut capture = capture;
    #[cfg(target_os = "ios")]
    {
        capture.linked = linked;
    }
    #[cfg(not(target_os = "ios"))]
    let _ = linked;
    let admission = operation.bind_archive_creator(capture);
    #[cfg(target_os = "ios")]
    let admission = match linked_read_context {
        Some(context) => admission.with_ios_context(context),
        None => admission,
    };
    #[cfg(unix)]
    let creator = admission.admit().map_err(creator_admission_failure)?;
    #[cfg(windows)]
    let creator = admission
        .admit(Some(Arc::new(crate::runner::SystemToolRunner)))
        .map_err(creator_admission_failure)?;
    Ok(OwnedInstalledArchiveCreator {
        archive: OwnedAdmittedArchive {
            creator,
            #[cfg(target_os = "ios")]
            linked_authority,
        },
        finish: DirectReadFinishOwner {
            carrier,
            clock,
            broker: if operated {
                DirectBrokerCompletion::JoinedOperated
            } else {
                DirectBrokerCompletion::CapturedByo
            },
        },
    })
}

impl<'a> AdmittedArchive<'a> {
    pub fn prepared_operation(&self) -> &PreparedArchiveOperation<'a> {
        &self.operation
    }

    pub fn into_owned_hold(self) -> Result<OwnedAdmittedArchiveHold, Box<Self>> {
        let Self {
            operation,
            runner,
            restic_path,
            write,
            read,
            repository_id,
            effects,
            write_repository_id,
            read_repository_id,
            archives,
        } = self;
        match operation.into_owned_hold() {
            Ok(operation) => {
                let capture = capture_archive_predecessor(
                    restic_path,
                    write,
                    read,
                    ArchiveCaptureFacts {
                        effects,
                        write_repository_id,
                        read_repository_id,
                        repository_id: Some(repository_id),
                        archives,
                    },
                );
                Ok(OwnedAdmittedArchiveHold {
                    inner: Box::new(OwnedAdmittedArchiveHoldInner::Captured(Box::new(
                        OwnedCapturedArchive {
                            operation,
                            _capture: capture,
                        },
                    ))),
                })
            }
            Err(operation) => Err(Box::new(Self {
                operation: *operation,
                runner,
                restic_path,
                write,
                read,
                repository_id,
                effects,
                write_repository_id,
                read_repository_id,
                archives,
            })),
        }
    }

    /// Move this operation's original configuration, credentials and budget into
    /// the concrete installed-tool readback door without renewing its lifetime.
    pub fn into_read_operation(self) -> Result<ReadOperation<'a>, ReadbackAdmissionError<'a>> {
        let unsupported = !self.read.global_options.is_empty();
        let destination = self.read.destination.clone();
        let password = self.read.password.clone();
        let repository_id = self.repository_id.clone();
        let restic_path = self.restic_path.clone();
        let Self {
            operation,
            runner: _,
            restic_path: captured_path,
            write,
            read,
            repository_id: captured_id,
            effects,
            write_repository_id,
            read_repository_id,
            archives,
        } = self;
        let capture = capture_archive_predecessor(
            captured_path,
            write,
            read,
            ArchiveCaptureFacts {
                effects,
                write_repository_id,
                read_repository_id,
                repository_id: Some(captured_id),
                archives,
            },
        );
        let input =
            ReadAdmission::bind(operation, destination, password, repository_id, restic_path)
                .retain_unqualified_archive(capture);
        if unsupported {
            return Err(input.refuse(ReadbackError::UnsupportedBackend));
        }
        #[cfg(windows)]
        return ReadOperation::admit(input, Some(Arc::new(crate::runner::SystemToolRunner)));
        #[cfg(not(windows))]
        ReadOperation::admit(input)
    }

    pub fn carrier(&self) -> &Arc<solstone_core_journal_io::media::RootActivityCarrier> {
        self.operation.carrier()
    }
    pub fn activity(&self) -> &Arc<solstone_core_journal_io::activity::ActivityLease> {
        self.operation.activity()
    }

    pub fn repository_id(&self) -> &str {
        &self.repository_id
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.operation.journal_root()
    }

    /// Sample the admitted clock after checking the original local lifetime.
    /// This observation cannot extend the operation's deadline or credentials.
    pub fn current_time(&self) -> Result<i64, ArchiveError> {
        self.revalidate_local()?;
        Ok(self.operation.clock().now_unix())
    }

    /// Recheck the retained root, captured backup configuration and remaining
    /// lifetime without issuing any remote request.
    pub fn revalidate_local(&self) -> Result<(), ArchiveError> {
        self.operation.revalidate()?;
        Ok(())
    }

    fn read_repository_id(&self, runtime: &Runtime) -> Result<String, ArchiveError> {
        self.revalidate_local()?;
        let timeout = self
            .operation
            .budget()
            .remaining(self.operation.clock())?
            .min(Duration::from_secs(ARCHIVE_LS_TIMEOUT_SECONDS));
        let mut args = runtime.global_options.clone();
        args.extend(["cat".into(), "config".into(), "--no-cache".into()]);
        let runner = ArchiveRunner {
            inner: self.runner,
            operation: &self.operation,
        };
        let output = run_restic(
            &runner,
            &args,
            &runtime.destination.repository,
            &runtime.password,
            &self.restic_path,
            Some(&runtime.backend_env),
            false,
            None,
            Some(timeout),
            &[],
        );
        self.revalidate_local()?;
        let output = output.map_err(|_| ArchiveError::ToolFailed { reason: "failed" })?;
        if output.returncode != 0 {
            return Err(ArchiveError::ToolFailed {
                reason: reason_for_returncode(output.returncode),
            });
        }
        #[derive(serde::Deserialize)]
        struct RepositoryConfig {
            id: String,
        }
        let config: RepositoryConfig = serde_json::from_str(&output.stdout)
            .map_err(|_| ArchiveError::RepositoryIdentityInvalid)?;
        if !full_archive_id(&config.id) {
            return Err(ArchiveError::RepositoryIdentityInvalid);
        }
        Ok(config.id)
    }

    /// Recheck both transports against the originally admitted repository.
    pub fn revalidate(&self) -> Result<(), ArchiveError> {
        if self.read_repository_id(&self.write)? != self.repository_id
            || self.read_repository_id(&self.read)? != self.repository_id
        {
            return Err(ArchiveError::RepositoryIdentityChanged);
        }
        Ok(())
    }

    /// Archive caller-owned staged paths with forced source rereading. This does
    /// not establish source immutability or verify recoverable file contents.
    pub fn archive(&mut self, paths: &[PathBuf]) -> Result<ArchiveSnapshot, ArchiveFailure> {
        if paths.is_empty() || paths.iter().any(|path| !path.is_absolute()) {
            return Err(ArchiveFailure {
                error: ArchiveError::InvalidSelection,
                remote_effects: ArchiveRemoteEffects::NotAttempted,
            });
        }
        let paths: Vec<_> = paths
            .iter()
            .map(|path| crate::restic_filesystem_path(path))
            .collect();
        self.archive_paths(&paths, &[], &[])
    }

    /// Force an archive of retained prepared inputs using their exact native
    /// transport. The caller keeps every prepared file and stage on refusal.
    /// Success still requires exact snapshot content readback before proof use.
    /// macOS sources are anchored relative paths, which only the owned creator
    /// can run, so they refuse here.
    pub fn archive_owned(
        &mut self,
        sources: &[ArchiveSource<'_>],
    ) -> Result<ArchiveSnapshot, ArchiveFailure> {
        if sources.is_empty()
            || sources.iter().any(|source| {
                !source.path().is_absolute()
                    || source.path().to_str().is_none()
                    || source.path().file_name() != Some(source.name())
            })
        {
            return Err(ArchiveFailure {
                error: ArchiveError::InvalidSelection,
                remote_effects: ArchiveRemoteEffects::NotAttempted,
            });
        }
        let paths: Vec<_> = sources
            .iter()
            .map(|source| source.path().to_str().expect("checked UTF-8").to_owned())
            .collect();
        #[cfg(unix)]
        let descriptors: Vec<_> = sources
            .iter()
            .filter_map(ArchiveSource::descriptor)
            .collect();
        #[cfg(not(unix))]
        let descriptors = vec![];
        self.archive_paths(&paths, &descriptors, sources)
    }

    fn revalidate_sources(&self, sources: &[ArchiveSource<'_>]) -> Result<(), ArchiveError> {
        self.revalidate_local()?;
        let result = sources.iter().try_for_each(|source| {
            source
                .revalidate()
                .map_err(|_| ArchiveError::SourceUnavailable)
        });
        // A stop arriving during a native observation retains its exact reason.
        self.revalidate_local()?;
        result
    }

    fn archive_paths(
        &mut self,
        paths: &[String],
        descriptors: &[crate::runner::PassedHandle<'_>],
        sources: &[ArchiveSource<'_>],
    ) -> Result<ArchiveSnapshot, ArchiveFailure> {
        let result = self.archive_paths_inner(paths, descriptors, sources);
        let mut argv: Vec<std::ffi::OsString> =
            self.write.global_options.iter().map(Into::into).collect();
        argv.extend([
            "backup".into(),
            "--force".into(),
            "--no-cache".into(),
            "--tag".into(),
            ARCHIVE_TAG.into(),
            "--retry-lock".into(),
            REPOSITORY_RETRY_LOCK.into(),
        ]);
        argv.extend(paths.iter().map(Into::into));
        argv.push("--json".into());
        let effects = match &result {
            Ok(snapshot) => DirectArchiveEffects::Attempted {
                returncode: Some(0),
                observed_snapshot_id: Some(snapshot.snapshot_id.clone()),
            },
            Err(failure) => match &failure.remote_effects {
                ArchiveRemoteEffects::NotAttempted => DirectArchiveEffects::NotAttempted,
                ArchiveRemoteEffects::Attempted {
                    returncode,
                    observed_snapshot_id,
                } => DirectArchiveEffects::Attempted {
                    returncode: *returncode,
                    observed_snapshot_id: observed_snapshot_id.clone(),
                },
            },
        };
        self.archives.push(ArchiveWriterObservation {
            ordinal: self.archives.len() as u64,
            requested_argv: argv,
            sources: sources
                .iter()
                .filter_map(|source| {
                    source
                        .name()
                        .to_str()
                        .map(|name| ArchiveWriterSourceObservation {
                            name: name.to_owned(),
                            identity: source.identity(),
                            digest: source.digest().clone(),
                        })
                })
                .collect(),
            effects,
        });
        result
    }

    fn archive_paths_inner(
        &mut self,
        paths: &[String],
        descriptors: &[crate::runner::PassedHandle<'_>],
        sources: &[ArchiveSource<'_>],
    ) -> Result<ArchiveSnapshot, ArchiveFailure> {
        let before = |error| ArchiveFailure {
            error,
            remote_effects: ArchiveRemoteEffects::NotAttempted,
        };
        self.revalidate_sources(sources).map_err(before)?;
        self.revalidate().map_err(before)?;
        self.revalidate_sources(sources).map_err(before)?;
        let timeout = self
            .operation
            .budget()
            .remaining(self.operation.clock())
            .map_err(|error| before(error.into()))?;
        let mut args = self.write.global_options.clone();
        args.extend([
            "backup".into(),
            "--force".into(),
            "--no-cache".into(),
            "--tag".into(),
            ARCHIVE_TAG.into(),
            "--retry-lock".into(),
            REPOSITORY_RETRY_LOCK.into(),
        ]);
        args.extend(paths.iter().cloned());
        let runner = ArchiveRunner {
            inner: self.runner,
            operation: &self.operation,
        };
        let output = run_restic(
            &runner,
            &args,
            &self.write.destination.repository,
            &self.write.password,
            &self.restic_path,
            Some(&self.write.backend_env),
            true,
            None,
            Some(timeout),
            descriptors,
        );
        let (returncode, observed_snapshot_id) = match &output {
            Ok(output) => (
                Some(output.returncode),
                snapshot_id(output.json.as_ref()).filter(|id| full_archive_id(id)),
            ),
            Err(_) => (None, None),
        };
        let effects = ArchiveRemoteEffects::Attempted {
            returncode,
            observed_snapshot_id: observed_snapshot_id.clone(),
        };
        let after = |error| ArchiveFailure {
            error,
            remote_effects: effects.clone(),
        };
        self.revalidate_sources(sources).map_err(after)?;
        let output = output.map_err(|_| after(ArchiveError::ToolFailed { reason: "failed" }))?;
        self.revalidate().map_err(after)?;
        self.revalidate_sources(sources).map_err(after)?;
        if output.returncode != 0 {
            return Err(after(ArchiveError::ToolFailed {
                reason: reason_for_returncode(output.returncode),
            }));
        }
        let snapshot_id =
            observed_snapshot_id.ok_or_else(|| after(ArchiveError::SnapshotIdentityInvalid))?;
        Ok(ArchiveSnapshot {
            repository_id: self.repository_id.clone(),
            snapshot_id,
        })
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
                    let got = observed.get(&crate::restic_tree_path(path)).copied();
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

    #[test]
    fn closed_archive_custody_is_send_static() {
        fn check<T: Send + 'static>() {}
        check::<DirectReadFinishOwner>();
        check::<OwnedDirectReadFinish>();
        check::<OwnedArchiveAdmissionFailure>();
        check::<OwnedPreparedArchiveHold>();
        check::<OwnedArchivePreparationFailure>();
        check::<OwnedDirectReadHold>();
        check::<OwnedReadbackAdmissionError>();
        check::<solstone_core_backup::archive_readback::OwnedHeldDirectRead>();
        check::<OwnedSettledDirectRead>();
    }

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

    const TEST_DAILY_KEY: &str = "synthetic-daily-backup-key";

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
        fn execute_cancellable(
            &self,
            request: &HttpRequest,
            cancellation: &ArchiveCancellation,
        ) -> Result<HttpResponse, HttpError> {
            assert!(!cancellation.is_cancelled());
            self.execute(request)
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
    #[test]
    fn admitted_tool_failure_keeps_original_setup_after_replacement() {
        let journal = configured_journal();
        let config = get_backup_config(journal.path()).unwrap();
        let original = solstone_core_backup::backup_setup_fingerprint(&config, None).unwrap();
        let capability = prepare(journal.path(), &FixedClock).unwrap();
        let mut replacement = solstone_core_backup::get_destination(journal.path())
            .unwrap()
            .unwrap();
        replacement.repository = "s3:replacement".into();
        solstone_core_backup::set_destination(journal.path(), &replacement).unwrap();
        capability.record_tool_error(
            &FixedClock,
            ClosedToolError::ResticUnavailable {
                guidance: "synthetic guidance".into(),
                detail: "synthetic tool failure".into(),
            },
        );
        let result = get_backup_config(journal.path()).unwrap()["last_backup"].clone();
        assert_eq!(result["setup_fingerprint"], original);
        assert_eq!(result["error_reason"], "restic_unavailable");
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

    #[test]
    fn restore_requests_one_scan_without_waiting_for_the_index_writer() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("imports/fixture/summary.md");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(source, "restored kestrel").unwrap();
        let lease = solstone_core_journal_io::hold_lock(
            solstone_core_indexer_store::db::db_path(root.path()),
            Default::default(),
        )
        .unwrap();
        NativeJournalMaintenance.full_scan(root.path()).unwrap();
        assert_eq!(
            solstone_core_indexer_store::todo::inspect(root.path())
                .unwrap()
                .keys,
            ["@scan-full"]
        );
        assert!(!solstone_core_indexer_store::db::db_path(root.path()).exists());
        drop(lease);
        assert_eq!(
            solstone_core_indexer_store::todo::drain(root.path(), Default::default())
                .unwrap()
                .failed,
            0
        );
        let conn = solstone_core_indexer_store::open_index_reader(root.path()).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunks WHERE chunks MATCH 'kestrel'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0);
        let check = solstone_core_indexer_store::scan::scan_journal(root.path(), false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
    }
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
    fn without_exclude_file(args: &[String]) -> Vec<String> {
        let mut result = Vec::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if arg == "--exclude-file" {
                iter.next();
            } else {
                result.push(arg.clone());
            }
        }
        result
    }

    fn json_backup_args(resolved_journal: &Path) -> Vec<String> {
        let mut args = backup_args(resolved_journal);
        args.extend(["--retry-lock".into(), "30m".into(), "--json".into()]);
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
            restic: Some((Path::new("/fixture/bin/restic")).into()),
            rclone_path: None,
            version: "test",
            journal_maintenance: maintenance,
        }
    }
    #[test]
    fn database_selection_preserves_directories_and_keeps_argv_bounded() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        fs::create_dir_all(root.join("imports/health-dedupe.sqlite")).unwrap();
        fs::create_dir_all(root.join("chronicle/20260000/indexer/indexer.sqlite")).unwrap();
        let small = prepare_backup_args(root).unwrap();
        let argv_size: usize = small.args.iter().map(String::len).sum();
        drop(small);
        for day in 20260000..20260500 {
            fs::create_dir_all(root.join(format!("chronicle/{day}/indexer"))).unwrap();
        }
        let prepared = prepare_backup_args(root).unwrap();
        let policy = fs::read_to_string(prepared._exclude_file.path()).unwrap();
        assert_eq!(policy.lines().count(), 8 + 500 * 8 - 2);
        assert!(
            !policy
                .lines()
                .any(|line| line.ends_with("/imports/health-dedupe.sqlite"))
        );
        assert!(
            !policy
                .lines()
                .any(|line| line.ends_with("/chronicle/20260000/indexer/indexer.sqlite"))
        );
        assert!(
            policy.lines().any(
                |line| line.ends_with("/chronicle/20260499/indexer/transcripts.sqlite-journal")
            )
        );
        assert_eq!(prepared.args.len(), backup_args(root).len() + 2);
        assert_eq!(
            prepared.args.iter().map(String::len).sum::<usize>(),
            argv_size
        );
        let private_path = prepared._exclude_file.path().to_owned();
        drop(prepared);
        assert!(!private_path.exists());
    }

    #[test]
    fn database_metadata_failure_does_not_invoke_restic_even_for_unlock() {
        let journal = configured_journal();
        fs::write(journal.path().join("chronicle"), b"invalid namespace").unwrap();
        let fingerprint = solstone_core_backup::backup_setup_fingerprint(
            &get_backup_config(journal.path()).unwrap(),
            None,
        )
        .unwrap();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            commands: RefCell::new(vec![]),
        };
        let result = run_backup(
            journal.path(),
            &services(&runner, &Http, &FixedClock, &Maintenance),
        );
        assert_eq!(result.status, "error");
        assert_eq!(result.error_reason.as_deref(), Some("failed"));
        assert!(runner.commands.borrow().is_empty());
        assert_eq!(
            get_backup_config(journal.path()).unwrap()["last_backup"]["setup_fingerprint"],
            fingerprint
        );
    }

    #[test]
    fn database_exclude_file_lives_through_the_child_and_is_removed_on_terminal_results() {
        struct ReadsPolicy {
            returncode: Option<i32>,
            path: RefCell<Option<PathBuf>>,
        }
        impl ToolRunner for ReadsPolicy {
            fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
                if request.argv.iter().any(|arg| arg == "unlock") {
                    return Ok(output(0, ""));
                }
                let index = request
                    .argv
                    .iter()
                    .position(|arg| arg == "--exclude-file")
                    .unwrap();
                let path = PathBuf::from(&request.argv[index + 1]);
                let text = fs::read_to_string(&path).expect("policy readable while restic runs");
                assert!(
                    text.lines()
                        .any(|line| line.ends_with("/imports/health-dedupe.sqlite"))
                );
                *self.path.borrow_mut() = Some(path);
                match self.returncode {
                    Some(code) => Ok(output(
                        code,
                        "{\"message_type\":\"summary\",\"snapshot_id\":\"snap\"}",
                    )),
                    None => Err(io::Error::other("synthetic tool failure")),
                }
            }
        }
        for code in [Some(0), Some(1), None] {
            let journal = configured_journal();
            let runner = ReadsPolicy {
                returncode: code,
                path: RefCell::new(None),
            };
            let result = run_backup(
                journal.path(),
                &services(&runner, &Http, &FixedClock, &Maintenance),
            );
            assert_eq!(result.status, if code == Some(0) { "ok" } else { "error" });
            assert!(
                !runner
                    .path
                    .borrow()
                    .as_ref()
                    .expect("backup invoked")
                    .exists()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn unrepresentable_exclude_file_root_fails_before_runner_invocation() {
        let parent = tempfile::tempdir().unwrap();
        let journal = parent.path().join("journal\nowner");
        fs::create_dir(&journal).unwrap();
        configure_journal(&journal);
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            commands: RefCell::new(vec![]),
        };
        assert_eq!(
            run_backup(
                &journal,
                &services(&runner, &Http, &FixedClock, &Maintenance)
            )
            .status,
            "error"
        );
        assert!(runner.commands.borrow().is_empty());
    }

    fn valid_backup_section(destination: Value) -> Value {
        json!({
            "enabled": true,
            "mode": "byo",
            "daily_key": TEST_DAILY_KEY,
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

    #[cfg(feature = "full-tests")]
    mod archive_admission {
        use super::*;
        use solstone_core_backup::archive_admission::ArchiveCancellation;

        const REPOSITORY_ID: &str =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const OTHER_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        const SNAPSHOT_ID: &str =
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

        type RequestHook = Box<dyn Fn(&ToolRequest<'_>)>;

        struct ArchiveScript {
            outputs: RefCell<VecDeque<io::Result<ToolOutput>>>,
            requests: RefCell<Vec<ObservedRequest>>,
            programs: RefCell<Vec<std::ffi::OsString>>,
            hook: Option<RequestHook>,
        }

        impl ToolRunner for ArchiveScript {
            fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
                self.programs.borrow_mut().push(request.program.clone());
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
                if let Some(hook) = &self.hook {
                    hook(request);
                }
                self.outputs
                    .borrow_mut()
                    .pop_front()
                    .expect("script exhausted")
            }

            fn run_cancellable(
                &self,
                request: &ToolRequest<'_>,
                cancellation: &ArchiveCancellation,
            ) -> io::Result<ToolOutput> {
                assert!(
                    !cancellation.is_cancelled(),
                    "owner must refuse before launch"
                );
                self.run(request)
            }
        }

        fn script(outputs: Vec<ToolOutput>) -> ArchiveScript {
            ArchiveScript {
                outputs: RefCell::new(outputs.into_iter().map(Ok).collect()),
                requests: RefCell::new(vec![]),
                programs: RefCell::new(vec![]),
                hook: None,
            }
        }

        fn repo(id: &str) -> ToolOutput {
            output(
                0,
                &json!({"version": 2, "id": id, "chunker_polynomial": "1"}).to_string(),
            )
        }

        fn snapshot(code: i32, id: &str) -> ToolOutput {
            output(
                code,
                &json!({"message_type": "summary", "snapshot_id": id}).to_string(),
            )
        }

        fn fixture() -> (tempfile::TempDir, Arc<JournalRoot>) {
            let journal = tempfile::tempdir().unwrap();
            write_backup_section(journal.path(), valid_backup_section(valid_destination()));
            let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
            (journal, root)
        }

        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        #[test]
        #[ignore = "requires an actual copied installed package and supported origin fixture"]
        fn installed_forward_recovery_binding_uses_original_read_and_current_native_facts() {
            use std::os::unix::fs::MetadataExt;
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Input {
                journal: PathBuf,
                restic_path: PathBuf,
                repository_id: String,
            }
            let input: Input = serde_json::from_slice(
                &std::fs::read(std::env::var_os("ROOT_FORWARD_PROBE_JSON").unwrap()).unwrap(),
            )
            .unwrap();
            let root = Arc::new(JournalRoot::open(&input.journal).unwrap());
            let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
            let cancellation = ArchiveCancellation::new();
            let prepared = OwnedPreparedArchiveOperation::prepare(
                Arc::clone(&root),
                ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone()).unwrap(),
                Arc::clone(&clock),
            )
            .unwrap_or_else(|e| panic!("prepare: {}", e.reason()));
            let installed = admit_prepared_archive_owned_read_installed(
                prepared,
                input.repository_id.clone(),
                &input.restic_path,
                env!("CARGO_PKG_VERSION"),
                &UreqHttpTransport,
            )
            .unwrap_or_else(|e| panic!("installed read: {:?}", e.error()));
            let (read, outside) = installed.into_parts();
            let qualified = outside.observe_recovery_binding(&read).unwrap();
            qualified.revalidate_for_read(&read).unwrap();
            assert_eq!(qualified.data().repository_id(), read.repository_id());
            assert_eq!(read.repository_id(), input.repository_id);
            assert!(read.archive_origin_failure().is_none());
            assert!(Arc::ptr_eq(read.carrier(), outside.carrier()));
            let reservation = std::fs::metadata(read.carrier().diagnostic_path()).unwrap();
            assert_eq!(
                qualified.data().reservation_identity(),
                solstone_core_journal_io::media::MediaIdentity::Unix {
                    device: reservation.dev(),
                    inode: reservation.ino()
                }
            );
            assert_eq!(
                qualified.data().reservation_name(),
                read.carrier()
                    .diagnostic_path()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
            );
            let (_other_temp, other_root) = fixture();
            let other = OwnedPreparedArchiveOperation::prepare(
                other_root,
                ArchiveBudget::new(clock.as_ref()).unwrap(),
                Arc::clone(&clock),
            )
            .unwrap_or_else(|e| panic!("other carrier: {}", e.reason()));
            let wrong_carrier = DirectReadFinishOwner {
                carrier: Arc::clone(other.carrier()),
                clock: Arc::clone(&clock),
                broker: DirectBrokerCompletion::CapturedByo,
            };
            assert!(matches!(
                wrong_carrier.observe_recovery_binding(&read),
                Err(RecoveryBindingRefusal::Pairing(_))
            ));
            let wrong_clock = DirectReadFinishOwner {
                carrier: Arc::clone(read.carrier()),
                clock: Arc::new(FixedClock),
                broker: DirectBrokerCompletion::CapturedByo,
            };
            assert!(matches!(
                wrong_clock.observe_recovery_binding(&read),
                Err(RecoveryBindingRefusal::Pairing(_))
            ));
            let lock = input.journal.join("backup-activity.lock");
            std::fs::write(&lock, b"actual later native stamp").unwrap();
            assert!(matches!(
                qualified.revalidate_for_read(&read),
                Err(RecoveryBindingRefusal::Changed)
            ));
            let current = outside.observe_recovery_binding(&read).unwrap();
            let alias = input.journal.join("lock-alias");
            std::fs::hard_link(&lock, &alias).unwrap();
            assert!(matches!(
                current.revalidate_for_read(&read),
                Err(RecoveryBindingRefusal::Read(_))
            ));
            std::fs::remove_file(alias).unwrap();
            cancellation.cancel();
            assert!(matches!(
                current.revalidate_for_read(&read),
                Err(RecoveryBindingRefusal::Read(_))
            ));
            assert!(matches!(
                solstone_core_journal_io::activity::ActivityLease::try_acquire(Arc::clone(&root)),
                Err(solstone_core_journal_io::activity::ActivityAdmissionError::Busy)
            ));
            read.carrier().revalidate().unwrap();
            let data = serde_json::to_value(qualified.data()).unwrap();
            println!(
                "ROOT_FORWARD_BINDING_RESULT={}",
                serde_json::json!({
                "repository_id":read.repository_id(),"binding":data,
                "same_read_revalidated":true,"actual_pair_mismatches_refused":true,
                "actual_stale_native_refused":true,"cancellation_refused":true,"owners_retained":true})
            );
            drop((
                current,
                qualified,
                wrong_clock,
                wrong_carrier,
                other,
                read,
                outside,
            ));
        }

        #[test]
        fn closed_admitted_custody_keeps_pair_clock_and_partial_cat_observations() {
            for replace_password in [false, true] {
                let (_journal, root) = fixture();
                let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
                let original = match OwnedPreparedArchiveOperation::prepare(
                    Arc::clone(&root),
                    ArchiveBudget::new(clock.as_ref()).unwrap(),
                    Arc::clone(&clock),
                ) {
                    Ok(original) => original,
                    Err(failure) => panic!("original fixture: {}", failure.reason()),
                };
                let carrier = Arc::clone(original.carrier());
                let mut destination = original.configuration().destination().unwrap();
                if !replace_password {
                    destination.repository = "s3:rerouted".into();
                }
                let password = if replace_password {
                    "replacement"
                } else {
                    TEST_DAILY_KEY
                };
                let env = value_env(&destination).unwrap();
                let input = original.bind_archive_creator(capture_archive_predecessor(
                    PathBuf::from("/absent/restic"),
                    Runtime {
                        destination: destination.clone(),
                        password: password.into(),
                        backend_env: env.clone(),
                        global_options: vec![],
                    },
                    Runtime {
                        destination,
                        password: password.into(),
                        backend_env: env,
                        global_options: vec![],
                    },
                    ArchiveCaptureFacts {
                        effects: ArchiveAdmissionEffects::default(),
                        write_repository_id: None,
                        read_repository_id: None,
                        repository_id: None,
                        archives: vec![],
                    },
                ));
                #[cfg(unix)]
                let refusal = match input.admit() {
                    Err(refusal) => refusal,
                    Ok(_) => panic!("captured pair cannot reroute original configuration"),
                };
                #[cfg(windows)]
                let refusal = match input.admit(Some(Arc::new(crate::runner::SystemToolRunner))) {
                    Err(refusal) => refusal,
                    Ok(_) => panic!("captured pair cannot reroute original configuration"),
                };
                assert_eq!(
                    refusal.reason(),
                    if replace_password {
                        ReadbackError::CredentialsUnavailable
                    } else {
                        ReadbackError::NotConfigured
                    }
                );
                assert_eq!(refusal.archive_admission_effects(), Some((false, false)));
                let refusal = std::thread::spawn(move || refusal).join().unwrap();
                assert!(Arc::ptr_eq(refusal.carrier(), &carrier));
                assert!(Arc::ptr_eq(refusal.owned_clock(), &clock));
            }
            for owned in [true, false] {
                let (_journal, root) = fixture();
                let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
                let budget = ArchiveBudget::new(clock.as_ref()).unwrap();
                let original = if owned {
                    PreparedArchiveOperation::prepare_owned(
                        Arc::clone(&root),
                        budget,
                        Arc::clone(&clock),
                    )
                    .unwrap()
                } else {
                    PreparedArchiveOperation::prepare(Arc::clone(&root), budget, clock.as_ref())
                        .unwrap()
                };
                let carrier = Arc::clone(original.carrier());
                let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
                let http = ScriptedHttp {
                    responses: RefCell::new(VecDeque::new()),
                    requests: RefCell::new(Vec::new()),
                };
                let services = services(&runner, &http, clock.as_ref(), &Maintenance);
                let admitted = admit_prepared_archive(original, &services).unwrap();
                let write = admitted.write.destination.clone();
                let read = admitted.read.destination.clone();
                let password = admitted.write.password.clone();
                assert!(Arc::ptr_eq(
                    admitted.prepared_operation().carrier(),
                    &carrier
                ));
                match admitted.into_owned_hold() {
                    Ok(hold) => {
                        assert!(owned);
                        let hold = std::thread::spawn(move || hold).join().unwrap();
                        assert!(Arc::ptr_eq(hold.carrier(), &carrier));
                        assert!(Arc::ptr_eq(hold.owned_clock(), &clock));
                        let OwnedAdmittedArchiveHoldInner::Captured(captured) = &*hold.inner else {
                            panic!("generic close must retain captured pair")
                        };
                        assert_eq!(captured._capture.write.destination, write);
                        assert_eq!(captured._capture.read.destination, read);
                        assert_eq!(captured._capture.write.password, password);
                        assert_eq!(
                            captured._capture.write_repository_id.as_deref(),
                            Some(REPOSITORY_ID)
                        );
                        assert_eq!(
                            captured._capture.read_repository_id.as_deref(),
                            Some(REPOSITORY_ID)
                        );
                    }
                    Err(original) => {
                        assert!(!owned);
                        assert!(Arc::ptr_eq(original.carrier(), &carrier));
                        assert!(original.operation.owned_clock().is_none());
                        assert_eq!(original.write.destination, write);
                        assert_eq!(original.read.destination, read);
                        assert_eq!(original.write.password, password);
                    }
                }
                assert_eq!(runner.requests.borrow().len(), 2);
                assert!(http.requests.borrow().is_empty());
            }
            for second in [false, true] {
                let (_journal, root) = fixture();
                let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
                let original = PreparedArchiveOperation::prepare_owned(
                    Arc::clone(&root),
                    ArchiveBudget::new(clock.as_ref()).unwrap(),
                    Arc::clone(&clock),
                )
                .unwrap();
                let carrier = Arc::clone(original.carrier());
                let runner = script(if second {
                    vec![repo(REPOSITORY_ID), output(1, "")]
                } else {
                    vec![output(1, "")]
                });
                let http = ScriptedHttp {
                    responses: RefCell::new(VecDeque::new()),
                    requests: RefCell::new(Vec::new()),
                };
                let services = services(&runner, &http, clock.as_ref(), &Maintenance);
                let failure = refusal(admit_prepared_archive(original, &services));
                assert!(Arc::ptr_eq(failure.carrier().unwrap(), &carrier));
                let failure = failure.into_owned_hold().unwrap();
                let failure = std::thread::spawn(move || failure).join().unwrap();
                let ClosedArchiveAdmissionOwner::Admitted(hold) = &failure.inner.owned else {
                    panic!("cat refusal must preserve whole admission")
                };
                let OwnedAdmittedArchiveHoldInner::Captured(captured) = &*hold.inner else {
                    panic!("cat refusal pair")
                };
                assert!(Arc::ptr_eq(hold.owned_clock(), &clock));
                assert!(captured._capture.write_repository_requested);
                assert_eq!(captured._capture.read_repository_requested, second);
                assert_eq!(
                    captured._capture.write_repository_id.as_deref(),
                    second.then_some(REPOSITORY_ID)
                );
                assert!(captured._capture.read_repository_id.is_none());
                assert_eq!(runner.requests.borrow().len(), if second { 2 } else { 1 });
            }
        }

        #[test]
        fn typed_owned_preparation_and_admission_refusals_close_original_lifetime() {
            struct OwnedClock(std::sync::atomic::AtomicI64);
            impl Clock for OwnedClock {
                fn now_unix(&self) -> i64 {
                    self.0.load(std::sync::atomic::Ordering::Relaxed)
                }
                fn iso_week(&self) -> u8 {
                    7
                }
            }
            for cancelled in [false, true] {
                let (_journal, root) = fixture();
                let actual_clock = Arc::new(OwnedClock(std::sync::atomic::AtomicI64::new(50)));
                let clock: Arc<dyn Clock + Send + Sync> = actual_clock.clone();
                let cancellation = ArchiveCancellation::new();
                let budget =
                    ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone()).unwrap();
                let original = match OwnedPreparedArchiveOperation::prepare(
                    Arc::clone(&root),
                    budget,
                    Arc::clone(&clock),
                ) {
                    Ok(original) => original,
                    Err(failure) => panic!("preparation refused: {}", failure.reason()),
                };
                let weak = Arc::downgrade(original.carrier());
                if cancelled {
                    cancellation.cancel();
                } else {
                    actual_clock.0.store(
                        51 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                let refusal = match admit_prepared_archive_owned_read_installed(
                    original,
                    REPOSITORY_ID.into(),
                    Path::new("/absent/restic"),
                    "fixture",
                    &UreqHttpTransport,
                ) {
                    Err(refusal) => refusal,
                    Ok(_) => panic!("original stopped lifetime must refuse"),
                };
                assert_eq!(
                    refusal.error(),
                    if cancelled {
                        ArchiveError::Cancelled
                    } else {
                        ArchiveError::DeadlineExceeded
                    }
                );
                assert_eq!(*refusal.effects(), ArchiveAdmissionEffects::default());
                let refusal = std::thread::spawn(move || refusal).join().unwrap();
                assert!(Arc::ptr_eq(refusal.journal_root(), &root));
                assert!(Arc::ptr_eq(refusal.owned_clock().unwrap(), &clock));
                assert!(Arc::ptr_eq(
                    refusal.carrier().unwrap(),
                    &weak.upgrade().unwrap()
                ));
                drop(refusal);
                assert!(weak.upgrade().is_none());
            }

            let (_journal, root) = fixture();
            let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
            let cancellation = ArchiveCancellation::new();
            let budget =
                ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone()).unwrap();
            cancellation.cancel();
            let failure = match OwnedPreparedArchiveOperation::prepare(
                Arc::clone(&root),
                budget,
                Arc::clone(&clock),
            ) {
                Err(failure) => failure,
                Ok(_) => panic!("stopped preparation must refuse"),
            };
            let failure = std::thread::spawn(move || failure).join().unwrap();
            assert_eq!(failure.reason(), AdmissionError::Cancelled);
            assert!(Arc::ptr_eq(failure.journal_root(), &root));
            assert!(Arc::ptr_eq(failure.owned_clock(), &clock));
            assert!(failure.carrier().is_none());
        }

        #[test]
        fn closed_owned_preparation_keeps_same_clock_carrier_and_reservation() {
            let (_journal, root) = fixture();
            let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
            let cancellation = ArchiveCancellation::new();
            let budget =
                ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone()).unwrap();
            let original = PreparedArchiveOperation::prepare_owned(
                Arc::clone(&root),
                budget,
                Arc::clone(&clock),
            )
            .unwrap();
            assert!(Arc::ptr_eq(original.owned_clock().unwrap(), &clock));
            let weak = Arc::downgrade(original.carrier());
            cancellation.cancel();
            assert_eq!(original.revalidate(), Err(AdmissionError::Cancelled));
            let hold = match original.into_owned_hold() {
                Ok(hold) => hold,
                Err(_) => panic!("actual owned clock must transfer"),
            };
            let hold = std::thread::spawn(move || hold).join().unwrap();
            assert!(Arc::ptr_eq(hold.owned_clock(), &clock));
            assert!(Arc::ptr_eq(hold.journal_root(), &root));
            assert!(weak.upgrade().is_some());
            assert!(
                !solstone_core_journal_io::activity_obligations::observe_obligations(&root)
                    .unwrap()
                    .entries()
                    .is_empty()
            );
            drop(hold);
            assert!(weak.upgrade().is_none());
            // Dropping closed custody never certifies or retires the reservation.
            assert!(
                !solstone_core_journal_io::activity_obligations::observe_obligations(&root)
                    .unwrap()
                    .entries()
                    .is_empty()
            );
        }

        #[test]
        fn refused_finish_retires_only_an_unshared_carrier_and_never_completes() {
            use solstone_core_backup::archive_readback::RefusedDirectRead;
            let (_journal, root) = fixture();
            let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
            let operation = PreparedArchiveOperation::prepare_owned(
                Arc::clone(&root),
                ArchiveBudget::new(clock.as_ref()).unwrap(),
                Arc::clone(&clock),
            )
            .unwrap();
            let outside = |clock: &Arc<dyn Clock + Send + Sync>| DirectReadFinishOwner {
                carrier: Arc::clone(operation.carrier()),
                clock: Arc::clone(clock),
                broker: DirectBrokerCompletion::CapturedByo,
            };
            let refused = || RefusedDirectRead::for_test(Arc::clone(&root), Arc::clone(&clock));
            let path = operation.carrier().diagnostic_path();
            // The read's own activity still shares the carrier.
            let shared = outside(&clock).finish_refused(refused());
            assert!(!shared.released());
            assert_eq!(shared.state(), DirectReadFinishState::Held);
            assert!(shared.retirement().is_none());
            drop(shared);
            // Another operation's clock never retires this carrier.
            let other: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
            let foreign = outside(&other).finish_refused(refused());
            assert!(!foreign.released());
            drop(foreign);
            assert!(path.is_dir());
            // Once the refused read's activity ended, its empty carrier retires.
            let owner = outside(&clock);
            drop(operation);
            let released = owner.finish_refused(refused());
            assert!(released.released());
            assert_eq!(released.state(), DirectReadFinishState::Held);
            assert_eq!(released.reason(), Some(ReadbackError::ToolFailed));
            assert!(!path.exists());
            assert!(matches!(
                solstone_core_journal_io::activity::ActivityLease::try_acquire(Arc::clone(&root)),
                Err(solstone_core_journal_io::activity::ActivityAdmissionError::Busy)
            ));
            drop(released);
            solstone_core_journal_io::activity::ActivityLease::try_acquire(Arc::clone(&root))
                .unwrap();
            assert!(
                solstone_core_journal_io::activity_obligations::observe_obligations(&root)
                    .unwrap()
                    .entries()
                    .is_empty()
            );
        }

        #[test]
        fn installed_finish_borrowed_refusal_retains_original_before_effects() {
            let (_journal, root) = fixture();
            let budget = ArchiveBudget::new(&FixedClock).unwrap();
            let original = PreparedArchiveOperation::prepare(root, budget, &FixedClock).unwrap();
            let carrier = Arc::clone(original.carrier());
            let refusal = match admit_prepared_archive_read_installed(
                original,
                REPOSITORY_ID.into(),
                Path::new("/absent/restic"),
                "fixture",
                &UreqHttpTransport,
            ) {
                Err(refusal) => refusal,
                Ok(_) => panic!("borrowed clock must not mint finish owner"),
            };
            assert_eq!(
                refusal.error(),
                ArchiveError::Readback(ReadbackError::Unsupported)
            );
            assert_eq!(*refusal.effects(), ArchiveAdmissionEffects::default());
            assert!(Arc::ptr_eq(refusal.carrier().unwrap(), &carrier));
            let refusal = match refusal.into_owned_hold() {
                Err(original) => original,
                Ok(_) => panic!("borrowed refusal must remain borrowed"),
            };
            assert!(Arc::ptr_eq(refusal.carrier().unwrap(), &carrier));
        }

        #[test]
        fn closed_owned_admission_refusal_and_preparation_failure_keep_original_clock() {
            let (_journal, root) = fixture();
            let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
            let budget = ArchiveBudget::new(clock.as_ref()).unwrap();
            let original = PreparedArchiveOperation::prepare_owned(
                Arc::clone(&root),
                budget,
                Arc::clone(&clock),
            )
            .unwrap();
            let carrier = Arc::clone(original.carrier());
            let refusal = match admit_prepared_archive_read_installed(
                original,
                "bad-id".into(),
                Path::new("/absent/restic"),
                "fixture",
                &UreqHttpTransport,
            ) {
                Err(refusal) => refusal,
                Ok(_) => panic!("bad identity must refuse"),
            };
            assert_eq!(refusal.error(), ArchiveError::RepositoryIdentityInvalid);
            assert_eq!(*refusal.effects(), ArchiveAdmissionEffects::default());
            let refusal = refusal.into_owned_hold().unwrap();
            let refusal = std::thread::spawn(move || refusal).join().unwrap();
            assert!(Arc::ptr_eq(refusal.carrier().unwrap(), &carrier));
            drop(refusal);
            drop(carrier);

            let cancellation = ArchiveCancellation::new();
            let budget =
                ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone()).unwrap();
            cancellation.cancel();
            let failure = match PreparedArchiveOperation::prepare_owned(
                Arc::clone(&root),
                budget,
                Arc::clone(&clock),
            ) {
                Err(failure) => failure,
                Ok(_) => panic!("cancellation must refuse before native preparation"),
            };
            assert_eq!(failure.reason(), AdmissionError::Cancelled);
            assert!(Arc::ptr_eq(failure.owned_clock().unwrap(), &clock));
            let failure = failure.into_owned_hold().unwrap();
            let failure = std::thread::spawn(move || failure).join().unwrap();
            assert!(Arc::ptr_eq(failure.owned_clock(), &clock));
            assert!(Arc::ptr_eq(failure.journal_root(), &root));
            assert!(failure.carrier().is_none());
        }

        fn source(journal: &Path) -> Vec<PathBuf> {
            // Transport data only; no assertion of source ownership or immutability.
            vec![journal.join("staged/audio.flac")]
        }

        fn refusal<'a>(
            result: Result<AdmittedArchive<'a>, ArchiveAdmissionFailure<'a>>,
        ) -> ArchiveAdmissionFailure<'a> {
            match result {
                Ok(_) => panic!("unexpected admission"),
                Err(error) => error,
            }
        }

        fn hosted(journal: &Path, expiry: &str) -> ScriptedHttp {
            solstone_core_backup::set_mode(journal, "operated").unwrap();
            solstone_core_backup::save_hosted_binding(journal, &hosted_binding("https://broker"))
                .unwrap();
            let mut response = credentials_response();
            let mut body: Value = serde_json::from_slice(&response.body).unwrap();
            body["expires_at"] = json!(expiry);
            response.body = serde_json::to_vec(&body).unwrap();
            ScriptedHttp {
                responses: RefCell::new(VecDeque::from([Ok(response)])),
                requests: RefCell::new(vec![]),
            }
        }

        struct ControlledHttp {
            response: Result<HttpResponse, HttpError>,
            cancel_during: bool,
            requests: RefCell<Vec<HttpRequest>>,
            observed: RefCell<Option<ArchiveCancellation>>,
        }

        impl HttpTransport for ControlledHttp {
            fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
                panic!("controlled HTTP must never fall back to ordinary execution")
            }

            fn execute_cancellable(
                &self,
                request: &HttpRequest,
                cancellation: &ArchiveCancellation,
            ) -> Result<HttpResponse, HttpError> {
                assert!(!cancellation.is_cancelled());
                self.requests.borrow_mut().push(request.clone());
                *self.observed.borrow_mut() = Some(cancellation.clone());
                if self.cancel_during {
                    cancellation.cancel();
                }
                self.response.clone()
            }
        }

        fn controlled_http(journal: &Path, cancel_during: bool, fails: bool) -> ControlledHttp {
            let script = hosted(journal, "2030-01-01T00:00:00Z");
            ControlledHttp {
                response: if fails {
                    Err(HttpError::Other)
                } else {
                    script.responses.borrow_mut().pop_front().unwrap()
                },
                cancel_during,
                requests: RefCell::new(vec![]),
                observed: RefCell::new(None),
            }
        }

        #[test]
        fn controlled_broker_keeps_request_ceiling_original_budget_and_stop_signal() {
            for seconds_left in [5, 35] {
                let (journal, root) = fixture();
                let http = controlled_http(journal.path(), false, false);
                let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
                let now = Rc::new(Cell::new(50));
                let clock = MutableClock(now.clone());
                let cancellation = ArchiveCancellation::new();
                let budget = ArchiveBudget::new_cancellable(&clock, cancellation.clone()).unwrap();
                now.set(50 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64 - seconds_left);
                let mut services = services(&runner, &http, &clock, &Maintenance);
                services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
                let admitted = admit_archive_with_budget(root, &services, budget).unwrap();
                assert_eq!(http.requests.borrow().len(), 1);
                assert_eq!(
                    http.requests.borrow()[0].timeout,
                    Duration::from_secs(seconds_left.min(30) as u64)
                );
                assert!(
                    runner
                        .requests
                        .borrow()
                        .iter()
                        .all(|r| r.timeout == Some(Duration::from_secs(seconds_left as u64)))
                );
                http.observed.borrow().as_ref().unwrap().cancel();
                assert!(cancellation.is_cancelled());
                assert_eq!(admitted.revalidate_local(), Err(ArchiveError::Cancelled));
            }
        }

        #[test]
        fn controlled_broker_cancellation_after_success_or_error_refuses_before_tools() {
            for fails in [false, true] {
                let (journal, root) = fixture();
                let http = controlled_http(journal.path(), true, fails);
                let cancellation = ArchiveCancellation::new();
                let budget =
                    ArchiveBudget::new_cancellable(&FixedClock, cancellation.clone()).unwrap();
                let services = services(&PanicRunner, &http, &FixedClock, &Maintenance);
                assert_eq!(
                    refusal(admit_archive_with_budget(root, &services, budget)).error(),
                    ArchiveError::Cancelled
                );
                assert!(cancellation.is_cancelled());
                assert_eq!(http.requests.borrow().len(), 1);
            }
        }

        #[test]
        fn precancelled_operated_budget_refuses_before_controlled_http() {
            let (journal, root) = fixture();
            let http = controlled_http(journal.path(), false, false);
            let cancellation = ArchiveCancellation::new();
            let budget = ArchiveBudget::new_cancellable(&FixedClock, cancellation.clone()).unwrap();
            cancellation.cancel();
            let services = services(&PanicRunner, &http, &FixedClock, &Maintenance);
            assert_eq!(
                refusal(admit_archive_with_budget(root, &services, budget)).error(),
                ArchiveError::Cancelled
            );
            assert!(http.requests.borrow().is_empty());
            assert!(http.observed.borrow().is_none());
        }

        #[test]
        #[cfg(feature = "full-tests")]
        fn prepared_archive_refusal_retains_same_owner_after_second_repository_request() {
            let (_journal, root) = fixture();
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            runner.outputs.borrow_mut()[1] = Err(io::Error::other("unknown repository completion"));
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let original = PreparedArchiveOperation::prepare(
                root.clone(),
                ArchiveBudget::new(&FixedClock).unwrap(),
                &FixedClock,
            )
            .unwrap();
            let carrier = original.carrier().clone();
            let failure = refusal(admit_prepared_archive(original, &services));
            assert_eq!(
                failure.error(),
                ArchiveError::ToolFailed { reason: "failed" }
            );
            assert!(Arc::ptr_eq(failure.carrier().unwrap(), &carrier));
            assert!(!failure.effects().broker_requested);
            assert!(failure.effects().write_repository_requested);
            assert!(failure.effects().read_repository_requested);
            assert_eq!(runner.requests.borrow().len(), 2);
            let next = PreparedArchiveOperation::prepare(
                root,
                ArchiveBudget::new(&FixedClock).unwrap(),
                &FixedClock,
            )
            .err()
            .expect("same retained owner excludes fresh admission");
            assert!(next.preparation().is_some());
            assert_eq!(runner.requests.borrow().len(), 2);
        }

        #[test]
        #[cfg(feature = "full-tests")]
        fn prepared_archive_public_entrance_keeps_original_expiry_and_stop() {
            for cancelled in [false, true] {
                let (_journal, root) = fixture();
                let now = Rc::new(Cell::new(50));
                let clock = MutableClock(now.clone());
                let cancellation = ArchiveCancellation::new();
                let budget = ArchiveBudget::new_cancellable(&clock, cancellation.clone()).unwrap();
                let original = PreparedArchiveOperation::prepare(root, budget, &clock).unwrap();
                let carrier = original.carrier().clone();
                if cancelled {
                    cancellation.cancel();
                } else {
                    now.set(50 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64);
                }
                let services = services(&PanicRunner, &PanicHttp, &clock, &Maintenance);
                let failure = refusal(admit_prepared_archive(original, &services));
                assert_eq!(
                    failure.error(),
                    if cancelled {
                        ArchiveError::Cancelled
                    } else {
                        ArchiveError::DeadlineExceeded
                    }
                );
                assert!(Arc::ptr_eq(failure.carrier().unwrap(), &carrier));
                assert!(!failure.effects().broker_requested);
                assert!(!failure.effects().write_repository_requested);
                assert!(!failure.effects().read_repository_requested);
            }
        }

        #[test]
        #[cfg(unix)]
        fn original_native_carrier_loss_refuses_before_adapter_dispatch() {
            for occupied in [false, true] {
                let (journal, root) = fixture();
                if occupied {
                    // This twin would enter the operated broker if its original
                    // native carrier were not revalidated before dispatch.
                    let _ = hosted(journal.path(), "2030-01-01T00:00:00Z");
                }
                let operation = PreparedArchiveOperation::prepare(
                    root,
                    ArchiveBudget::new(&FixedClock).unwrap(),
                    &FixedClock,
                )
                .unwrap();
                let original = operation.carrier().clone();
                let path = original.diagnostic_path();
                if occupied {
                    fs::write(path.join("foreign"), b"unresolved owner").unwrap();
                } else {
                    fs::rename(&path, path.with_extension("held")).unwrap();
                }
                let services = services(&PanicRunner, &PanicHttp, &FixedClock, &Maintenance);
                let failure = refusal(admit_prepared_archive(operation, &services));
                assert_eq!(failure.error(), ArchiveError::JournalIdentityChanged);
                assert!(Arc::ptr_eq(failure.carrier().unwrap(), &original));
                assert!(original.revalidate().is_err());
                assert_eq!(*failure.effects(), ArchiveAdmissionEffects::default());
            }
        }

        #[test]
        #[cfg(unix)]
        fn original_native_carrier_loss_during_final_cat_rejects_valid_repository() {
            for occupied in [false, true] {
                let (_journal, root) = fixture();
                let operation = PreparedArchiveOperation::prepare(
                    root,
                    ArchiveBudget::new(&FixedClock).unwrap(),
                    &FixedClock,
                )
                .unwrap();
                let original = operation.carrier().clone();
                let path = original.diagnostic_path();
                let calls = Rc::new(Cell::new(0));
                let observed = calls.clone();
                let mut runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
                runner.hook = Some(Box::new(move |_| {
                    observed.set(observed.get() + 1);
                    if observed.get() == 2 {
                        if occupied {
                            fs::write(path.join("foreign"), b"unresolved owner").unwrap();
                        } else {
                            fs::rename(&path, path.with_extension("held")).unwrap();
                        }
                    }
                }));
                let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                let failure = refusal(admit_prepared_archive(operation, &services));
                assert_eq!(failure.error(), ArchiveError::JournalIdentityChanged);
                assert!(Arc::ptr_eq(failure.carrier().unwrap(), &original));
                assert!(original.revalidate().is_err());
                assert!(!failure.effects().broker_requested);
                assert!(failure.effects().write_repository_requested);
                assert!(failure.effects().read_repository_requested);
                assert_eq!(calls.get(), 2);
                assert_eq!(runner.requests.borrow().len(), 2);
                assert!(runner.outputs.borrow().is_empty());
            }
        }

        // macOS sources are relative to an anchor only the owned creator holds;
        // this generic runner refuses them.
        #[cfg(all(test, feature = "full-tests", unix, not(target_os = "macos")))]
        mod owned_sources {
            use super::*;
            use solstone_core_journal_io::media::{
                Cleanup, MediaDirectory, MediaStage, PreparedMedia,
            };
            use std::ffi::OsStr;
            use std::os::fd::AsRawFd;

            fn prepared(
                root: &Arc<JournalRoot>,
                journal: &Path,
                budget: ArchiveBudget,
            ) -> (PreparedMedia, PathBuf, PreparedArchiveOperation<'static>) {
                let operation =
                    PreparedArchiveOperation::prepare(root.clone(), budget, &FixedClock).unwrap();
                let parent_path = journal.join("owned");
                fs::create_dir(&parent_path).unwrap();
                fs::write(
                    parent_path.join("audio.flac"),
                    b"archive original basename\n",
                )
                .unwrap();
                let parent = MediaDirectory::open(root.clone(), Path::new("owned")).unwrap();
                let digest = parent
                    .observe_file(OsStr::new("audio.flac"))
                    .unwrap()
                    .unwrap()
                    .digest;
                let activity = operation.activity().clone();
                let stage = MediaStage::create(parent.clone(), activity).unwrap();
                let path = stage.diagnostic_path();
                let media = stage
                    .create_named_target(OsStr::new("audio.flac"))
                    .unwrap()
                    .copy_from(&parent, OsStr::new("audio.flac"), &digest)
                    .unwrap();
                (media, path, operation)
            }

            fn archive_script() -> ArchiveScript {
                script(vec![
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                    snapshot(0, SNAPSHOT_ID),
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                ])
            }

            fn invalidate(path: &Path, substitute: bool) {
                use std::os::unix::fs::MetadataExt;
                let path = path.join("audio.flac");
                let before = fs::metadata(&path).unwrap();
                if substitute {
                    fs::rename(&path, path.with_file_name("retained-original")).unwrap();
                }
                fs::write(&path, b"different original bytes!\n").unwrap();
                let after = fs::metadata(path).unwrap();
                assert_eq!(after.len(), before.len());
                if !substitute {
                    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                }
            }

            #[test]
            fn exact_named_source_transport_is_only_passed_to_forced_backup() {
                let (journal, root) = fixture();
                let (mut media, stage, operation) = prepared(
                    &root,
                    journal.path(),
                    ArchiveBudget::new(&FixedClock).unwrap(),
                );
                let source = media.archive_source().unwrap();
                let exact_path = source.path().to_str().unwrap().to_owned();
                let descriptor = source.descriptor().map(|fd| fd.as_raw_fd());
                let original_flags = source
                    .descriptor()
                    .map(|fd| nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETFD).unwrap());
                let expected = exact_path.clone();
                let mut runner = archive_script();
                runner.hook = Some(Box::new(move |request| {
                    if request.argv.iter().any(|arg| arg == "backup") {
                        assert!(request.argv.iter().any(|arg| arg == OsStr::new(&expected)));
                        assert!(request.argv.iter().any(|arg| arg == "--force"));
                        assert!(request.argv.iter().any(|arg| arg == "--no-cache"));
                        assert_eq!(
                            request
                                .pass_fds
                                .iter()
                                .map(|fd| fd.as_raw_fd())
                                .collect::<Vec<_>>(),
                            descriptor.into_iter().collect::<Vec<_>>()
                        );
                        assert_eq!(fs::read(&expected).unwrap(), b"archive original basename\n");
                    } else {
                        assert!(request.pass_fds.is_empty());
                    }
                }));
                let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                let mut admitted = admit_prepared_archive(operation, &services).unwrap();
                assert_eq!(
                    admitted
                        .archive_owned(std::slice::from_ref(&source))
                        .unwrap(),
                    ArchiveSnapshot {
                        repository_id: REPOSITORY_ID.into(),
                        snapshot_id: SNAPSHOT_ID.into()
                    }
                );
                assert_eq!(source.name(), OsStr::new("audio.flac"));
                source.revalidate().unwrap();
                assert_eq!(
                    source.descriptor().map(|fd| nix::fcntl::fcntl(
                        fd,
                        nix::fcntl::FcntlArg::F_GETFD
                    )
                    .unwrap()),
                    original_flags
                );
                assert!(stage.exists());
                assert_eq!(runner.requests.borrow().len(), 7);
                drop(source);
                assert_eq!(media.cleanup(), Cleanup::Complete);
            }

            #[test]
            fn changed_or_substituted_source_refuses_before_any_archive_query() {
                for substitute in [false, true] {
                    let (journal, root) = fixture();
                    let (mut media, stage, operation) = prepared(
                        &root,
                        journal.path(),
                        ArchiveBudget::new(&FixedClock).unwrap(),
                    );
                    let source = media.archive_source().unwrap();
                    let runner = archive_script();
                    let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                    let mut admitted = admit_prepared_archive(operation, &services).unwrap();
                    invalidate(&stage, substitute);
                    assert_eq!(
                        admitted.archive_owned(&[source]).unwrap_err(),
                        ArchiveFailure {
                            error: ArchiveError::SourceUnavailable,
                            remote_effects: ArchiveRemoteEffects::NotAttempted
                        }
                    );
                    assert_eq!(runner.requests.borrow().len(), 2);
                    assert!(stage.exists());
                }
            }

            #[test]
            fn source_changes_during_either_pair_or_final_query_refuse_with_exact_effects() {
                for cut in [2, 3, 5, 6] {
                    for substitute in [false, true] {
                        let (journal, root) = fixture();
                        let (mut media, stage, operation) = prepared(
                            &root,
                            journal.path(),
                            ArchiveBudget::new(&FixedClock).unwrap(),
                        );
                        let source = media.archive_source().unwrap();
                        let hook_path = stage.clone();
                        let count = Cell::new(0);
                        let mut runner = archive_script();
                        runner.hook = Some(Box::new(move |_| {
                            let index = count.get();
                            count.set(index + 1);
                            if index == cut {
                                invalidate(&hook_path, substitute);
                            }
                        }));
                        let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                        let mut admitted = admit_prepared_archive(operation, &services).unwrap();
                        assert_eq!(
                            admitted.archive_owned(&[source]).unwrap_err(),
                            ArchiveFailure {
                                error: ArchiveError::SourceUnavailable,
                                remote_effects: if cut < 4 {
                                    ArchiveRemoteEffects::NotAttempted
                                } else {
                                    ArchiveRemoteEffects::Attempted {
                                        returncode: Some(0),
                                        observed_snapshot_id: Some(SNAPSHOT_ID.into()),
                                    }
                                }
                            }
                        );
                        assert_eq!(runner.requests.borrow().len(), if cut < 4 { 4 } else { 7 });
                        assert!(stage.exists());
                    }
                }
            }

            #[test]
            fn backup_source_failure_preserves_nonzero_or_unknown_effects_and_exact_cancel() {
                for code in [Some(0), Some(3), None] {
                    for cancelled in [false, true] {
                        let (journal, root) = fixture();
                        let cancellation = ArchiveCancellation::new();
                        let budget =
                            ArchiveBudget::new_cancellable(&FixedClock, cancellation.clone())
                                .unwrap();
                        let (mut media, stage, operation) = prepared(&root, journal.path(), budget);
                        let source = media.archive_source().unwrap();
                        let stop = cancellation.clone();
                        let hook_path = stage.clone();
                        let mut runner = archive_script();
                        runner.outputs.borrow_mut()[4] = match code {
                            Some(code) => Ok(snapshot(code, SNAPSHOT_ID)),
                            None => Err(io::Error::other("unknown backup completion")),
                        };
                        runner.hook = Some(Box::new(move |request| {
                            if request.argv.iter().any(|arg| arg == "backup") {
                                invalidate(&hook_path, false);
                                if cancelled {
                                    stop.cancel();
                                }
                            }
                        }));
                        let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                        let mut admitted = admit_prepared_archive(operation, &services).unwrap();
                        assert_eq!(
                            admitted.archive_owned(&[source]).unwrap_err(),
                            ArchiveFailure {
                                error: if cancelled {
                                    ArchiveError::Cancelled
                                } else {
                                    ArchiveError::SourceUnavailable
                                },
                                remote_effects: ArchiveRemoteEffects::Attempted {
                                    returncode: code,
                                    observed_snapshot_id: code.map(|_| SNAPSHOT_ID.into())
                                }
                            }
                        );
                        assert_eq!(runner.requests.borrow().len(), 5);
                        assert!(stage.exists());
                    }
                }
            }
        }

        #[test]
        fn inherited_budget_keeps_elapsed_time_expiry_and_cancellation_through_readback() {
            for capped in [false, true] {
                let (_journal, root) = fixture();
                let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
                let now = Rc::new(Cell::new(50));
                let clock = MutableClock(Rc::clone(&now));
                let cancellation = ArchiveCancellation::new();
                let mut budget =
                    ArchiveBudget::new_cancellable(&clock, cancellation.clone()).unwrap();
                let deadline = 50 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64;
                if capped {
                    budget.cap_credential_expiry(deadline - 2);
                }
                now.set(deadline - 5);
                let services = services(&runner, &PanicHttp, &clock, &Maintenance);
                let admitted = admit_archive_with_budget(root, &services, budget).unwrap();
                let seconds = if capped { 3 } else { 5 };
                assert!(
                    runner
                        .requests
                        .borrow()
                        .iter()
                        .all(|r| r.timeout == Some(Duration::from_secs(seconds)))
                );
                let carrier = admitted.carrier().clone();
                assert_eq!(
                    admitted.operation.budget().remaining(&clock).unwrap(),
                    Duration::from_secs(seconds)
                );
                now.set(if capped { deadline - 2 } else { deadline });
                let failure = admitted
                    .into_read_operation()
                    .err()
                    .expect("original lifetime must expire");
                assert!(Arc::ptr_eq(failure.carrier(), &carrier));
                assert_eq!(
                    failure.reason(),
                    solstone_core_backup::archive_readback::ReadbackError::Admission(if capped {
                        AdmissionError::CredentialsExpired
                    } else {
                        AdmissionError::DeadlineExceeded
                    })
                );
            }
        }

        #[test]
        fn inherited_cancelled_or_poisoned_budget_refuses_before_any_transport() {
            for cancelled in [false, true] {
                let (_journal, root) = fixture();
                let now = Rc::new(Cell::new(50));
                let clock = MutableClock(Rc::clone(&now));
                let cancellation = ArchiveCancellation::new();
                let budget = ArchiveBudget::new_cancellable(&clock, cancellation.clone()).unwrap();
                let expected = if cancelled {
                    cancellation.cancel();
                    ArchiveError::Cancelled
                } else {
                    now.set(49);
                    assert_eq!(budget.remaining(&clock), Err(AdmissionError::ClockChanged));
                    now.set(51);
                    ArchiveError::ClockChanged
                };
                let services = services(&PanicRunner, &PanicHttp, &clock, &Maintenance);
                assert_eq!(
                    refusal(admit_archive_with_budget(root, &services, budget)).error(),
                    expected
                );
            }
        }

        #[test]
        fn cancellation_on_either_pairing_leg_refuses_exactly_without_next_query() {
            for leg in [0, 1] {
                for fails in [false, true] {
                    let (_journal, root) = fixture();
                    let cancellation = ArchiveCancellation::new();
                    let mut runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
                    let count = Rc::new(Cell::new(0));
                    let observed = count.clone();
                    let stop = cancellation.clone();
                    runner.hook = Some(Box::new(move |_| {
                        let index = observed.get();
                        observed.set(index + 1);
                        if index == leg {
                            stop.cancel();
                        }
                    }));
                    if fails {
                        runner.outputs.borrow_mut()[leg] =
                            Err(io::Error::other("interrupted tool"));
                    }
                    let budget = ArchiveBudget::new_cancellable(&FixedClock, cancellation).unwrap();
                    let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                    assert_eq!(
                        refusal(admit_archive_with_budget(root, &services, budget)).error(),
                        ArchiveError::Cancelled
                    );
                    assert_eq!(count.get(), leg + 1);
                }
            }
        }

        #[test]
        fn unsupported_cancellable_runner_and_http_never_fall_back_to_ordinary_execution() {
            let (_journal, root) = fixture();
            let budget =
                ArchiveBudget::new_cancellable(&FixedClock, ArchiveCancellation::new()).unwrap();
            let services = services(&PanicRunner, &PanicHttp, &FixedClock, &Maintenance);
            assert_eq!(
                refusal(admit_archive_with_budget(root.clone(), &services, budget)).error(),
                ArchiveError::ToolFailed { reason: "failed" }
            );
            let (journal, root) = fixture();
            let _ = hosted(journal.path(), "1970-01-01T00:02:00Z");
            let budget =
                ArchiveBudget::new_cancellable(&FixedClock, ArchiveCancellation::new()).unwrap();
            assert!(matches!(
                refusal(admit_archive_with_budget(root, &services, budget)).error(),
                ArchiveError::CredentialsUnavailable { .. }
            ));
        }

        #[test]
        fn cancelled_archive_preserves_actual_output_or_unknown_attempt_before_refusal() {
            for code in [Some(0), Some(3), None] {
                let (journal, root) = fixture();
                let cancellation = ArchiveCancellation::new();
                let mut runner = script(vec![
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                    snapshot(code.unwrap_or(0), SNAPSHOT_ID),
                ]);
                if code.is_none() {
                    runner.outputs.borrow_mut()[4] = Err(io::Error::other("interrupted backup"));
                }
                let stop = cancellation.clone();
                runner.hook = Some(Box::new(move |request| {
                    if request.argv.iter().any(|arg| arg == "backup") {
                        stop.cancel();
                    }
                }));
                let budget = ArchiveBudget::new_cancellable(&FixedClock, cancellation).unwrap();
                let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                let mut admitted = admit_archive_with_budget(root, &services, budget).unwrap();
                assert_eq!(
                    admitted.archive(&source(journal.path())).unwrap_err(),
                    ArchiveFailure {
                        error: ArchiveError::Cancelled,
                        remote_effects: ArchiveRemoteEffects::Attempted {
                            returncode: code,
                            observed_snapshot_id: code.map(|_| SNAPSHOT_ID.to_owned())
                        }
                    }
                );
                assert_eq!(runner.requests.borrow().len(), 5);
                let failure = admitted.into_read_operation().err().unwrap();
                assert_eq!(
                    failure.reason(),
                    solstone_core_backup::archive_readback::ReadbackError::Admission(
                        AdmissionError::Cancelled
                    )
                );
            }
        }

        #[test]
        fn expired_readback_handoff_retains_original_admission() {
            let (_journal, root) = fixture();
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            let now = Rc::new(Cell::new(50));
            let clock = MutableClock(Rc::clone(&now));
            let services = services(&runner, &PanicHttp, &clock, &Maintenance);
            let admitted = admit_archive(Arc::clone(&root), &services).unwrap();
            let carrier = admitted.carrier().clone();
            assert!(Arc::ptr_eq(admitted.operation.journal_root(), &root));
            now.set(50 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64);
            let failure = admitted
                .into_read_operation()
                .err()
                .expect("original deadline must refuse");
            assert!(Arc::ptr_eq(failure.carrier(), &carrier));
            assert_eq!(
                failure.reason(),
                solstone_core_backup::archive_readback::ReadbackError::Admission(
                    AdmissionError::DeadlineExceeded
                )
            );
            assert_eq!(runner.requests.borrow().len(), 2);
        }

        #[test]
        fn readback_transfer_cannot_renew_expiry_or_poisoned_clock() {
            for poison in [false, true] {
                let (journal, root) = fixture();
                let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
                let http = hosted(journal.path(), "1970-01-01T00:01:00Z");
                let now = Rc::new(Cell::new(50));
                let clock = MutableClock(Rc::clone(&now));
                let mut services = services(&runner, &http, &clock, &Maintenance);
                services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
                let admitted = admit_archive(root, &services).unwrap();
                let expected = if poison {
                    now.set(49);
                    assert_eq!(admitted.revalidate_local(), Err(ArchiveError::ClockChanged));
                    now.set(55);
                    AdmissionError::ClockChanged
                } else {
                    now.set(60);
                    AdmissionError::CredentialsExpired
                };
                let failure = admitted.into_read_operation().err().unwrap();
                assert_eq!(
                    failure.reason(),
                    solstone_core_backup::archive_readback::ReadbackError::Admission(expected)
                );
                assert_eq!(runner.requests.borrow().len(), 2);
                assert_eq!(http.requests.borrow().len(), 1);
            }
        }

        #[test]
        fn readback_transfer_refuses_extra_read_options_before_actual_execution() {
            let (_journal, root) = fixture();
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let mut admitted = admit_archive(root, &services).unwrap();
            admitted
                .read
                .global_options
                .push("--password-command=helper".into());
            let failure = admitted.into_read_operation().err().unwrap();
            assert_eq!(
                failure.reason(),
                solstone_core_backup::archive_readback::ReadbackError::UnsupportedBackend
            );
            assert_eq!(runner.requests.borrow().len(), 2);
        }

        #[test]
        fn byo_pairs_and_archives_with_captured_transport_and_forced_source_read() {
            let (journal, root) = fixture();
            let runner = script(vec![
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                snapshot(0, SNAPSHOT_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
            ]);
            let mut services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let mut admitted = admit_archive(Arc::clone(&root), &services).unwrap();
            services.restic = Some(Path::new("/replacement/restic").into());
            assert_eq!(
                services.restic_path().unwrap(),
                Path::new("/replacement/restic")
            );
            assert!(Arc::ptr_eq(admitted.journal_root(), &root));
            assert_eq!(admitted.repository_id(), REPOSITORY_ID);
            let result = admitted.archive(&source(journal.path())).unwrap();
            assert_eq!(
                result,
                ArchiveSnapshot {
                    repository_id: REPOSITORY_ID.into(),
                    snapshot_id: SNAPSHOT_ID.into()
                }
            );
            let requests = runner.requests.borrow();
            assert_eq!(requests.len(), 7);
            let archive = &requests[4];
            assert!(archive.argv.iter().any(|arg| arg == "--force"));
            assert!(
                archive
                    .argv
                    .windows(2)
                    .any(|pair| pair == ["--tag", ARCHIVE_TAG])
            );
            assert!(
                archive
                    .argv
                    .iter()
                    .any(|arg| arg == &crate::restic_filesystem_path(&source(journal.path())[0]))
            );
            assert!(
                requests
                    .iter()
                    .all(|request| request.env["RESTIC_REPOSITORY"] == "s3:repo")
            );
            assert!(!requests.iter().any(|request| {
                request
                    .argv
                    .iter()
                    .any(|arg| matches!(arg.as_str(), "init" | "unlock" | "forget" | "prune"))
            }));
            assert!(runner.outputs.borrow().is_empty());
            assert!(
                runner
                    .programs
                    .borrow()
                    .iter()
                    .all(|program| program == "/fixture/bin/restic")
            );
        }

        #[test]
        fn operated_pairs_distinct_transports_from_one_credential_response() {
            let (journal, root) = fixture();
            let http = hosted(journal.path(), "1970-01-01T02:00:00Z");
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            let mut services = services(&runner, &http, &FixedClock, &Maintenance);
            services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
            let admitted = admit_archive(root, &services).unwrap();
            assert_eq!(admitted.repository_id(), REPOSITORY_ID);
            assert_eq!(http.requests.borrow().len(), 1);
            let requests = runner.requests.borrow();
            assert_eq!(
                requests[0].env["RESTIC_REPOSITORY"],
                "rclone:spb:bucket/prefix"
            );
            assert_eq!(
                requests[1].env["RESTIC_REPOSITORY"],
                "s3:https://s3.example/bucket/prefix"
            );
            assert_eq!(
                requests[0].env["RCLONE_CONFIG_SPB_ACCESS_KEY_ID"],
                requests[1].env["AWS_ACCESS_KEY_ID"]
            );
            assert!(
                requests[0]
                    .argv
                    .iter()
                    .any(|arg| arg.contains("--append-only"))
            );
            assert!(
                requests.iter().all(|request| request.timeout
                    == Some(Duration::from_secs(ARCHIVE_LS_TIMEOUT_SECONDS)))
            );
        }

        #[test]
        fn installed_operated_creator_without_append_only_transport_never_asks_the_broker() {
            let (journal, root) = fixture();
            let _http = hosted(journal.path(), "1970-01-01T02:00:00Z");
            let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
            let prepared = OwnedPreparedArchiveOperation::prepare(
                root,
                ArchiveBudget::new(clock.as_ref()).unwrap(),
                Arc::clone(&clock),
            )
            .unwrap_or_else(|e| panic!("prepare: {}", e.reason()));
            let Err(failure) = admit_prepared_archive_owned_creator_installed(
                prepared,
                Path::new("/fixture/bin/restic"),
                None,
                "test",
                &UreqHttpTransport,
            ) else {
                panic!("operated creator admitted without its append-only transport");
            };
            assert_eq!(failure.error(), ArchiveError::ToolUnavailable);
            assert!(!failure.effects().broker_requested);
        }

        #[test]
        fn repository_identity_requires_full_canonical_matching_ids() {
            for id in [
                "",
                "latest",
                "aaaaaaaa",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
            ] {
                let (_journal, root) = fixture();
                let runner = script(vec![repo(id)]);
                let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                assert_eq!(
                    refusal(admit_archive(root, &services)).error(),
                    ArchiveError::RepositoryIdentityInvalid
                );
                assert_eq!(runner.requests.borrow().len(), 1);
            }
            let (_journal, root) = fixture();
            let runner = script(vec![repo(REPOSITORY_ID), repo(OTHER_ID)]);
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            assert_eq!(
                refusal(admit_archive(root, &services)).error(),
                ArchiveError::RepositoryIdentityChanged
            );
        }

        #[test]
        fn repository_metadata_and_helper_failures_refuse_without_leaking_output() {
            for body in [
                "{}",
                "{",
                "{\"id\":1}",
                "{\"id\":\"secret\"}",
                "{\"id\":\"a\",\"id\":\"b\"}",
            ] {
                let (_journal, root) = fixture();
                let runner = script(vec![output(0, body)]);
                let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                let error = refusal(admit_archive(root, &services)).error();
                assert_eq!(error, ArchiveError::RepositoryIdentityInvalid);
                assert!(!format!("{error:?} {error}").contains("secret"));
            }
            for code in [1, 3, 10, 12, 124] {
                let (_journal, root) = fixture();
                let runner = script(vec![output(code, "secret")]);
                let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                assert_eq!(
                    refusal(admit_archive(root, &services)).error(),
                    ArchiveError::ToolFailed {
                        reason: reason_for_returncode(code)
                    }
                );
            }
            let (_journal, root) = fixture();
            let runner = script(vec![]);
            runner
                .outputs
                .borrow_mut()
                .push_back(Err(io::Error::other("secret")));
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            assert_eq!(
                refusal(admit_archive(root, &services)).error(),
                ArchiveError::ToolFailed { reason: "failed" }
            );
        }

        #[test]
        fn expiry_is_parsed_and_enforced_before_any_tool_launch() {
            for (expiry, expected) in [
                ("later", ArchiveError::InvalidCredentialExpiry),
                ("1970-01-01T00:00:49Z", ArchiveError::CredentialsExpired),
                ("1970-01-01T00:00:50Z", ArchiveError::CredentialsExpired),
            ] {
                let (journal, root) = fixture();
                let http = hosted(journal.path(), expiry);
                let mut services = services(&PanicRunner, &http, &FixedClock, &Maintenance);
                services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
                assert_eq!(refusal(admit_archive(root, &services)).error(), expected);
                assert_eq!(http.requests.borrow().len(), 1);
            }
        }

        #[test]
        fn expiry_bounds_queries_and_never_renews_an_admitted_session() {
            let (journal, root) = fixture();
            let now = Rc::new(Cell::new(50));
            let clock = MutableClock(Rc::clone(&now));
            let http = hosted(journal.path(), "1970-01-01T00:01:00Z");
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            let mut services = services(&runner, &http, &clock, &Maintenance);
            services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
            let mut admitted = admit_archive(root, &services).unwrap();
            assert!(
                runner
                    .requests
                    .borrow()
                    .iter()
                    .all(|request| request.timeout == Some(Duration::from_secs(10)))
            );
            now.set(60);
            let failure = admitted.archive(&source(journal.path())).unwrap_err();
            assert_eq!(failure.error, ArchiveError::CredentialsExpired);
            assert_eq!(failure.remote_effects, ArchiveRemoteEffects::NotAttempted);
            assert_eq!(runner.requests.borrow().len(), 2);
            assert_eq!(http.requests.borrow().len(), 1);
        }

        #[test]
        fn broker_request_consumes_the_same_operation_budget() {
            struct TimedHttp<'a> {
                inner: &'a ScriptedHttp,
                now: Rc<Cell<i64>>,
            }
            impl HttpTransport for TimedHttp<'_> {
                fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
                    self.now.set(50 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64);
                    self.inner.execute(request)
                }
                fn execute_cancellable(
                    &self,
                    request: &HttpRequest,
                    cancellation: &ArchiveCancellation,
                ) -> Result<HttpResponse, HttpError> {
                    assert!(!cancellation.is_cancelled());
                    self.execute(request)
                }
            }
            let (journal, root) = fixture();
            let http = hosted(journal.path(), "1970-01-02T00:00:00Z");
            let now = Rc::new(Cell::new(50));
            let clock = MutableClock(Rc::clone(&now));
            let timed_http = TimedHttp {
                inner: &http,
                now: Rc::clone(&now),
            };
            let mut services = services(&PanicRunner, &timed_http, &clock, &Maintenance);
            services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
            assert_eq!(
                refusal(admit_archive(root, &services)).error(),
                ArchiveError::DeadlineExceeded
            );
            assert_eq!(http.requests.borrow().len(), 1);

            let budget = ArchiveBudget::new(&FixedClock).unwrap();
            now.set(FixedClock.now_unix() + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64 - 5);
            let (_bounded_journal, bounded_root) = fixture();
            let operation =
                PreparedArchiveOperation::prepare(bounded_root, budget, &clock).unwrap();
            let http = ScriptedHttp {
                requests: RefCell::new(vec![]),
                responses: RefCell::new(VecDeque::from([Ok(credentials_response())])),
            };
            let bounded = ArchiveAdmissionHttp {
                inner: &http,
                operation: &operation,
            };
            let request = HttpRequest {
                method: "POST".into(),
                url: "https://broker".into(),
                headers: vec![],
                body: vec![],
                timeout: Duration::from_secs(30),
            };
            bounded.execute(&request).unwrap();
            assert_eq!(http.requests.borrow()[0].timeout, Duration::from_secs(5));
        }

        #[test]
        fn cumulative_deadline_includes_admission_idle_time_and_every_command() {
            let (journal, root) = fixture();
            let now = Rc::new(Cell::new(50));
            let clock = MutableClock(Rc::clone(&now));
            let mut runner = script(vec![
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                snapshot(0, SNAPSHOT_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
            ]);
            let advance = Rc::clone(&now);
            runner.hook = Some(Box::new(move |_| advance.set(advance.get() + 10)));
            let services = services(&runner, &PanicHttp, &clock, &Maintenance);
            let mut admitted = admit_archive(root, &services).unwrap();
            let deadline = 50 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64;
            now.set(deadline - 100);
            admitted.archive(&source(journal.path())).unwrap();
            let timeouts: Vec<_> = runner
                .requests
                .borrow()
                .iter()
                .map(|request| request.timeout.unwrap().as_secs())
                .collect();
            assert_eq!(&timeouts[2..], &[100, 90, 80, 70, 60]);
            now.set(deadline);
            assert_eq!(
                admitted.archive(&source(journal.path())).unwrap_err().error,
                ArchiveError::DeadlineExceeded
            );
            assert_eq!(runner.requests.borrow().len(), 7);
        }

        #[test]
        fn clock_reversal_refuses_instead_of_replenishing_budget() {
            let (_journal, root) = fixture();
            let now = Rc::new(Cell::new(50));
            let clock = MutableClock(Rc::clone(&now));
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            let services = services(&runner, &PanicHttp, &clock, &Maintenance);
            let admitted = admit_archive(root, &services).unwrap();
            now.set(49);
            assert_eq!(admitted.revalidate(), Err(ArchiveError::ClockChanged));
            now.set(50);
            assert_eq!(admitted.revalidate(), Err(ArchiveError::ClockChanged));
            assert_eq!(runner.requests.borrow().len(), 2);
        }

        #[test]
        fn status_churn_is_allowed_but_each_transport_setting_change_refuses() {
            for key in [
                "enabled",
                "mode",
                "destination",
                "daily_key",
                "recovery_key",
            ] {
                let (journal, root) = fixture();
                let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
                let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                let admitted = admit_archive(root, &services).unwrap();
                let mut config = valid_backup_section(valid_destination());
                config[key] = match key {
                    "enabled" => json!(false),
                    "mode" => json!("other"),
                    "destination" => json!({"repository": "s3:other"}),
                    _ => json!("changed"),
                };
                write_backup_section(journal.path(), config);
                assert_eq!(
                    admitted.revalidate(),
                    Err(ArchiveError::ConfigurationChanged)
                );
                assert_eq!(runner.requests.borrow().len(), 2);
            }
            let (journal, root) = fixture();
            let runner = script(vec![
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
            ]);
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let admitted = admit_archive(root, &services).unwrap();
            let mut config = valid_backup_section(valid_destination());
            config["last_backup"] = json!({"status": "ok", "snapshot_id": SNAPSHOT_ID});
            write_backup_section(journal.path(), config);
            admitted.revalidate().unwrap();
        }

        #[test]
        fn local_revalidation_never_launches_a_remote_command() {
            let (journal, root) = fixture();
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let admitted = admit_archive(root, &services).unwrap();
            admitted.revalidate_local().unwrap();
            assert_eq!(runner.requests.borrow().len(), 2);

            let mut config = valid_backup_section(valid_destination());
            config["enabled"] = json!(false);
            write_backup_section(journal.path(), config);
            assert_eq!(
                admitted.revalidate_local(),
                Err(ArchiveError::ConfigurationChanged)
            );
            assert_eq!(runner.requests.borrow().len(), 2);
            assert!(runner.outputs.borrow().is_empty());
        }

        #[test]
        fn missing_configuration_or_binding_refuses_before_remote_work() {
            for relative in ["config/journal.json", "backup/hosted/binding.json"] {
                let (journal, root) = fixture();
                let _http = hosted(journal.path(), "1970-01-01T02:00:00Z");
                fs::remove_file(journal.path().join(relative)).unwrap();
                let services = services(&PanicRunner, &PanicHttp, &FixedClock, &Maintenance);
                assert_eq!(
                    refusal(admit_archive(root, &services)).error(),
                    ArchiveError::ConfigurationUnavailable
                );
            }
        }

        #[test]
        fn configuration_and_binding_require_bounded_json_objects() {
            for relative in ["config/journal.json", "backup/hosted/binding.json"] {
                for contents in ["", "{", "[]", "null", "42"] {
                    let (journal, root) = fixture();
                    let _http = hosted(journal.path(), "1970-01-01T02:00:00Z");
                    fs::write(journal.path().join(relative), contents).unwrap();
                    let services = services(&PanicRunner, &PanicHttp, &FixedClock, &Maintenance);
                    assert_eq!(
                        refusal(admit_archive(root, &services)).error(),
                        ArchiveError::ConfigurationUnavailable
                    );
                }
                let (journal, root) = fixture();
                let _http = hosted(journal.path(), "1970-01-01T02:00:00Z");
                let path = journal.path().join(relative);
                let mut oversized: Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                oversized["padding"] = json!("x".repeat(2 * 1024 * 1024));
                fs::write(path, serde_json::to_vec(&oversized).unwrap()).unwrap();
                let services = services(&PanicRunner, &PanicHttp, &FixedClock, &Maintenance);
                assert_eq!(
                    refusal(admit_archive(root, &services)).error(),
                    ArchiveError::ConfigurationUnavailable
                );
            }
        }

        #[test]
        fn retained_binding_preserves_nonblank_field_requirements() {
            for key in [
                "broker_endpoint",
                "account_id",
                "instance_id",
                "bucket",
                "prefix",
                "broker_token",
            ] {
                for value in [Value::Null, json!(" \t"), json!(1)] {
                    let (journal, root) = fixture();
                    let _http = hosted(journal.path(), "1970-01-01T02:00:00Z");
                    let path = journal.path().join("backup/hosted/binding.json");
                    let mut binding: Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    binding[key] = value;
                    fs::write(&path, serde_json::to_vec(&binding).unwrap()).unwrap();
                    let services = services(&PanicRunner, &PanicHttp, &FixedClock, &Maintenance);
                    assert_eq!(
                        refusal(admit_archive(root, &services)).error(),
                        ArchiveError::ConfigurationUnavailable
                    );
                }
            }
        }

        #[cfg(unix)]
        #[test]
        fn symlinked_configuration_and_binding_descendants_refuse() {
            for relative in [
                "config",
                "config/journal.json",
                "backup/hosted",
                "backup/hosted/binding.json",
            ] {
                let (journal, root) = fixture();
                let _http = hosted(journal.path(), "1970-01-01T02:00:00Z");
                let path = journal.path().join(relative);
                let retained = path.with_extension("retained");
                fs::rename(&path, &retained).unwrap();
                std::os::unix::fs::symlink(&retained, &path).unwrap();
                let services = services(&PanicRunner, &PanicHttp, &FixedClock, &Maintenance);
                assert_eq!(
                    refusal(admit_archive(root, &services)).error(),
                    ArchiveError::ConfigurationUnavailable
                );
            }
        }

        #[cfg(unix)]
        #[test]
        fn nonplain_configuration_and_binding_files_refuse_without_blocking() {
            for relative in ["config/journal.json", "backup/hosted/binding.json"] {
                for fifo in [false, true] {
                    let (journal, root) = fixture();
                    let _http = hosted(journal.path(), "1970-01-01T02:00:00Z");
                    let path = journal.path().join(relative);
                    fs::remove_file(&path).unwrap();
                    if fifo {
                        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRUSR).unwrap();
                    } else {
                        fs::create_dir(&path).unwrap();
                    }
                    let services = services(&PanicRunner, &PanicHttp, &FixedClock, &Maintenance);
                    assert_eq!(
                        refusal(admit_archive(root, &services)).error(),
                        ArchiveError::ConfigurationUnavailable
                    );
                }
            }
        }

        #[test]
        fn replacement_configuration_or_binding_refuses_during_admission() {
            for replace_binding in [false, true] {
                let (journal, root) = fixture();
                let http = hosted(journal.path(), "1970-01-01T02:00:00Z");
                let mut runner = script(vec![repo(REPOSITORY_ID)]);
                let journal_path = journal.path().to_path_buf();
                runner.hook = Some(Box::new(move |_| {
                    let relative = if replace_binding {
                        "backup/hosted/binding.json"
                    } else {
                        "config/journal.json"
                    };
                    let path = journal_path.join(relative);
                    fs::rename(&path, path.with_extension("retained")).unwrap();
                    if replace_binding {
                        let mut binding = hosted_binding("https://broker");
                        binding.prefix = "replacement".into();
                        solstone_core_backup::save_hosted_binding(&journal_path, &binding).unwrap();
                    } else {
                        let mut config = valid_backup_section(valid_destination());
                        config["mode"] = json!("operated");
                        config["enabled"] = json!(false);
                        write_backup_section(&journal_path, config);
                    }
                }));
                let mut services = services(&runner, &http, &FixedClock, &Maintenance);
                services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
                assert_eq!(
                    refusal(admit_archive(root, &services)).error(),
                    ArchiveError::ConfigurationChanged
                );
                assert_eq!(runner.requests.borrow().len(), 1);
                assert_eq!(http.requests.borrow().len(), 1);
            }
        }

        #[test]
        fn binding_change_is_not_replaced_by_retained_byo_destination() {
            let (journal, root) = fixture();
            let http = hosted(journal.path(), "1970-01-01T02:00:00Z");
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            let mut services = services(&runner, &http, &FixedClock, &Maintenance);
            services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
            let admitted = admit_archive(root, &services).unwrap();
            let mut binding = hosted_binding("https://broker");
            binding.prefix = "other".into();
            solstone_core_backup::save_hosted_binding(journal.path(), &binding).unwrap();
            assert_eq!(
                admitted.revalidate(),
                Err(ArchiveError::ConfigurationChanged)
            );
            assert_eq!(runner.requests.borrow().len(), 2);
            assert_eq!(http.requests.borrow().len(), 1);
        }

        #[cfg(unix)]
        #[test]
        fn root_path_replacement_never_reads_the_replacement_configuration() {
            let parent = tempfile::tempdir().unwrap();
            let journal = parent.path().join("journal");
            fs::create_dir(&journal).unwrap();
            write_backup_section(&journal, valid_backup_section(valid_destination()));
            let root = Arc::new(JournalRoot::open(&journal).unwrap());
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let admitted = admit_archive(root, &services).unwrap();
            fs::rename(&journal, parent.path().join("retained")).unwrap();
            fs::create_dir(&journal).unwrap();
            write_backup_section(&journal, valid_backup_section(valid_destination()));
            assert_eq!(
                admitted.revalidate(),
                Err(ArchiveError::JournalIdentityChanged)
            );
            assert_eq!(runner.requests.borrow().len(), 2);
        }

        #[test]
        fn repository_replacement_after_admission_refuses_before_archive() {
            let (journal, root) = fixture();
            let runner = script(vec![
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(OTHER_ID),
            ]);
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let mut admitted = admit_archive(root, &services).unwrap();
            let failure = admitted.archive(&source(journal.path())).unwrap_err();
            assert_eq!(failure.error, ArchiveError::RepositoryIdentityChanged);
            assert_eq!(failure.remote_effects, ArchiveRemoteEffects::NotAttempted);
            assert_eq!(runner.requests.borrow().len(), 3);
        }

        #[test]
        fn post_archive_repository_drift_preserves_observed_snapshot() {
            let (journal, root) = fixture();
            let runner = script(vec![
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                snapshot(0, SNAPSHOT_ID),
                repo(OTHER_ID),
            ]);
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let mut admitted = admit_archive(root, &services).unwrap();
            let failure = admitted.archive(&source(journal.path())).unwrap_err();
            assert_eq!(failure.error, ArchiveError::RepositoryIdentityChanged);
            assert_eq!(
                failure.remote_effects,
                ArchiveRemoteEffects::Attempted {
                    returncode: Some(0),
                    observed_snapshot_id: Some(SNAPSHOT_ID.into())
                }
            );
        }

        #[test]
        fn post_archive_configuration_drift_preserves_observed_snapshot() {
            let (journal, root) = fixture();
            let mut runner = script(vec![
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                repo(REPOSITORY_ID),
                snapshot(0, SNAPSHOT_ID),
            ]);
            let journal_path = journal.path().to_path_buf();
            runner.hook = Some(Box::new(move |request| {
                if request.argv.iter().any(|arg| arg == "backup") {
                    let mut config = valid_backup_section(valid_destination());
                    config["enabled"] = json!(false);
                    write_backup_section(&journal_path, config);
                }
            }));
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let mut admitted = admit_archive(root, &services).unwrap();
            let failure = admitted.archive(&source(journal.path())).unwrap_err();
            assert_eq!(failure.error, ArchiveError::ConfigurationChanged);
            assert_eq!(
                failure.remote_effects,
                ArchiveRemoteEffects::Attempted {
                    returncode: Some(0),
                    observed_snapshot_id: Some(SNAPSHOT_ID.into())
                }
            );
            assert_eq!(runner.requests.borrow().len(), 5);
        }

        #[test]
        fn incomplete_and_malformed_archives_never_return_a_successful_snapshot() {
            for (code, id, expected) in [
                (
                    3,
                    SNAPSHOT_ID,
                    ArchiveError::ToolFailed {
                        reason: "incomplete",
                    },
                ),
                (
                    1,
                    SNAPSHOT_ID,
                    ArchiveError::ToolFailed { reason: "failed" },
                ),
                (0, "", ArchiveError::SnapshotIdentityInvalid),
                (0, "latest", ArchiveError::SnapshotIdentityInvalid),
                (0, "cccccccc", ArchiveError::SnapshotIdentityInvalid),
            ] {
                let (journal, root) = fixture();
                let runner = script(vec![
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                    snapshot(code, id),
                    repo(REPOSITORY_ID),
                    repo(REPOSITORY_ID),
                ]);
                let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
                let mut admitted = admit_archive(root, &services).unwrap();
                let failure = admitted.archive(&source(journal.path())).unwrap_err();
                assert_eq!(failure.error, expected);
                assert_eq!(
                    failure.remote_effects,
                    ArchiveRemoteEffects::Attempted {
                        returncode: Some(code),
                        observed_snapshot_id: full_archive_id(id).then(|| id.to_owned())
                    }
                );
            }
        }

        #[test]
        fn empty_and_option_shaped_selections_launch_no_commands() {
            let (_journal, root) = fixture();
            let runner = script(vec![repo(REPOSITORY_ID), repo(REPOSITORY_ID)]);
            let services = services(&runner, &PanicHttp, &FixedClock, &Maintenance);
            let mut admitted = admit_archive(root, &services).unwrap();
            for paths in [vec![], vec![PathBuf::from("--exclude=*")]] {
                assert_eq!(
                    admitted.archive(&paths).unwrap_err().error,
                    ArchiveError::InvalidSelection
                );
            }
            assert_eq!(runner.requests.borrow().len(), 2);
        }
    }

    fn assert_one_resolution_attempt() {
        assert_eq!(backup_path_resolution_attempts(), 1);
    }

    fn prepare_error(result: Result<AdmittedCapability, BackupResult>) -> BackupResult {
        match result {
            Ok(_) => panic!("backup unexpectedly admitted"),
            Err(result) => result,
        }
    }
    /// restic exit 3 wrote a snapshot and could not read every source file.
    ///
    /// 🔴 That is a backup with a caveat, not a failed backup, and the
    /// distinction is owner-visible: `status` is what drives the backup panel's
    /// copy key, so reporting `error` made a run that genuinely backed the
    /// owner's data up read as "failed". On Windows the running journal holds
    /// files open, which makes exit 3 the ORDINARY outcome there -- a working
    /// BYO backup would have shown failed every single day.
    #[test]
    fn a_partial_backup_keeps_its_snapshot_and_reports_success_with_a_caveat() {
        let journal = configured_journal();
        let runner = ObservedScript {
            outputs: RefCell::new(VecDeque::from([
                output(0, ""),
                output(
                    3,
                    concat!(
                        r#"[{"message_type":"error","item":"/journal/b.txt","#,
                        r#""error":{"message":"open /journal/b.txt: permission denied"}},"#,
                        r#"{"message_type":"error","item":"/journal/c.txt","#,
                        r#""error":{"message":"open /journal/c.txt: permission denied"}},"#,
                        r#"{"message_type":"summary","snapshot_id":"partial"}]"#
                    ),
                ),
            ])),
            requests: RefCell::new(Vec::new()),
            after_unlock: RefCell::new(None),
        };
        let services = services(&runner, &Http, &FixedClock, &Maintenance);
        let result = prepare(journal.path(), &FixedClock)
            .unwrap()
            .execute(&services);
        assert_eq!(result.status, "ok");
        assert_eq!(result.snapshot_id.as_deref(), Some("partial"));
        assert_eq!(result.error_reason.as_deref(), Some("incomplete"));
        // ✅ restic's own per-file reasons are carried, not discarded: the owner
        // line must not have to guess at a cause the tool already reported.
        let unreadable = result.unreadable.expect("restic's read failures");
        assert_eq!(unreadable.count, 2);
        assert_eq!(
            unreadable.first_reason.as_deref(),
            Some("open /journal/b.txt: permission denied")
        );
    }

    /// 🔴 restic writes its per-file error records to stderr and only the
    /// summary to stdout. Reading stdout alone told the owner "some files
    /// couldn't be read" with no count and no reason, on every partial run.
    #[test]
    fn a_partial_backup_counts_the_read_failures_restic_writes_to_stderr() {
        let journal = configured_journal();
        let runner = ObservedScript {
            outputs: RefCell::new(VecDeque::from([
                output(0, ""),
                ToolOutput {
                    returncode: 3,
                    stdout: concat!(
                        r#"{"message_type":"status","percent_done":1}"#,
                        "\n",
                        r#"{"message_type":"summary","snapshot_id":"partial"}"#,
                        "\n"
                    )
                    .as_bytes()
                    .to_vec(),
                    stderr: concat!(
                        r#"{"message_type":"error","error":{"message":"open C:\\j\\b.log: "#,
                        r#"The file is in use by another process."},"#,
                        r#""during":"archival","item":"C:\\j\\b.log"}"#,
                        "\n",
                        "a plain warning line that is not JSON\n",
                        r#"{"message_type":"error","error":{"message":"open C:\\j\\c.log: denied"},"#,
                        r#""during":"archival","item":"C:\\j\\c.log"}"#,
                        "\n"
                    )
                    .as_bytes()
                    .to_vec(),
                },
            ])),
            requests: RefCell::new(Vec::new()),
            after_unlock: RefCell::new(None),
        };
        let services = services(&runner, &Http, &FixedClock, &Maintenance);
        let result = prepare(journal.path(), &FixedClock)
            .unwrap()
            .execute(&services);
        assert_eq!(result.status, "ok");
        assert_eq!(result.snapshot_id.as_deref(), Some("partial"));
        let unreadable = result.unreadable.expect("restic's read failures on stderr");
        assert_eq!(unreadable.count, 2);
        assert_eq!(
            unreadable.first_reason.as_deref(),
            Some("open C:\\j\\b.log: The file is in use by another process.")
        );
    }

    /// ⛔ The inverse: exit 3 with no snapshot is not a backup at all.
    #[test]
    fn a_partial_run_that_wrote_no_snapshot_is_still_an_error() {
        let journal = configured_journal();
        let runner = ObservedScript {
            outputs: RefCell::new(VecDeque::from([output(0, ""), output(3, "")])),
            requests: RefCell::new(Vec::new()),
            after_unlock: RefCell::new(None),
        };
        let services = services(&runner, &Http, &FixedClock, &Maintenance);
        let result = prepare(journal.path(), &FixedClock)
            .unwrap()
            .execute(&services);
        assert_eq!(result.status, "error");
        assert_eq!(result.snapshot_id, None);
        assert_eq!(result.error_reason.as_deref(), Some("incomplete"));
    }

    #[test]
    fn prune_keeps_every_snapshot_that_can_hold_the_only_copy_of_media() {
        let journal = configured_journal();
        let runner = ObservedScript {
            outputs: RefCell::new(VecDeque::from([output(0, ""), output(0, "")])),
            requests: RefCell::new(Vec::new()),
            after_unlock: RefCell::new(None),
        };
        let services = services(&runner, &Http, &FixedClock, &Maintenance);
        assert_eq!(run_prune(journal.path(), &services).status, "ok");
        let requests = runner.requests.borrow();
        let forget = requests
            .iter()
            .find(|request| request.argv.iter().any(|arg| arg == "forget"))
            .expect("prune runs forget");
        for tag in [ARCHIVE_TAG, OFFLOAD_ARCHIVE_TAG] {
            assert!(
                forget
                    .argv
                    .windows(2)
                    .any(|pair| pair == ["--keep-tag", tag]),
                "{tag} snapshots must survive rotation: {:?}",
                forget.argv
            );
        }
        assert!(
            forget
                .argv
                .windows(2)
                .any(|pair| pair == ["--group-by", "tags"]),
            "rotation must not split one journal's snapshots by machine name or path: {:?}",
            forget.argv
        );
    }

    #[test]
    fn repository_operations_wait_for_native_locks_within_existing_deadlines() {
        let journal = configured_journal();
        let runner = ObservedScript {
            outputs: RefCell::new(VecDeque::from([
                output(0, ""),
                output(0, r#"{"message_type":"summary","snapshot_id":"snap"}"#),
                output(0, ""),
                output(0, ""),
                output(0, ""),
                output(0, ""),
                output(0, r#"{"message_type":"summary","snapshot_id":"archive"}"#),
            ])),
            requests: RefCell::new(Vec::new()),
            after_unlock: RefCell::new(None),
        };
        let services = services(&runner, &Http, &FixedClock, &Maintenance);
        assert_eq!(
            prepare(journal.path(), &FixedClock)
                .unwrap()
                .execute(&services)
                .status,
            "ok"
        );
        assert_eq!(run_prune(journal.path(), &services).status, "ok");
        assert_eq!(
            run_verification(journal.path(), &services, &FixedClock).status,
            "ok"
        );
        assert_eq!(
            run_archive_backup(journal.path(), &services, &[journal.path().join("archive")]).status,
            "ok"
        );
        let requests = runner.requests.borrow();
        assert_eq!(requests.len(), 7);
        let mut operations = Vec::new();
        for request in requests.iter() {
            let waits: Vec<_> = request
                .argv
                .windows(2)
                .filter(|args| args[0] == "--retry-lock")
                .collect();
            if request.argv.iter().any(|arg| arg == "unlock") {
                assert!(waits.is_empty());
                assert_eq!(
                    request.timeout,
                    Some(Duration::from_secs(UNLOCK_TIMEOUT_SECONDS))
                );
                continue;
            }
            assert_eq!(waits.len(), 1, "{:?}", request.argv);
            assert_eq!(waits[0][1], "30m");
            let (operation, timeout) = if request.argv.iter().any(|arg| arg == "forget") {
                ("prune", PRUNE_TIMEOUT_SECONDS)
            } else if request.argv.iter().any(|arg| arg == "check") {
                ("verify", VERIFY_TIMEOUT_SECONDS)
            } else if request.argv.iter().any(|arg| arg == "--tag") {
                ("archive", ARCHIVE_BACKUP_TIMEOUT_SECONDS)
            } else {
                ("backup", INITIAL_BACKUP_TIMEOUT_SECONDS)
            };
            assert_eq!(request.timeout, Some(Duration::from_secs(timeout)));
            operations.push(operation);
        }
        assert_eq!(operations, ["backup", "prune", "verify", "archive"]);
        assert!(runner.outputs.borrow().is_empty());
    }

    #[test]
    fn excludes_are_exact_and_keep_durable_health() {
        assert!(!solstone_core_journal_io::copy_excludes::copy_excluded(
            "health/retention.log"
        ));
        assert!(!solstone_core_journal_io::copy_excludes::copy_excluded(
            "config/journal.json"
        ));
    }
    #[test]
    fn backup_excludes_endpoint_key_material_by_its_resolved_path() {
        let journal = Path::new("/journal");
        let args = backup_args(journal);
        let root = crate::restic_filesystem_path(journal);
        let root = if cfg!(windows) {
            root.replace('\\', "/")
        } else {
            root
        };
        let endpoint = format!("{root}/mcp-endpoint");
        let chatgpt = format!("{root}/config/chatgpt-sign-in.json*");
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--exclude" && pair[1] == endpoint),
            "{args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--exclude" && pair[1] == chatgpt),
            "{args:?}"
        );
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
        let capability = prepare(journal.path(), &clock).expect("backup admits");
        assert!(matches!(&capability.mode, AdmittedBackupMode::Byo { .. }));
        assert_eq!(
            capability
                .execute(&services(&runner, &http, &clock, &maintenance))
                .status,
            "ok"
        );
        let commands = runner.commands.borrow();
        assert_eq!(commands[0][0], "unlock");
        assert_eq!(commands[1][0], "backup");
        assert!(
            commands[1]
                .windows(2)
                .any(|pair| pair[0] == "--exclude" && pair[1].ends_with("/**/.removing_*"))
        );
        assert!(
            commands[1]
                .windows(2)
                .any(|pair| pair[0] == "--exclude" && pair[1].ends_with("/health/sync"))
        );
        assert!(
            !commands[1]
                .windows(2)
                .any(|pair| pair == ["--exclude", "health"])
        );
        assert_one_resolution_attempt();
    }
    #[test]
    fn a_repository_without_its_backend_prefix_is_refused_before_restic_runs() {
        // restic would read the bare name as a folder in the journal, and every
        // run would then copy that folder into itself.
        let journal = configured_journal();
        let mut destination = solstone_core_backup::get_destination(journal.path())
            .unwrap()
            .unwrap();
        destination.repository = "solstone-backup".into();
        solstone_core_backup::set_destination(journal.path(), &destination).unwrap();
        reset_backup_path_resolution_attempts();
        let runner = Script {
            outputs: RefCell::new(VecDeque::new()),
            commands: RefCell::new(vec![]),
        };
        let http = Http;
        let clock = FixedClock;
        let maintenance = Maintenance;
        let result = run_backup(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
        );
        assert_eq!(result.status, "error");
        assert_eq!(
            result.error_reason.as_deref(),
            Some("repository_unsupported")
        );
        assert!(runner.commands.borrow().is_empty());
        let recorded = solstone_core_backup::get_backup_config(journal.path()).unwrap();
        assert_eq!(
            recorded["last_backup"]["error_reason"],
            "repository_unsupported"
        );
        let archive = run_archive_backup(
            journal.path(),
            &services(&runner, &http, &clock, &maintenance),
            &[journal.path().join("chronicle")],
        );
        assert_eq!(
            archive.error_reason.as_deref(),
            Some("repository_unsupported")
        );
        assert!(runner.commands.borrow().is_empty());
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
        assert_eq!(
            without_exclude_file(&commands[1]),
            json_backup_args(&resolved)
        );
        assert_eq!(commands[1][1], crate::restic_filesystem_path(&resolved));
        assert_eq!(
            commands[1]
                .windows(2)
                .find(|pair| pair[0] == "--exclude" && pair[1].ends_with("/mcp-endpoint")),
            Some(
                ["--exclude".to_owned(), {
                    let root = crate::restic_filesystem_path(&resolved);
                    let root = if cfg!(windows) {
                        root.replace('\\', "/")
                    } else {
                        root
                    };
                    format!("{root}/mcp-endpoint")
                }]
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
        assert_eq!(
            without_exclude_file(&runner.commands.borrow()[1]),
            json_backup_args(&original)
        );
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
        assert_eq!(
            without_exclude_file(&runner.commands.borrow()[1]),
            json_backup_args(&resolved)
        );
        assert_one_resolution_attempt();
    }

    #[cfg(unix)]
    #[test]
    fn enabled_journal_path_resolution_failure_does_not_invoke_the_runner() {
        use std::os::unix::fs::symlink;

        let sandbox = tempfile::tempdir().expect("test sandbox creates");
        let link = sandbox.path().join("journal-link");
        symlink("missing-journal", &link).expect("dangling journal link creates");
        let clock = FixedClock;
        reset_backup_path_resolution_attempts();

        let result = prepare_error(prepare(&link, &clock));

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
        let clock = FixedClock;
        reset_backup_path_resolution_attempts();

        let result = run_with_backup_journal_resolved_hook(
            || {},
            || prepare_error(prepare(journal.path(), &clock)),
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
        let mut missing_recovery = valid_backup_section(valid_destination());
        missing_recovery
            .as_object_mut()
            .expect("backup object")
            .remove("recovery_key");
        let mut missing_binding = valid_backup_section(valid_destination());
        missing_binding["mode"] = Value::String("operated".into());
        let mut missing_destination = valid_backup_section(valid_destination());
        missing_destination
            .as_object_mut()
            .expect("backup object")
            .remove("destination");
        let mut missing_repository = valid_backup_section(valid_destination());
        missing_repository["destination"]
            .as_object_mut()
            .expect("destination object")
            .remove("repository");
        let mut missing_backend = valid_backup_section(valid_destination());
        missing_backend["destination"]
            .as_object_mut()
            .expect("destination object")
            .remove("backend");
        let cases = vec![
            ("disabled", serde_json::json!({"enabled": false})),
            ("missing_daily_key", missing_daily),
            ("missing_recovery_key", missing_recovery),
            ("missing_operated_binding", missing_binding),
            ("missing_destination", missing_destination),
            ("missing_repository", missing_repository),
            ("missing_backend", missing_backend),
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
            let clock = FixedClock;
            reset_backup_path_resolution_attempts();

            let result = prepare_error(prepare(journal.path(), &clock));

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

        let capability = prepare(journal.path(), &clock).expect("BYO admits");
        assert!(matches!(&capability.mode, AdmittedBackupMode::Byo { .. }));
        let result = capability.execute(&services(&runner, &http, &clock, &maintenance));

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
    fn empty_string_byo_credentials_are_admitted_then_reach_the_runner() {
        let journal = tempfile::tempdir().expect("test journal creates");
        write_backup_section(
            journal.path(),
            valid_backup_section(serde_json::json!({
                "repository": "s3:repo",
                "backend": "s3",
                "credentials": {"access_key_id": "", "secret_access_key": ""},
            })),
        );
        let runner = Script {
            outputs: RefCell::new(VecDeque::from([output(11, ""), output(1, "")])),
            commands: RefCell::new(vec![]),
        };
        let http = PanicHttp;
        let clock = FixedClock;
        let maintenance = Maintenance;
        reset_backup_path_resolution_attempts();

        let capability = prepare(journal.path(), &clock).expect("blank credentials admit");
        assert!(matches!(&capability.mode, AdmittedBackupMode::Byo { .. }));
        let result = capability.execute(&services(&runner, &http, &clock, &maintenance));

        assert_eq!(result.status, "error");
        assert_eq!(result.error_reason.as_deref(), Some("failed"));
        assert_eq!(runner.commands.borrow().len(), 2);
        assert_eq!(runner.commands.borrow()[1][0], "backup");
        assert_one_resolution_attempt();
    }

    #[test]
    fn corrupt_or_unreadable_config_returns_broker_error_without_runner() {
        let journal = tempfile::tempdir().expect("test journal creates");
        let config = journal.path().join("config/journal.json");
        fs::create_dir_all(config.parent().expect("config parent")).expect("config parent creates");
        fs::write(&config, b"{not json").expect("corrupt config writes");
        let before_bytes = fs::read(&config).expect("config reads");
        let before_metadata = fs::metadata(&config).expect("metadata reads");
        let clock = FixedClock;
        reset_backup_path_resolution_attempts();

        let result = prepare_error(prepare(journal.path(), &clock));

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
        let result = prepare_error(prepare(unreadable.path(), &clock));
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
            let clock = FixedClock;
            reset_backup_path_resolution_attempts();

            let result = prepare_error(prepare(journal.path(), &clock));

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

        let capability = prepare(journal.path(), &clock).expect("operated backup admits");
        assert!(matches!(
            &capability.mode,
            AdmittedBackupMode::Operated { .. }
        ));
        let result = capability.execute(&services);

        assert_eq!(result.status, "ok");
        assert_eq!(http.requests.borrow().len(), 1);
        assert_eq!(runner.commands.borrow()[1][0], "-o");
        assert_one_resolution_attempt();
    }

    #[test]
    fn scheduled_run_on_a_superseded_binding_clears_local_state_instead_of_retrying() {
        let journal = tempfile::tempdir().expect("test journal creates");
        let mut backup = valid_backup_section(valid_destination());
        backup["mode"] = Value::String("operated".into());
        write_backup_section(journal.path(), backup);
        solstone_core_backup::save_hosted_binding(
            journal.path(),
            &hosted_binding("https://broker"),
        )
        .expect("binding writes");
        // The scheduled run must never touch restic once the broker has
        // refused the mint -- a superseded binding can never produce
        // credentials to back up with.
        let runner = PanicRunner;
        let http = ScriptedHttp {
            responses: RefCell::new(VecDeque::from([Ok(HttpResponse {
                status: 401,
                headers: vec![],
                body: br#"{"error":"binding_superseded"}"#.to_vec(),
            })])),
            requests: RefCell::new(vec![]),
        };
        let clock = FixedClock;
        let maintenance = Maintenance;
        let mut mint_services = services(&runner, &http, &clock, &maintenance);
        mint_services.rclone_path = Some(Path::new("/fixture/bin/rclone"));
        reset_backup_path_resolution_attempts();

        let result = prepare(journal.path(), &clock)
            .expect("operated backup admits")
            .execute(&mint_services);

        assert_eq!(result.status, "cleared_superseded");
        assert_eq!(result.error_reason.as_deref(), Some("binding_superseded"));
        assert_eq!(http.requests.borrow().len(), 1);
        assert!(
            solstone_core_backup::load_hosted_binding(journal.path()).is_none(),
            "the superseded binding must be deleted, not just reported"
        );
        let config = solstone_core_backup::get_backup_config(journal.path()).expect("config reads");
        assert_eq!(
            config["enabled"],
            Value::Bool(false),
            "local backup settings must be cleared to their defaults, matching an explicit teardown"
        );
        assert_eq!(
            config["last_backup"]["status"],
            Value::String("cleared_superseded".into())
        );

        // A second scheduled run against the now-cleared config finds
        // nothing configured and skips -- it never asks the broker again.
        let quiet_runner = PanicRunner;
        let quiet_http = ScriptedHttp {
            responses: RefCell::new(VecDeque::new()),
            requests: RefCell::new(vec![]),
        };
        let quiet_services = services(&quiet_runner, &quiet_http, &clock, &maintenance);
        let second_result = run_backup(journal.path(), &quiet_services);
        assert_eq!(second_result.status, "skipped");
        assert_eq!(quiet_http.requests.borrow().len(), 0);
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
            || {
                prepare(Path::new("journal"), &clock)
                    .expect("operated backup admits")
                    .execute(&services)
            },
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
            without_exclude_file(&requests[1].argv[backup_index..]),
            json_backup_args(&source_path)
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
            || {
                prepare(&alias, &clock)
                    .expect("operated backup admits")
                    .execute(&services)
            },
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
        assert_eq!(
            without_exclude_file(&runner.commands.borrow()[1]),
            json_backup_args(&source_path)
        );
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
    fn dropping_admitted_capability_does_not_mutate_or_reresolve() {
        let journal = configured_journal();
        let config = journal.path().join("config/journal.json");
        let before = fs::read(&config).expect("config reads");
        let clock = FixedClock;
        reset_backup_path_resolution_attempts();

        let capability = prepare(journal.path(), &clock).expect("backup admits");
        assert_one_resolution_attempt();
        drop(capability);

        assert_eq!(fs::read(&config).expect("config rereads"), before);
        assert_one_resolution_attempt();
    }

    #[cfg(unix)]
    #[test]
    fn capability_tool_error_records_at_original_alias_without_reresolution() {
        use std::os::unix::fs::symlink;

        let _lock = CURRENT_DIRECTORY
            .lock()
            .expect("working directory lock holds");
        let source = configured_journal();
        let replacement = tempfile::tempdir().expect("replacement journal creates");
        write_backup_section(replacement.path(), serde_json::json!({"enabled": false}));
        let sandbox = tempfile::tempdir().expect("test sandbox creates");
        let alias = sandbox.path().join("journal");
        symlink(source.path(), &alias).expect("alias creates");
        let replacement_path = replacement.path().to_path_buf();
        let hook_alias = alias.clone();
        let clock = FixedClock;
        let _directory = CurrentDirectoryGuard::change_to(sandbox.path());
        reset_backup_path_resolution_attempts();

        let capability = run_with_backup_journal_resolved_hook(
            move || {
                fs::remove_file(&hook_alias).expect("source alias removes");
                symlink(&replacement_path, &hook_alias).expect("replacement alias creates");
            },
            || prepare(Path::new("journal"), &clock).expect("backup admits"),
        );
        std::env::set_current_dir("/").expect("working directory changes after admission");
        let replacement_before = fs::read(replacement.path().join("config/journal.json"))
            .expect("replacement config reads");
        let result = capability.record_tool_error(
            &clock,
            ClosedToolError::ResticUnavailable {
                detail: "missing".into(),
                guidance: solstone_core_installed_payload::guidance::MANIFEST_MISSING.into(),
            },
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.error_reason.as_deref(), Some("restic_unavailable"));
        assert_eq!(result.detail.as_deref(), Some("missing"));
        assert_eq!(
            solstone_core_backup::get_backup_config(source.path()).expect("source state reads")["last_backup"]
                ["error_reason"],
            "restic_unavailable"
        );
        assert_eq!(
            solstone_core_backup::get_backup_config(source.path()).expect("source state reads")["last_backup"]
                ["detail"],
            "missing"
        );
        assert_eq!(
            fs::read(replacement.path().join("config/journal.json"))
                .expect("replacement config rereads"),
            replacement_before
        );
        assert_one_resolution_attempt();
    }

    #[test]
    fn record_backup_without_failure_hook_records_normally() {
        let journal = configured_journal();
        let clock = FixedClock;
        let result = BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some("restic_unavailable".into()),
            unreadable: None,
            detail: None,
        };

        record_backup(journal.path(), &clock, &result, None);

        let config =
            solstone_core_backup::get_backup_config(journal.path()).expect("backup config reads");
        assert_eq!(config["last_backup"]["status"], "error");
        assert_eq!(config["last_backup"]["error_reason"], "restic_unavailable");
    }

    #[test]
    fn backup_record_failure_hook_rejects_wrong_target_and_normal_recording_recovers() {
        let expected = configured_journal();
        let actual = configured_journal();
        let clock = FixedClock;
        let result = BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some("restic_unavailable".into()),
            unreadable: None,
            detail: None,
        };
        arm_backup_record_failure_hook(expected.path().to_path_buf());

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            record_backup(actual.path(), &clock, &result, None);
        }))
        .expect_err("wrong record target rejects");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("panic message is text");
        assert!(message.contains("backup record failure hook target mismatch"));
        assert!(message.contains(&expected.path().display().to_string()));
        assert!(message.contains(&actual.path().display().to_string()));

        clear_backup_record_failure_hook();
        record_backup(actual.path(), &clock, &result, None);
        let config =
            solstone_core_backup::get_backup_config(actual.path()).expect("backup config reads");
        assert_eq!(config["last_backup"]["status"], "error");
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn backup_record_failure_hook_is_one_shot_and_resettable() {
        let journal = configured_journal();
        let clock = FixedClock;
        let result = BackupResult {
            status: "error".into(),
            snapshot_id: None,
            error_reason: Some("restic_unavailable".into()),
            unreadable: None,
            detail: None,
        };
        reset_backup_record_failure_hook();
        install_backup_record_failure_hook(journal.path().to_path_buf());

        record_backup(journal.path(), &clock, &result, None);

        assert!(!backup_record_failure_hook_armed());
        assert_eq!(
            backup_record_failure_hook_consumed_target().as_deref(),
            Some(journal.path())
        );
        record_backup(journal.path(), &clock, &result, None);
        let config =
            solstone_core_backup::get_backup_config(journal.path()).expect("backup config reads");
        assert_eq!(config["last_backup"]["status"], "error");

        install_backup_record_failure_hook(journal.path().to_path_buf());
        assert!(backup_record_failure_hook_armed());
        reset_backup_record_failure_hook();
        assert!(!backup_record_failure_hook_armed());
        assert_eq!(backup_record_failure_hook_consumed_target(), None);
    }

    #[cfg(unix)]
    #[test]
    fn record_tool_error_preserves_result_when_recording_fails() {
        use std::os::unix::fs::symlink;

        let journal = configured_journal();
        let config = journal.path().join("config/journal.json");
        let before = fs::read(&config).expect("config reads");
        let sentinel = journal.path().join("sentinel");
        fs::write(&sentinel, b"sentinel").expect("sentinel writes");
        let clock = FixedClock;
        reset_backup_path_resolution_attempts();
        let capability = prepare(journal.path(), &clock).expect("backup admits");
        let lock = journal.path().join("config/journal.json.lock");
        fs::remove_file(&lock).expect("admission lock releases");
        symlink(&sentinel, &lock).expect("lock sentinel links");

        let result = capability.record_tool_error(
            &clock,
            ClosedToolError::ResticUnavailable {
                detail: "missing".into(),
                guidance: solstone_core_installed_payload::guidance::MANIFEST_MISSING.into(),
            },
        );

        assert_eq!(result.status, "error");
        assert_eq!(result.snapshot_id, None);
        assert_eq!(result.error_reason.as_deref(), Some("restic_unavailable"));
        assert_eq!(fs::read(&config).expect("config rereads"), before);
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

            let capability = prepare(journal.path(), &clock).expect("BYO admits");
            assert!(matches!(&capability.mode, AdmittedBackupMode::Byo { .. }));
            let result = capability.execute(&services(&runner, &http, &clock, &maintenance));

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
        services.restic = None;

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

#[cfg(test)]
#[path = "engine_credential_exclusion_tests.rs"]
mod credential_exclusion_tests;

#[cfg(all(test, feature = "full-tests"))]
#[path = "engine_copy_tests.rs"]
mod copy_tests;
