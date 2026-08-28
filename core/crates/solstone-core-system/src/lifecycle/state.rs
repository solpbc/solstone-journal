// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Lifecycle-owned health state.

use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use solstone_core_journal_io::{
    BoundAtomicOutcome, BoundParentLock, ClaimName, ExistingParentLockError, FlatDirectory,
    JournalRoot, acquire_existing_parent_lock_bound, atomic_replace_bound,
    claim_and_remove_observed, create_or_open_flat_directory_bound,
};
use solstone_core_journal_io::{
    ClaimRemovalError, ClaimRemovalOutcome, DetailedAtomicError, FileObservation,
    FlatDirectoryError,
};
use thiserror::Error;

use super::LifecycleError;
#[cfg(unix)]
use super::readiness::ReadinessMarker;

#[cfg(unix)]
static SELF_HEARTBEAT_CLAIM_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn health(journal: &Path) -> PathBuf {
    journal.join("health")
}

/// The retained health capability and singleton lock acquired beneath it.
#[cfg(unix)]
pub(crate) struct SupervisorLock {
    pub(crate) health: FlatDirectory,
    pub(crate) lease: BoundParentLock,
}

#[derive(Debug, Error)]
#[cfg(unix)]
pub(crate) enum SupervisorLockError {
    #[error("supervisor already running")]
    AlreadyRunning,
    #[error("could not bind health directory: {0}")]
    BindHealth(FlatDirectoryError),
    #[error("could not acquire supervisor lock: {0}")]
    Acquire(ExistingParentLockError),
}

/// Bind/create `health` and retain the persistent singleton lock beneath it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn open_supervisor_lock(
    root: &JournalRoot,
) -> Result<SupervisorLock, SupervisorLockError> {
    let health = create_or_open_flat_directory_bound(
        root,
        OsStr::new("health"),
        0o700,
        root.canonical_path(),
    )
    .map_err(SupervisorLockError::BindHealth)?;
    let lease = match acquire_existing_parent_lock_bound(
        &health,
        OsStr::new("supervisor.lock"),
        Duration::ZERO,
        Duration::ZERO,
    ) {
        Ok(lease) => lease,
        Err(ExistingParentLockError::Timeout(_)) => {
            return Err(SupervisorLockError::AlreadyRunning);
        }
        Err(error) => return Err(SupervisorLockError::Acquire(error)),
    };
    Ok(SupervisorLock { health, lease })
}

#[derive(Debug, Error)]
pub enum HeartbeatWriteError {
    #[error("invalid heartbeat filename")]
    InvalidFilename,
    #[error("could not publish heartbeat: {source}")]
    Publish {
        #[source]
        source: DetailedAtomicError,
    },
    #[error("heartbeat publication durability is uncertain: {source}")]
    DurabilityUncertain {
        #[source]
        source: std::io::Error,
    },
    #[error("published heartbeat disappeared before it could be retained")]
    ObservationMissing,
    #[error("could not retain published heartbeat observation: {source}")]
    Observation {
        #[source]
        source: FlatDirectoryError,
    },
    #[error("published heartbeat bytes changed before retention")]
    ObservationBytesMismatched,
    #[error("published heartbeat identity could not be retained: {source}")]
    PublicationObservationUncertain {
        #[source]
        source: std::io::Error,
    },
}

/// Failure to publish and retain a lifecycle-owned health artifact.
#[derive(Debug, Error)]
pub enum LifecycleArtifactWriteError {
    #[error("could not publish lifecycle artifact {name}: {source}")]
    Publish {
        name: &'static str,
        #[source]
        source: DetailedAtomicError,
    },
    #[error("lifecycle artifact {name} publication durability is uncertain: {source}")]
    DurabilityUncertain {
        name: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("published lifecycle artifact {name} disappeared before retention")]
    ObservationMissing { name: &'static str },
    #[error("could not retain published lifecycle artifact {name}: {source}")]
    Observation {
        name: &'static str,
        #[source]
        source: FlatDirectoryError,
    },
    #[error("published lifecycle artifact {name} changed before retention")]
    ObservationBytesMismatched { name: &'static str },
    #[error("published lifecycle artifact {name} identity could not be retained: {source}")]
    PublicationObservationUncertain {
        name: &'static str,
        #[source]
        source: std::io::Error,
    },
}

/// Exact observations and values for the current run's shared identity files.
#[derive(Debug)]
pub(crate) struct SupervisorIdentityArtifacts {
    pub(crate) pid: u32,
    pub(crate) start_time: f64,
    pub(crate) pid_observation: FileObservation,
    pub(crate) start_time_observation: FileObservation,
}

/// Outcome of claim-only self-heartbeat cleanup.
#[derive(Debug)]
pub enum SelfHeartbeatRemoval {
    NoCleanupAuthority,
    Removed,
    NotClean { outcome: ClaimRemovalOutcome },
    NotCleanError { source: ClaimRemovalError },
    VerificationFailed { reason: String },
}

/// Failure to remove a retained admission-wait marker through the only safe
/// identity-bound claim primitive.
#[derive(Debug, Error)]
pub enum AdmissionWaitMarkerCleanupError {
    #[error("admission-wait marker has no retained observation")]
    MissingObservation,
    #[error("admission-wait marker was not cleanly removed: {outcome:?}")]
    Outcome { outcome: ClaimRemovalOutcome },
    #[error("could not claim and remove admission-wait marker: {source}")]
    Claim {
        #[source]
        source: ClaimRemovalError,
    },
}

impl fmt::Display for SelfHeartbeatRemoval {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCleanupAuthority => formatter.write_str("no self-heartbeat cleanup authority"),
            Self::Removed => formatter.write_str("self heartbeat removed"),
            Self::NotClean { outcome } => write!(
                formatter,
                "self heartbeat was not cleanly removed: {outcome:?}"
            ),
            Self::NotCleanError { source } => {
                write!(formatter, "self heartbeat cleanup failed: {source}")
            }
            Self::VerificationFailed { reason } => {
                write!(
                    formatter,
                    "self heartbeat cleanup could not be verified: {reason}"
                )
            }
        }
    }
}

#[cfg(unix)]
pub(crate) fn write_readiness(
    health: &FlatDirectory,
    identity: &SupervisorIdentityArtifacts,
    ready_at: f64,
    mut extra: serde_json::Map<String, serde_json::Value>,
) -> Result<FileObservation, LifecycleError> {
    extra.remove("pid");
    extra.remove("ready_at");
    extra.remove("start_time");
    let marker = ReadinessMarker {
        pid: identity.pid,
        ready_at,
        start_time: identity.start_time,
        extra,
    };
    write_lifecycle_artifact(health, "supervisor.ready", &serde_json::to_vec(&marker)?)
}

/// Service-management cleanup used only after the external manager has
/// established that no supervisor run owns readiness.
pub fn clear_ready(journal: &Path) -> Result<(), LifecycleError> {
    match fs::remove_file(health(journal).join("supervisor.ready")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Atomically publish a heartbeat through the retained sync descriptor, then
/// retain only an exact stable observation of the bytes that were published.
#[cfg(unix)]
pub fn write_sync_heartbeat(
    sync: &FlatDirectory,
    filename: &str,
    body: &[u8],
) -> Result<FileObservation, HeartbeatWriteError> {
    let name = heartbeat_name(filename)?;
    match atomic_replace_bound(sync, name, body, 0o600) {
        Ok(BoundAtomicOutcome::Published { observation }) => Ok(observation),
        Ok(BoundAtomicOutcome::PublishedDurabilityUncertain {
            observation,
            source,
        }) => {
            let cleanup = cleanup_uncertain_publication(sync, name, &observation);
            Err(HeartbeatWriteError::DurabilityUncertain {
                source: durability_uncertain_source(source, cleanup),
            })
        }
        Ok(BoundAtomicOutcome::PublishedObservationUncertain {
            observation,
            source,
            durability_source,
        }) => {
            let cleanup = cleanup_uncertain_publication(sync, name, &observation);
            Err(HeartbeatWriteError::PublicationObservationUncertain {
                source: publication_observation_uncertain_source(
                    source,
                    durability_source,
                    cleanup,
                ),
            })
        }
        Err(source) => Err(HeartbeatWriteError::Publish { source }),
    }
}

/// Remove only the exact observation retained after a prior successful publish.
#[cfg(unix)]
pub fn clear_self_heartbeat(
    sync: &FlatDirectory,
    self_filename: &str,
    retained: Option<&FileObservation>,
) -> Result<SelfHeartbeatRemoval, LifecycleError> {
    validate_heartbeat_filename(self_filename)?;
    let Some(prior) = retained else {
        return Ok(SelfHeartbeatRemoval::NoCleanupAuthority);
    };
    let claim = next_claim_name()?;
    match claim_and_remove_observed(sync, OsStr::new(self_filename), prior, &claim) {
        Ok(ClaimRemovalOutcome::Removed) => Ok(SelfHeartbeatRemoval::Removed),
        Ok(outcome) => Ok(SelfHeartbeatRemoval::NotClean { outcome }),
        Err(source) => Ok(SelfHeartbeatRemoval::NotCleanError { source }),
    }
}

#[cfg(unix)]
pub(crate) fn clear_lifecycle_artifact(
    directory: &FlatDirectory,
    name: &'static str,
    retained: Option<&FileObservation>,
) -> Result<SelfHeartbeatRemoval, LifecycleError> {
    let Some(prior) = retained else {
        return Ok(SelfHeartbeatRemoval::NoCleanupAuthority);
    };
    let claim = next_claim_name()?;
    match claim_and_remove_observed(directory, OsStr::new(name), prior, &claim) {
        Ok(ClaimRemovalOutcome::Removed) => Ok(SelfHeartbeatRemoval::Removed),
        Ok(outcome) => Ok(SelfHeartbeatRemoval::NotClean { outcome }),
        Err(source) => Ok(SelfHeartbeatRemoval::NotCleanError { source }),
    }
}

#[cfg(unix)]
pub(crate) fn require_lifecycle_artifact_removed(
    name: &'static str,
    removal: Result<SelfHeartbeatRemoval, LifecycleError>,
) -> Result<(), LifecycleError> {
    match removal? {
        SelfHeartbeatRemoval::Removed => Ok(()),
        outcome => Err(LifecycleError::LifecycleArtifactCleanup { name, outcome }),
    }
}

#[cfg(unix)]
pub(crate) fn clear_supervisor_identity(
    health: &FlatDirectory,
    identity: &SupervisorIdentityArtifacts,
) -> Result<(), LifecycleError> {
    let pid = require_lifecycle_artifact_removed(
        "supervisor.pid",
        clear_lifecycle_artifact(health, "supervisor.pid", Some(&identity.pid_observation)),
    );
    let start_time = require_lifecycle_artifact_removed(
        "supervisor.start_time",
        clear_lifecycle_artifact(
            health,
            "supervisor.start_time",
            Some(&identity.start_time_observation),
        ),
    );
    pid?;
    start_time
}

#[cfg(unix)]
fn write_lifecycle_artifact(
    directory: &FlatDirectory,
    name: &'static str,
    body: &[u8],
) -> Result<FileObservation, LifecycleError> {
    match atomic_replace_bound(directory, OsStr::new(name), body, 0o600) {
        Ok(BoundAtomicOutcome::Published { observation }) => Ok(observation),
        Ok(BoundAtomicOutcome::PublishedDurabilityUncertain {
            observation,
            source,
        }) => {
            let cleanup = cleanup_uncertain_publication(directory, OsStr::new(name), &observation);
            Err(LifecycleArtifactWriteError::DurabilityUncertain {
                name,
                source: durability_uncertain_source(source, cleanup),
            }
            .into())
        }
        Ok(BoundAtomicOutcome::PublishedObservationUncertain {
            observation,
            source,
            durability_source,
        }) => {
            let cleanup = cleanup_uncertain_publication(directory, OsStr::new(name), &observation);
            Err(
                LifecycleArtifactWriteError::PublicationObservationUncertain {
                    name,
                    source: publication_observation_uncertain_source(
                        source,
                        durability_source,
                        cleanup,
                    ),
                }
                .into(),
            )
        }
        Err(source) => Err(LifecycleArtifactWriteError::Publish { name, source }.into()),
    }
}

/// A rename has already landed when parent-directory sync fails. Recover an
/// exact observation of those bytes and remove only that observation before
/// returning the durability error, so a refused boot does not strand its own
/// lifecycle artifact.
#[cfg(unix)]
fn cleanup_uncertain_publication(
    directory: &FlatDirectory,
    name: &OsStr,
    observation: &FileObservation,
) -> Result<(), String> {
    let claim = next_claim_name().map_err(|source| source.to_string())?;
    match claim_and_remove_observed(directory, name, observation, &claim) {
        Ok(ClaimRemovalOutcome::Removed) => Ok(()),
        Ok(outcome) => Err(format!("exact cleanup was not durable: {outcome:?}")),
        Err(source) => Err(format!("exact cleanup failed: {source}")),
    }
}

#[cfg(unix)]
fn durability_uncertain_source(
    source: std::io::Error,
    cleanup: Result<(), String>,
) -> std::io::Error {
    match cleanup {
        Ok(()) => source,
        Err(cleanup) => std::io::Error::new(
            source.kind(),
            format!("{source}; exact post-publication cleanup failed: {cleanup}"),
        ),
    }
}

#[cfg(unix)]
fn publication_observation_uncertain_source(
    source: std::io::Error,
    durability_source: Option<std::io::Error>,
    cleanup: Result<(), String>,
) -> std::io::Error {
    let mut detail = source.to_string();
    if let Some(durability_source) = durability_source {
        detail.push_str(&format!("; parent sync failed: {durability_source}"));
    }
    if let Err(cleanup) = cleanup {
        detail.push_str(&format!(
            "; exact post-publication cleanup failed: {cleanup}"
        ));
    }
    std::io::Error::new(source.kind(), detail)
}

/// Claim-remove a wait marker only when the exact published observation was
/// retained. Any outcome other than a durably removed marker is a failure.
#[cfg(unix)]
pub(crate) fn clear_admission_wait_marker(
    sync: &FlatDirectory,
    filename: &str,
    retained: Option<&FileObservation>,
) -> Result<(), LifecycleError> {
    validate_heartbeat_filename(filename)?;
    let prior = retained.ok_or({
        LifecycleError::AdmissionWaitMarkerCleanup(
            AdmissionWaitMarkerCleanupError::MissingObservation,
        )
    })?;
    let claim = next_claim_name()?;
    admission_wait_marker_cleanup_result(claim_and_remove_observed(
        sync,
        OsStr::new(filename),
        prior,
        &claim,
    ))
}

#[cfg(unix)]
fn admission_wait_marker_cleanup_result(
    result: Result<ClaimRemovalOutcome, ClaimRemovalError>,
) -> Result<(), LifecycleError> {
    match result {
        Ok(ClaimRemovalOutcome::Removed) => Ok(()),
        Ok(outcome) => Err(LifecycleError::AdmissionWaitMarkerCleanup(
            AdmissionWaitMarkerCleanupError::Outcome { outcome },
        )),
        Err(source) => Err(LifecycleError::AdmissionWaitMarkerCleanup(
            AdmissionWaitMarkerCleanupError::Claim { source },
        )),
    }
}

#[cfg(unix)]
fn heartbeat_name(filename: &str) -> Result<&OsStr, HeartbeatWriteError> {
    validate_heartbeat_filename(filename).map_err(|_| HeartbeatWriteError::InvalidFilename)?;
    Ok(OsStr::new(filename))
}

pub(crate) fn validate_heartbeat_filename(filename: &str) -> Result<(), LifecycleError> {
    let candidate = Path::new(filename);
    if filename.is_empty()
        || filename.starts_with('.')
        || !filename.ends_with(".check")
        || filename.contains(['/', '\\'])
        || candidate.file_name() != Some(OsStr::new(filename))
    {
        return Err(LifecycleError::InvalidHeartbeatFilename);
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn next_claim_name() -> Result<ClaimName, LifecycleError> {
    let sequence = SELF_HEARTBEAT_CLAIM_SEQUENCE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map_err(|_| LifecycleError::Identity("self heartbeat claim sequence exhausted"))?;
    let candidate = format!("!solstone-claim-{:08x}-{sequence:016x}", std::process::id());
    ClaimName::parse(&candidate).map_err(|_| LifecycleError::Identity("self heartbeat claim name"))
}

pub fn compact_log_if_oversized(log_path: &Path, max_bytes: u64) -> Result<(), LifecycleError> {
    let size = match log_path.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if size <= max_bytes {
        return Ok(());
    }
    let compact = log_path.with_file_name(format!(
        "{}.compact",
        log_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("supervisor.log")
    ));
    let result = (|| -> Result<(), LifecycleError> {
        let mut source = File::open(log_path)?;
        source.seek(SeekFrom::End(-(max_bytes as i64)))?;
        let mut tail = Vec::with_capacity(max_bytes as usize);
        source.read_to_end(&mut tail)?;
        let kept = tail
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| &tail[index + 1..])
            .unwrap_or_default();
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&compact)?;
        target.write_all(kept)?;
        target.flush()?;
        fs::rename(&compact, log_path)?;
        Ok(())
    })();
    if let Err(error) = &result {
        eprintln!("supervisor log compaction failed: {error}");
        let _ = fs::remove_file(&compact);
    }
    result
}

pub fn append_supervisor_log(
    log_path: &Path,
    message: &[u8],
    max_bytes: u64,
    backup_count: usize,
) -> Result<(), LifecycleError> {
    if log_path
        .metadata()
        .is_ok_and(|metadata| metadata.len() >= max_bytes)
    {
        for index in (1..=backup_count).rev() {
            let source = if index == 1 {
                log_path.to_path_buf()
            } else {
                log_path.with_extension((index - 1).to_string())
            };
            let target = log_path.with_extension(index.to_string());
            if source.exists() {
                let _ = fs::rename(source, target);
            }
        }
    }
    let parent = log_path
        .parent()
        .ok_or(LifecycleError::Identity("log parent"))?;
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    file.write_all(message)?;
    file.flush()?;
    Ok(())
}

fn read_pid(path: &Path) -> Result<u32, LifecycleError> {
    fs::read_to_string(path)?
        .trim()
        .parse()
        .map_err(|_| LifecycleError::Identity("pid"))
}

pub fn recorded_supervisor_pid(journal: &Path) -> Option<u32> {
    read_pid(&health(journal).join("supervisor.pid")).ok()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn process_start_time_epoch_seconds(pid: u32) -> Result<f64, LifecycleError> {
    use crate::process::{InspectResult, ProcessInstanceSource, SystemProcessInstanceSource};

    match SystemProcessInstanceSource.inspect(pid) {
        InspectResult::Present { instance, .. } => Ok(instance.birth.epoch_seconds()),
        InspectResult::Absent | InspectResult::Unverifiable => {
            Err(LifecycleError::Identity("process start time"))
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn write_supervisor_identity(
    health: &FlatDirectory,
    pid: u32,
) -> Result<SupervisorIdentityArtifacts, LifecycleError> {
    let start_time = process_start_time_epoch_seconds(pid)?;
    let pid_observation =
        write_lifecycle_artifact(health, "supervisor.pid", pid.to_string().as_bytes())?;
    let start_time_observation = match write_lifecycle_artifact(
        health,
        "supervisor.start_time",
        start_time.to_string().as_bytes(),
    ) {
        Ok(observation) => observation,
        Err(error) => {
            require_lifecycle_artifact_removed(
                "supervisor.pid",
                clear_lifecycle_artifact(health, "supervisor.pid", Some(&pid_observation)),
            )?;
            return Err(error);
        }
    };
    Ok(SupervisorIdentityArtifacts {
        pid,
        start_time,
        pid_observation,
        start_time_observation,
    })
}

// Process start-time identity is owned by process::instance.
// iOS still has no supported process-start-time source.

#[cfg(all(test, unix))]
pub(crate) fn test_supervisor_journal(
    name: &str,
    pid: u32,
    start_time: f64,
    marker: Option<&ReadinessMarker>,
) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("solstone-{name}-{stamp}"));
    let health = health(&root);
    fs::create_dir_all(&health).expect("health");
    fs::write(health.join("supervisor.pid"), pid.to_string()).expect("pid");
    fs::write(health.join("supervisor.start_time"), start_time.to_string()).expect("start");
    if let Some(marker) = marker {
        fs::write(
            health.join("supervisor.ready"),
            serde_json::to_vec(marker).expect("marker"),
        )
        .expect("ready");
    }
    root
}

#[cfg(all(test, unix))]
pub(crate) fn remove_test_supervisor_journal(root: PathBuf) {
    fs::remove_dir_all(root).expect("cleanup");
}

#[cfg(all(test, unix))]
mod tests {
    use solstone_core_journal_io::{
        ClaimDurability, ClaimRemovalOutcome, ClaimUnchangedReason, IdentityChangeDisposition,
    };
    use tempfile::Builder;

    use super::super::sync::{self, Heartbeat, HeartbeatV2, RunId, WriterId};
    use super::*;

    fn temporary() -> tempfile::TempDir {
        Builder::new()
            .prefix("solstone-lifecycle-state-")
            .tempdir_in("/var/tmp")
            .expect("temporary journal")
    }

    fn bound_sync(root_path: &Path) -> FlatDirectory {
        let root = JournalRoot::open(root_path).expect("open journal root");
        let health = create_or_open_flat_directory_bound(
            &root,
            OsStr::new("health"),
            0o700,
            root.canonical_path(),
        )
        .expect("bind health");
        create_or_open_flat_directory_bound(
            &health,
            OsStr::new("sync"),
            0o700,
            &root.canonical_path().join("health"),
        )
        .expect("bind sync")
    }

    #[test]
    fn no_retained_observation_never_falls_back_to_path_removal() {
        let temporary = temporary();
        let sync = bound_sync(temporary.path());
        let heartbeat = temporary.path().join("health/sync/self.check");
        fs::write(&heartbeat, b"unretained").expect("fixture heartbeat");

        assert!(matches!(
            clear_self_heartbeat(&sync, "self.check", None).expect("no cleanup authority"),
            SelfHeartbeatRemoval::NoCleanupAuthority
        ));
        assert!(heartbeat.exists());
    }

    #[test]
    fn verified_observation_is_removed_only_through_a_claim() {
        let temporary = temporary();
        let sync = bound_sync(temporary.path());
        let observation =
            write_sync_heartbeat(&sync, "self.check", b"heartbeat").expect("publish and retain");

        assert!(matches!(
            clear_self_heartbeat(&sync, "self.check", Some(&observation)).expect("claim removal"),
            SelfHeartbeatRemoval::Removed
        ));
        assert!(!temporary.path().join("health/sync/self.check").exists());
    }

    #[test]
    fn verified_v2_self_cleanup_preserves_a_coexisting_v1_heartbeat() {
        let temporary = temporary();
        let sync_directory = bound_sync(temporary.path());
        let writer_id = WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID");
        let run_id = RunId::parse("fedcba9876543210fedcba9876543210").expect("run ID");
        let filename = sync::v2_heartbeat_filename(&writer_id, &run_id);
        let heartbeat = HeartbeatV2::new(
            writer_id,
            run_id,
            "self".to_owned(),
            7,
            "100".to_owned(),
            "test".to_owned(),
            15,
            "/journal".to_owned(),
        );
        let observation = write_sync_heartbeat(
            &sync_directory,
            &filename,
            &serde_json::to_vec(&heartbeat).expect("v2 heartbeat JSON"),
        )
        .expect("publish and retain v2 heartbeat");
        let foreign_v1 = Heartbeat {
            schema: sync::HEARTBEAT_SCHEMA_V1,
            machine_id: "legacy-machine".to_owned(),
            hostname: "foreign".to_owned(),
            pid: 8,
            wall_time: "100".to_owned(),
            solstone_version: "test".to_owned(),
            interval_seconds: 15,
            journal_path: "/journal".to_owned(),
        };
        let foreign_v1_bytes = serde_json::to_vec(&foreign_v1).expect("v1 heartbeat JSON");
        let foreign_v1_path = temporary.path().join("health/sync/foreign-v1.check");
        fs::write(&foreign_v1_path, &foreign_v1_bytes).expect("foreign v1 heartbeat");

        assert!(matches!(
            clear_self_heartbeat(&sync_directory, &filename, Some(&observation))
                .expect("claim removal"),
            SelfHeartbeatRemoval::Removed
        ));
        assert!(
            !temporary
                .path()
                .join("health/sync")
                .join(&filename)
                .exists()
        );
        assert_eq!(
            fs::read(foreign_v1_path).expect("foreign v1 remains"),
            foreign_v1_bytes
        );
    }

    #[test]
    fn admission_wait_marker_is_retained_then_claim_removed() {
        let temporary = temporary();
        let sync = bound_sync(temporary.path());
        let filename = "solstone-wait-v2-0123456789abcdef0123456789abcdef-fedcba9876543210fedcba9876543210.check";
        let observation =
            write_sync_heartbeat(&sync, filename, b"wait marker").expect("publish and retain");

        clear_admission_wait_marker(&sync, filename, Some(&observation)).expect("claim removal");
        assert!(!temporary.path().join("health/sync").join(filename).exists());
    }

    #[test]
    fn every_non_removed_marker_claim_outcome_is_a_visible_failure() {
        let outcomes = [
            ClaimRemovalOutcome::RemovedDurabilityUncertain,
            ClaimRemovalOutcome::Unchanged {
                reason: ClaimUnchangedReason::ClaimNameOccupied,
            },
            ClaimRemovalOutcome::IdentityChanged {
                disposition: IdentityChangeDisposition::UnknownLocation,
                durability: ClaimDurability::NotEstablished,
            },
        ];
        for outcome in outcomes {
            assert!(matches!(
                admission_wait_marker_cleanup_result(Ok(outcome)),
                Err(LifecycleError::AdmissionWaitMarkerCleanup(
                    AdmissionWaitMarkerCleanupError::Outcome { .. }
                ))
            ));
        }
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn durability_uncertainty_removes_every_published_lifecycle_artifact() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_fault,
        };

        let temporary = temporary();
        let sync = bound_sync(temporary.path());
        let (result, fault_consumed) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::ParentSync,
            1,
            nix::errno::Errno::EIO as i32,
            || write_sync_heartbeat(&sync, "self.check", b"heartbeat"),
        );

        assert!(fault_consumed);
        assert!(matches!(
            result,
            Err(HeartbeatWriteError::DurabilityUncertain { .. })
        ));
        assert!(!temporary.path().join("health/sync/self.check").exists());

        let root = JournalRoot::open(temporary.path()).expect("journal root");
        let health = create_or_open_flat_directory_bound(
            &root,
            OsStr::new("health"),
            0o700,
            temporary.path(),
        )
        .expect("bound health");
        for name in [
            "supervisor.pid",
            "supervisor.start_time",
            "supervisor.ready",
        ] {
            let (result, fault_consumed) = run_with_bound_publication_fault(
                BoundPublicationPrimitive::ParentSync,
                1,
                nix::errno::Errno::EIO as i32,
                || write_lifecycle_artifact(&health, name, b"owned artifact"),
            );
            assert!(fault_consumed, "parent-sync fault for {name}");
            assert!(matches!(
                result,
                Err(LifecycleError::LifecycleArtifactWrite(
                    LifecycleArtifactWriteError::DurabilityUncertain { .. }
                ))
            ));
            assert!(
                !temporary.path().join("health").join(name).exists(),
                "{name} must be removed after uncertain publication"
            );
        }
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn published_observation_never_claims_an_identical_replacement() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_barrier,
        };

        let temporary = temporary();
        let sync = bound_sync(temporary.path());
        let target = temporary.path().join("health/sync/self.check");
        let replacement = temporary.path().join("health/sync/replacement.tmp");
        let callback_target = target.clone();
        let (result, barrier_fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::ParentSync,
            1,
            move || {
                fs::write(&replacement, b"identical").expect("replacement fixture");
                fs::rename(&replacement, &callback_target).expect("replace published inode");
            },
            || write_sync_heartbeat(&sync, "self.check", b"identical"),
        );

        assert!(barrier_fired);
        assert!(matches!(
            result,
            Err(HeartbeatWriteError::PublicationObservationUncertain { .. })
        ));
        assert_eq!(fs::read(target).expect("replacement remains"), b"identical");
    }
}
