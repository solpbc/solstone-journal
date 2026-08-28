// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows lifecycle orchestration and its path-based storage boundary.

#![cfg_attr(not(any(windows, test)), allow(dead_code))]

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::path::Path;

use solstone_core_journal_io::FileObservation;

use super::state::{HeartbeatWriteError, LifecycleArtifactWriteError, SelfHeartbeatRemoval};
use super::sync::{self, SyncCheckResult, SyncScanFailure, SyncSnapshot};
use super::{
    LifecycleError, StaleHeartbeatCollectionError, StaleHeartbeatGc, SyncTickOutcome,
    remove_collected_from_sync_result,
};
use super::{RunId, WriterId};

const SUPERVISOR_PID: &str = "supervisor.pid";
const SUPERVISOR_START_TIME: &str = "supervisor.start_time";
const SUPERVISOR_READY: &str = "supervisor.ready";

/// The two lifecycle directories reachable below an admitted journal root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowsLifecycleDirectory {
    Health,
    Sync,
}

/// A failed verify-then-delete attempt on a retained lifecycle entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsRemovalFailure {
    reason: String,
}

impl WindowsRemovalFailure {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for WindowsRemovalFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl Error for WindowsRemovalFailure {}

/// Storage operations used by the Windows boot and tick sequences.
///
/// The real implementation opens fresh no-follow directory capabilities for
/// each operation. Keeping this boundary narrow makes the sequencing testable
/// without Windows API calls.
pub(crate) trait WindowsLifecycleStore {
    fn scan_sync(
        &mut self,
        self_filename: &str,
        previous: Option<&SyncSnapshot>,
        now: f64,
    ) -> Result<SyncCheckResult, SyncScanFailure>;

    fn write_sync_heartbeat(
        &mut self,
        filename: &str,
        body: &[u8],
    ) -> Result<FileObservation, HeartbeatWriteError>;

    fn write_lifecycle_artifact(
        &mut self,
        name: &'static str,
        body: &[u8],
    ) -> Result<FileObservation, LifecycleArtifactWriteError>;

    fn remove_observed(
        &mut self,
        directory: WindowsLifecycleDirectory,
        name: &OsStr,
        observation: &FileObservation,
    ) -> Result<(), WindowsRemovalFailure>;
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct WindowsBootState {
    pub(crate) run_id: RunId,
    pub(crate) heartbeat_filename: String,
    pub(crate) identity: super::state::SupervisorIdentityArtifacts,
    pub(crate) retained_self_heartbeat: FileObservation,
    pub(crate) last_completed_sync_result: SyncCheckResult,
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) struct WindowsTickContext<'a> {
    pub(crate) journal: &'a Path,
    pub(crate) writer_id: &'a WriterId,
    pub(crate) run_id: RunId,
    pub(crate) heartbeat_filename: &'a str,
    pub(crate) retained_self_heartbeat: &'a mut Option<FileObservation>,
    pub(crate) last_completed_sync_result: &'a mut Option<SyncCheckResult>,
    pub(crate) stale_heartbeat_gc: &'a mut StaleHeartbeatGc,
}

/// Run the Windows admission sequence after the journal root, health
/// directory, singleton lock, and sync directory have already been admitted.
pub(crate) fn boot_with_store(
    store: &mut impl WindowsLifecycleStore,
    journal: &Path,
    writer_id: WriterId,
    run_id: RunId,
    hostname: String,
    now: f64,
    start_time: f64,
) -> Result<WindowsBootState, LifecycleError> {
    let heartbeat_filename = sync::v2_heartbeat_filename(&writer_id, &run_id);
    let first = store
        .scan_sync(&heartbeat_filename, None, now)
        .map_err(|error| LifecycleError::SyncScan(Box::new(error)))?;
    reject_live_or_unverifiable(&first)?;

    let heartbeat = sync::HeartbeatV2::new(
        writer_id.clone(),
        run_id,
        hostname,
        std::process::id(),
        now.to_string(),
        env!("CARGO_PKG_VERSION").to_owned(),
        sync::DEFAULT_INTERVAL_SECONDS as u32,
        journal.display().to_string(),
    );
    let heartbeat_body = serde_json::to_vec(&heartbeat)?;
    let retained_self_heartbeat =
        store.write_sync_heartbeat(&heartbeat_filename, &heartbeat_body)?;

    let second = match store.scan_sync(&heartbeat_filename, Some(&first.snapshot), now) {
        Ok(result) => result,
        Err(error) => {
            cleanup_after_failed_boot(store, &heartbeat_filename, &retained_self_heartbeat)?;
            return Err(LifecycleError::SyncScan(Box::new(error)));
        }
    };
    if let Err(error) = reject_live_or_unverifiable(&second) {
        cleanup_after_failed_boot(store, &heartbeat_filename, &retained_self_heartbeat)?;
        return Err(error);
    }

    let identity = match write_supervisor_identity(store, std::process::id(), start_time) {
        Ok(identity) => identity,
        Err(error) => {
            cleanup_after_failed_boot(store, &heartbeat_filename, &retained_self_heartbeat)?;
            return Err(error);
        }
    };
    Ok(WindowsBootState {
        run_id,
        heartbeat_filename,
        identity,
        retained_self_heartbeat,
        last_completed_sync_result: second,
    })
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn tick_with_store(
    store: &mut impl WindowsLifecycleStore,
    context: WindowsTickContext<'_>,
    hostname: String,
    previous: Option<&SyncSnapshot>,
    now: f64,
    monotonic_now: f64,
) -> SyncTickOutcome {
    let WindowsTickContext {
        journal,
        writer_id,
        run_id,
        heartbeat_filename,
        retained_self_heartbeat,
        last_completed_sync_result,
        stale_heartbeat_gc,
    } = context;
    let previous = previous.cloned().or_else(|| {
        last_completed_sync_result
            .as_ref()
            .map(|result| result.snapshot.clone())
    });
    let clock_discontinuous = stale_heartbeat_gc.begin_tick(now, monotonic_now);
    let heartbeat = sync::HeartbeatV2::new(
        writer_id.clone(),
        run_id,
        hostname,
        std::process::id(),
        now.to_string(),
        env!("CARGO_PKG_VERSION").to_owned(),
        sync::DEFAULT_INTERVAL_SECONDS as u32,
        journal.display().to_string(),
    );
    let body = match serde_json::to_vec(&heartbeat) {
        Ok(body) => body,
        Err(_) => unreachable!("heartbeat serializes"),
    };
    let observation = match store.write_sync_heartbeat(heartbeat_filename, &body) {
        Ok(observation) => observation,
        Err(
            error @ (HeartbeatWriteError::Publish { .. }
            | HeartbeatWriteError::DurabilityUncertain { .. }),
        ) => {
            stale_heartbeat_gc.clear_candidates();
            return SyncTickOutcome::RenewalFailure(error);
        }
        Err(
            error @ (HeartbeatWriteError::ObservationMissing
            | HeartbeatWriteError::Observation { .. }
            | HeartbeatWriteError::ObservationBytesMismatched
            | HeartbeatWriteError::PublicationObservationUncertain { .. }),
        ) => {
            stale_heartbeat_gc.clear_candidates();
            return SyncTickOutcome::RetainedObservationFailure(error);
        }
        Err(HeartbeatWriteError::InvalidFilename) => {
            stale_heartbeat_gc.clear_candidates();
            return SyncTickOutcome::RenewalFailure(HeartbeatWriteError::InvalidFilename);
        }
    };
    *retained_self_heartbeat = Some(observation);

    let mut result = match store.scan_sync(heartbeat_filename, previous.as_ref(), now) {
        Ok(result) => result,
        Err(error) => {
            stale_heartbeat_gc.clear_candidates();
            return SyncTickOutcome::CompleteScanFailure(error);
        }
    };
    if !clock_discontinuous {
        let collected = stale_heartbeat_gc.observe_completed_tick(
            &result,
            heartbeat_filename,
            now,
            |filename, prior| {
                store
                    .remove_observed(WindowsLifecycleDirectory::Sync, filename, prior)
                    .map_err(|error| StaleHeartbeatCollectionError::WindowsRemoval {
                        filename: filename.to_os_string(),
                        reason: error.to_string(),
                    })
            },
        );
        let collected = match collected {
            Ok(collected) => collected,
            Err(error) => return SyncTickOutcome::StaleHeartbeatCollectionFailure(error),
        };
        remove_collected_from_sync_result(&mut result, &collected);
    }
    let conflict = result.is_tick_conflict(previous.as_ref());
    *last_completed_sync_result = Some(result.clone());
    if conflict {
        SyncTickOutcome::Conflict(Box::new(result))
    } else {
        SyncTickOutcome::Healthy
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn write_readiness_with_store(
    store: &mut impl WindowsLifecycleStore,
    identity: &super::state::SupervisorIdentityArtifacts,
    ready_at: f64,
    mut extra: serde_json::Map<String, serde_json::Value>,
) -> Result<FileObservation, LifecycleError> {
    extra.remove("pid");
    extra.remove("ready_at");
    extra.remove("start_time");
    let marker = super::ReadinessMarker {
        pid: identity.pid,
        ready_at,
        start_time: identity.start_time,
        extra,
    };
    store
        .write_lifecycle_artifact(SUPERVISOR_READY, &serde_json::to_vec(&marker)?)
        .map_err(LifecycleError::from)
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn clear_ready_with_store(
    store: &mut impl WindowsLifecycleStore,
    observation: &FileObservation,
) -> Result<(), LifecycleError> {
    remove_lifecycle_artifact(
        store,
        WindowsLifecycleDirectory::Health,
        SUPERVISOR_READY,
        observation,
    )
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn abort_before_ready_with_store(
    store: &mut impl WindowsLifecycleStore,
    heartbeat_filename: &str,
    retained_self_heartbeat: Option<&FileObservation>,
    retained_readiness: Option<&FileObservation>,
    identity: &super::state::SupervisorIdentityArtifacts,
) -> Result<(), LifecycleError> {
    let heartbeat = remove_self_heartbeat(store, heartbeat_filename, retained_self_heartbeat);
    let readiness = retained_readiness.map_or(Ok(()), |observation| {
        clear_ready_with_store(store, observation)
    });
    let identity = clear_identity_with_store(store, identity);
    heartbeat?;
    readiness?;
    identity
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn shutdown_cleanup_with_store(
    store: &mut impl WindowsLifecycleStore,
    heartbeat_filename: &str,
    retained_self_heartbeat: Option<&FileObservation>,
    retained_readiness: Option<&FileObservation>,
    identity: &super::state::SupervisorIdentityArtifacts,
    sync_conflict: bool,
) -> (
    super::ArtifactClearOutcome,
    super::ArtifactClearOutcome,
    super::ArtifactClearOutcome,
) {
    let readiness =
        retained_readiness.map_or(super::ArtifactClearOutcome::Skipped, |observation| {
            match clear_ready_with_store(store, observation) {
                Ok(()) => super::ArtifactClearOutcome::Cleared,
                Err(error) => super::ArtifactClearOutcome::Failed(error.to_string()),
            }
        });
    if sync_conflict {
        return (
            readiness,
            super::ArtifactClearOutcome::Skipped,
            super::ArtifactClearOutcome::Skipped,
        );
    }
    let self_heartbeat =
        match remove_self_heartbeat(store, heartbeat_filename, retained_self_heartbeat) {
            Ok(()) => super::ArtifactClearOutcome::Cleared,
            Err(LifecycleError::PostPublicationHeartbeatCleanup(
                SelfHeartbeatRemoval::NoCleanupAuthority,
            )) => super::ArtifactClearOutcome::Skipped,
            Err(error) => {
                return (
                    readiness,
                    super::ArtifactClearOutcome::Failed(error.to_string()),
                    super::ArtifactClearOutcome::Skipped,
                );
            }
        };
    let identity = match clear_identity_with_store(store, identity) {
        Ok(()) => super::ArtifactClearOutcome::Cleared,
        Err(error) => super::ArtifactClearOutcome::Failed(error.to_string()),
    };
    (readiness, self_heartbeat, identity)
}

fn reject_live_or_unverifiable(result: &SyncCheckResult) -> Result<(), LifecycleError> {
    for peer in &result.live_peer_observations {
        let reason = match &peer.classification {
            sync::HeartbeatClassification::BoundedMalformed => "record is malformed or unreadable",
            sync::HeartbeatClassification::IdentityMismatch(_) => {
                "record does not match its filename identity"
            }
            _ => continue,
        };
        return Err(LifecycleError::AdmissionHeartbeatNeedsAttention {
            filename: peer.source_filename.clone(),
            reason,
        });
    }
    if result.is_boot_conflict() {
        Err(LifecycleError::AlreadyRunning)
    } else {
        Ok(())
    }
}

fn cleanup_after_failed_boot(
    store: &mut impl WindowsLifecycleStore,
    heartbeat_filename: &str,
    observation: &FileObservation,
) -> Result<(), LifecycleError> {
    remove_self_heartbeat(store, heartbeat_filename, Some(observation))
}

fn write_supervisor_identity(
    store: &mut impl WindowsLifecycleStore,
    pid: u32,
    start_time: f64,
) -> Result<super::state::SupervisorIdentityArtifacts, LifecycleError> {
    let pid_observation = store
        .write_lifecycle_artifact(SUPERVISOR_PID, pid.to_string().as_bytes())
        .map_err(LifecycleError::from)?;
    let start_time_observation = match store
        .write_lifecycle_artifact(SUPERVISOR_START_TIME, start_time.to_string().as_bytes())
    {
        Ok(observation) => observation,
        Err(error) => {
            remove_lifecycle_artifact(
                store,
                WindowsLifecycleDirectory::Health,
                SUPERVISOR_PID,
                &pid_observation,
            )?;
            return Err(error.into());
        }
    };
    Ok(super::state::SupervisorIdentityArtifacts {
        pid,
        start_time,
        pid_observation,
        start_time_observation,
    })
}

#[cfg_attr(not(windows), allow(dead_code))]
fn clear_identity_with_store(
    store: &mut impl WindowsLifecycleStore,
    identity: &super::state::SupervisorIdentityArtifacts,
) -> Result<(), LifecycleError> {
    let pid = remove_lifecycle_artifact(
        store,
        WindowsLifecycleDirectory::Health,
        SUPERVISOR_PID,
        &identity.pid_observation,
    );
    let start_time = remove_lifecycle_artifact(
        store,
        WindowsLifecycleDirectory::Health,
        SUPERVISOR_START_TIME,
        &identity.start_time_observation,
    );
    pid?;
    start_time
}

fn remove_self_heartbeat(
    store: &mut impl WindowsLifecycleStore,
    heartbeat_filename: &str,
    observation: Option<&FileObservation>,
) -> Result<(), LifecycleError> {
    let Some(observation) = observation else {
        return Err(LifecycleError::PostPublicationHeartbeatCleanup(
            SelfHeartbeatRemoval::NoCleanupAuthority,
        ));
    };
    store
        .remove_observed(
            WindowsLifecycleDirectory::Sync,
            OsStr::new(heartbeat_filename),
            observation,
        )
        .map_err(|error| {
            LifecycleError::PostPublicationHeartbeatCleanup(
                SelfHeartbeatRemoval::VerificationFailed {
                    reason: error.to_string(),
                },
            )
        })
}

fn remove_lifecycle_artifact(
    store: &mut impl WindowsLifecycleStore,
    directory: WindowsLifecycleDirectory,
    name: &'static str,
    observation: &FileObservation,
) -> Result<(), LifecycleError> {
    store
        .remove_observed(directory, OsStr::new(name), observation)
        .map_err(|error| LifecycleError::LifecycleArtifactCleanup {
            name,
            outcome: SelfHeartbeatRemoval::VerificationFailed {
                reason: error.to_string(),
            },
        })
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsStr;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use solstone_core_journal_io::{
        DetailedAtomicError, DetailedAtomicOutcome, ExistingParentLockError, FileObservation,
        FlatDirectoryError, JournalEntryKind, JournalRoot, JournalRootError, Removed,
        WindowsFlatDirectory, acquire_existing_parent_lock_bound, atomic_replace_detailed,
        create_or_open_windows_flat_directory_bound, list_windows_flat_directory,
        open_windows_flat_directory_bound, read_windows_observed_file_bounded, remove_file,
    };

    use super::{
        HeartbeatWriteError, LifecycleArtifactWriteError, SUPERVISOR_PID, SUPERVISOR_START_TIME,
        WindowsBootState, WindowsLifecycleDirectory, WindowsLifecycleStore, WindowsRemovalFailure,
        boot_with_store,
    };
    use crate::lifecycle::sweep;
    use crate::lifecycle::sync::{
        self, SyncCheckResult, SyncDirectoryOperation, SyncIncompleteSnapshotReason,
        SyncReadOperation, SyncScanFailure, SyncSnapshot, SyncUnsafeReason,
    };
    use crate::lifecycle::{LifecycleError, SupervisorLifecycle};

    const MAX_LIFECYCLE_ARTIFACT_BYTES: usize = sync::MAX_SYNC_HEARTBEAT_BYTES;

    pub(crate) fn boot(
        journal: impl AsRef<Path>,
        writer_id: super::WriterId,
    ) -> Result<SupervisorLifecycle, LifecycleError> {
        let journal = journal.as_ref().to_path_buf();
        let root = JournalRoot::open(&journal).map_err(|error| root_failure(&journal, error))?;
        let health = create_or_open_windows_flat_directory_bound(
            &root,
            OsStr::new("health"),
            root.canonical_path(),
        )
        .map_err(|reason| {
            directory_failure(
                root.canonical_path().join("health"),
                SyncDirectoryOperation::BindHealth,
                reason,
            )
        })?;
        let lease = match acquire_existing_parent_lock_bound(
            &health,
            OsStr::new("supervisor.lock"),
            Duration::ZERO,
            Duration::ZERO,
        ) {
            Ok(lease) => lease,
            Err(ExistingParentLockError::Timeout(_)) => return Err(LifecycleError::AlreadyRunning),
            Err(error) => return Err(LifecycleError::SupervisorLock(error)),
        };
        create_or_open_windows_flat_directory_bound(
            &health,
            OsStr::new("sync"),
            &root.canonical_path().join("health"),
        )
        .map_err(|reason| {
            directory_failure(
                root.canonical_path().join("health/sync"),
                SyncDirectoryOperation::BindSync,
                reason,
            )
        })?;

        let run_id = super::RunId::generate()?;
        let now = wall_seconds();
        // This is not a PID-reuse-proof process-birth timestamp.
        let start_time = wall_seconds();
        let hostname = windows_hostname();
        let mut store = WindowsFilesystemStore { root: &root };
        let state = boot_with_store(
            &mut store,
            &journal,
            writer_id.clone(),
            run_id,
            hostname,
            now,
            start_time,
        )?;
        let last_orphan_sweep = sweep::sweep_orphans(&journal, Duration::from_secs(1));
        Ok(SupervisorLifecycle {
            journal,
            writer_id,
            run_id: state.run_id,
            heartbeat_filename: state.heartbeat_filename,
            last_orphan_sweep,
            _journal_root: root,
            _lease: lease,
            identity: state.identity,
            retained_self_heartbeat: Some(state.retained_self_heartbeat),
            retained_readiness: None,
            last_completed_sync_result: Some(state.last_completed_sync_result),
            stale_heartbeat_gc: super::StaleHeartbeatGc::default(),
        })
    }

    pub(crate) struct WindowsFilesystemStore<'a> {
        root: &'a JournalRoot,
    }

    pub(crate) fn filesystem_store(root: &JournalRoot) -> WindowsFilesystemStore<'_> {
        WindowsFilesystemStore { root }
    }

    pub(crate) fn hostname() -> String {
        windows_hostname()
    }

    impl WindowsFilesystemStore<'_> {
        fn health(&self) -> Result<WindowsFlatDirectory, FlatDirectoryError> {
            let path = self.root.canonical_path().join("health");
            open_windows_flat_directory_bound(
                self.root,
                OsStr::new("health"),
                self.root.canonical_path(),
            )?
            .ok_or_else(|| missing_directory(path))
        }

        fn sync(&self) -> Result<WindowsFlatDirectory, FlatDirectoryError> {
            let health = self.health()?;
            let health_path = self.root.canonical_path().join("health");
            open_windows_flat_directory_bound(&health, OsStr::new("sync"), &health_path)?
                .ok_or_else(|| missing_directory(health_path.join("sync")))
        }

        fn directory(
            &self,
            directory: WindowsLifecycleDirectory,
        ) -> Result<WindowsFlatDirectory, FlatDirectoryError> {
            match directory {
                WindowsLifecycleDirectory::Health => self.health(),
                WindowsLifecycleDirectory::Sync => self.sync(),
            }
        }

        fn relative_path(directory: WindowsLifecycleDirectory, name: &str) -> String {
            match directory {
                WindowsLifecycleDirectory::Health => format!("health/{name}"),
                WindowsLifecycleDirectory::Sync => format!("health/sync/{name}"),
            }
        }

        fn write_and_retain(
            &self,
            directory: WindowsLifecycleDirectory,
            name: &str,
            body: &[u8],
            limit: usize,
        ) -> Result<FileObservation, WindowsPublicationFailure> {
            let destination = self
                .root
                .canonical_path()
                .join(Self::relative_path(directory, name));
            match atomic_replace_detailed(&destination, body, 0o600) {
                Ok(DetailedAtomicOutcome::Published) => {}
                Ok(DetailedAtomicOutcome::PublishedDurabilityUncertain { source }) => {
                    return Err(WindowsPublicationFailure::Durability(source));
                }
                Ok(DetailedAtomicOutcome::PublishedParentPathRaced { sync_error: _ }) => {
                    return Err(WindowsPublicationFailure::Uncertain(
                        publication_observation_error("published parent path changed"),
                    ));
                }
                Ok(DetailedAtomicOutcome::PublishedParentPathUnverified {
                    observation,
                    sync_error: _,
                }) => {
                    return Err(WindowsPublicationFailure::Uncertain(
                        publication_observation_error(&format!(
                            "published parent path could not be reverified: {observation}"
                        )),
                    ));
                }
                Err(source) => return Err(WindowsPublicationFailure::Publish(source)),
            }
            let directory = self
                .directory(directory)
                .map_err(WindowsPublicationFailure::Observation)?;
            let observation =
                read_windows_observed_file_bounded(&directory, OsStr::new(name), limit)
                    .map_err(WindowsPublicationFailure::Observation)?
                    .ok_or(WindowsPublicationFailure::Missing)?;
            if observation.bytes != body {
                return Err(WindowsPublicationFailure::BytesMismatched);
            }
            Ok(observation)
        }
    }

    impl WindowsLifecycleStore for WindowsFilesystemStore<'_> {
        fn scan_sync(
            &mut self,
            self_filename: &str,
            previous: Option<&SyncSnapshot>,
            now: f64,
        ) -> Result<SyncCheckResult, SyncScanFailure> {
            let sync = self.sync().map_err(|reason| {
                directory_failure(
                    self.root.canonical_path().join("health/sync"),
                    SyncDirectoryOperation::BindSync,
                    reason,
                )
            })?;
            scan_windows_sync(&sync, self_filename, previous, now)
        }

        fn write_sync_heartbeat(
            &mut self,
            filename: &str,
            body: &[u8],
        ) -> Result<FileObservation, HeartbeatWriteError> {
            self.write_and_retain(
                WindowsLifecycleDirectory::Sync,
                filename,
                body,
                sync::MAX_SYNC_HEARTBEAT_BYTES,
            )
            .map_err(HeartbeatWriteError::from)
        }

        fn write_lifecycle_artifact(
            &mut self,
            name: &'static str,
            body: &[u8],
        ) -> Result<FileObservation, LifecycleArtifactWriteError> {
            self.write_and_retain(
                WindowsLifecycleDirectory::Health,
                name,
                body,
                MAX_LIFECYCLE_ARTIFACT_BYTES,
            )
            .map_err(|error| LifecycleArtifactWriteError::from((name, error)))
        }

        fn remove_observed(
            &mut self,
            directory: WindowsLifecycleDirectory,
            name: &OsStr,
            observation: &FileObservation,
        ) -> Result<(), WindowsRemovalFailure> {
            let name = name.to_str().ok_or_else(|| {
                WindowsRemovalFailure::new("entry name is not losslessly representable as UTF-8")
            })?;
            let directory_handle = self
                .directory(directory)
                .map_err(|error| WindowsRemovalFailure::new(error.to_string()))?;
            let limit = match directory {
                WindowsLifecycleDirectory::Health => MAX_LIFECYCLE_ARTIFACT_BYTES,
                WindowsLifecycleDirectory::Sync => sync::MAX_SYNC_HEARTBEAT_BYTES,
            };
            let current =
                read_windows_observed_file_bounded(&directory_handle, OsStr::new(name), limit)
                    .map_err(|error| WindowsRemovalFailure::new(error.to_string()))?
                    .ok_or_else(|| {
                        WindowsRemovalFailure::new("retained entry disappeared before deletion")
                    })?;
            if &current != observation {
                return Err(WindowsRemovalFailure::new(
                    "retained entry changed before deletion",
                ));
            }
            match remove_file(
                self.root.canonical_path(),
                &Self::relative_path(directory, name),
            )
            .map_err(|error| WindowsRemovalFailure::new(error.to_string()))?
            {
                Removed::Unlinked => Ok(()),
                Removed::AlreadyAbsent => Err(WindowsRemovalFailure::new(
                    "retained entry disappeared during deletion",
                )),
            }
        }
    }

    #[derive(Debug)]
    enum WindowsPublicationFailure {
        Publish(DetailedAtomicError),
        Durability(io::Error),
        Missing,
        Observation(FlatDirectoryError),
        BytesMismatched,
        Uncertain(io::Error),
    }

    impl From<WindowsPublicationFailure> for HeartbeatWriteError {
        fn from(value: WindowsPublicationFailure) -> Self {
            match value {
                WindowsPublicationFailure::Publish(source) => Self::Publish { source },
                WindowsPublicationFailure::Durability(source) => {
                    Self::DurabilityUncertain { source }
                }
                WindowsPublicationFailure::Missing => Self::ObservationMissing,
                WindowsPublicationFailure::Observation(source) => Self::Observation { source },
                WindowsPublicationFailure::BytesMismatched => Self::ObservationBytesMismatched,
                WindowsPublicationFailure::Uncertain(source) => {
                    Self::PublicationObservationUncertain { source }
                }
            }
        }
    }

    impl From<(&'static str, WindowsPublicationFailure)> for LifecycleArtifactWriteError {
        fn from((name, value): (&'static str, WindowsPublicationFailure)) -> Self {
            match value {
                WindowsPublicationFailure::Publish(source) => Self::Publish { name, source },
                WindowsPublicationFailure::Durability(source) => {
                    Self::DurabilityUncertain { name, source }
                }
                WindowsPublicationFailure::Missing => Self::ObservationMissing { name },
                WindowsPublicationFailure::Observation(source) => {
                    Self::Observation { name, source }
                }
                WindowsPublicationFailure::BytesMismatched => {
                    Self::ObservationBytesMismatched { name }
                }
                WindowsPublicationFailure::Uncertain(source) => {
                    Self::PublicationObservationUncertain { name, source }
                }
            }
        }
    }

    fn scan_windows_sync(
        directory: &WindowsFlatDirectory,
        self_filename: &str,
        previous: Option<&SyncSnapshot>,
        now: f64,
    ) -> Result<SyncCheckResult, SyncScanFailure> {
        let folder = PathBuf::from("health/sync");
        let entries = list_windows_flat_directory(directory, sync::MAX_SYNC_DIRECTORY_ENTRIES)
            .map_err(|reason| SyncScanFailure::DirectoryBinding {
                path: folder.clone(),
                operation: SyncDirectoryOperation::InspectSync,
                reason: Box::new(reason),
            })?;
        let entries = entries.ok_or_else(|| SyncScanFailure::CountCapExceeded {
            folder: folder.clone(),
            found_more_than: sync::MAX_SYNC_DIRECTORY_ENTRIES,
            maximum: sync::MAX_SYNC_DIRECTORY_ENTRIES,
        })?;
        let mut pending = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.kind != JournalEntryKind::RegularFile {
                return Err(SyncScanFailure::UnsafeEntry {
                    folder: folder.clone(),
                    name: entry.name,
                    kind: entry.kind,
                    reason: SyncUnsafeReason::NonRegular { kind: entry.kind },
                    source: None,
                });
            }
            let name = entry.name;
            let observation = match read_windows_observed_file_bounded(
                directory,
                &name,
                sync::MAX_SYNC_HEARTBEAT_BYTES,
            ) {
                Ok(Some(observation)) => observation,
                Ok(None) => {
                    return Err(SyncScanFailure::IncompleteSnapshot {
                        folder: folder.clone(),
                        name,
                        reason: SyncIncompleteSnapshotReason::DisappearedDuringObservation,
                    });
                }
                Err(FlatDirectoryError::SizeLimitExceeded { size, limit, .. }) => {
                    return Err(SyncScanFailure::UnsafeEntry {
                        folder: folder.clone(),
                        name,
                        kind: JournalEntryKind::RegularFile,
                        reason: SyncUnsafeReason::OversizedRegular { size, limit },
                        source: None,
                    });
                }
                Err(
                    error @ (FlatDirectoryError::IdentityChanged { .. }
                    | FlatDirectoryError::NotRegular { .. }
                    | FlatDirectoryError::EnumerationChanged { .. }),
                ) => {
                    return Err(SyncScanFailure::IncompleteSnapshot {
                        folder: folder.clone(),
                        name,
                        reason: SyncIncompleteSnapshotReason::ReplacedDuringObservation {
                            source: Box::new(error),
                        },
                    });
                }
                Err(error) => {
                    return Err(SyncScanFailure::UnsafeEntry {
                        folder: folder.clone(),
                        name,
                        kind: JournalEntryKind::RegularFile,
                        reason: SyncUnsafeReason::Unreadable {
                            operation: SyncReadOperation::ReadObservedFileBounded,
                        },
                        source: Some(Box::new(error)),
                    });
                }
            };
            let classification = sync::classify_heartbeat(&name, &observation.bytes);
            pending.push((observation, classification));
        }
        Ok(sync::assemble_sync_result(
            pending,
            self_filename,
            previous,
            now,
        ))
    }

    fn root_failure(journal: &Path, error: JournalRootError) -> LifecycleError {
        let reason = match error {
            JournalRootError::Changed => FlatDirectoryError::IdentityChanged {
                path: journal.to_path_buf(),
            },
            JournalRootError::Invalid { root, reason, .. }
            | JournalRootError::Unsupported { root, reason, .. } => FlatDirectoryError::Io {
                operation: "open journal root",
                path: root,
                source: io::Error::new(io::ErrorKind::InvalidInput, reason),
            },
            JournalRootError::Io {
                operation,
                path,
                source,
            } => FlatDirectoryError::Io {
                operation,
                path,
                source,
            },
        };
        directory_failure(
            journal.to_path_buf(),
            SyncDirectoryOperation::BindHealth,
            reason,
        )
    }

    fn directory_failure(
        path: PathBuf,
        operation: SyncDirectoryOperation,
        reason: FlatDirectoryError,
    ) -> LifecycleError {
        LifecycleError::SyncScan(Box::new(SyncScanFailure::DirectoryBinding {
            path,
            operation,
            reason: Box::new(reason),
        }))
    }

    fn missing_directory(path: PathBuf) -> FlatDirectoryError {
        FlatDirectoryError::Io {
            operation: "open lifecycle directory",
            path,
            source: io::Error::from(io::ErrorKind::NotFound),
        }
    }

    fn publication_observation_error(message: &str) -> io::Error {
        io::Error::other(message)
    }

    fn windows_hostname() -> String {
        let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown-host".to_owned());
        sync::sanitize_hostname(&hostname)
    }

    fn wall_seconds() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |value| value.as_secs_f64())
    }
}

#[cfg(windows)]
pub(crate) use platform::{boot, filesystem_store, hostname};

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::path::Path;

    use solstone_core_journal_io::{
        FlatDirectoryEntry, FlatDirectoryError, JournalEntryKind, NativeMtime,
    };

    use super::*;

    const NOW: f64 = 100_000.0;

    enum ScanStep {
        Entries(Vec<FileObservation>),
        Failure(SyncScanFailure),
        ReadRaced { name: OsString },
    }

    #[derive(Default)]
    struct FakeWindowsLifecycleStore {
        scans: VecDeque<ScanStep>,
        writes: Vec<String>,
        scans_completed: usize,
        assembled_files: Vec<OsString>,
        removal_attempts: Vec<(WindowsLifecycleDirectory, OsString)>,
        removed: Vec<(WindowsLifecycleDirectory, OsString)>,
        next_heartbeat_error: Option<HeartbeatWriteError>,
        next_lifecycle_artifact_error: Option<LifecycleArtifactWriteError>,
        next_removal_error: Option<WindowsRemovalFailure>,
        sequence: u64,
    }

    impl FakeWindowsLifecycleStore {
        fn with_scans(scans: impl IntoIterator<Item = ScanStep>) -> Self {
            Self {
                scans: scans.into_iter().collect(),
                ..Self::default()
            }
        }

        fn next_observation(&mut self, name: &str, bytes: &[u8]) -> FileObservation {
            self.sequence += 1;
            observation(name, bytes.to_vec(), self.sequence, NOW as i64)
        }
    }

    impl WindowsLifecycleStore for FakeWindowsLifecycleStore {
        fn scan_sync(
            &mut self,
            self_filename: &str,
            previous: Option<&SyncSnapshot>,
            now: f64,
        ) -> Result<SyncCheckResult, SyncScanFailure> {
            match self.scans.pop_front().expect("planned scan") {
                ScanStep::Entries(entries) => {
                    let pending = entries.into_iter().map(|observation| {
                        let classification =
                            sync::classify_heartbeat(&observation.entry.name, &observation.bytes);
                        (observation, classification)
                    });
                    let result = sync::assemble_sync_result(pending, self_filename, previous, now);
                    self.scans_completed += 1;
                    self.assembled_files
                        .extend(result.snapshot.files.keys().cloned());
                    Ok(result)
                }
                ScanStep::Failure(error) => Err(error),
                ScanStep::ReadRaced { name } => Err(SyncScanFailure::IncompleteSnapshot {
                    folder: "health/sync".into(),
                    name: name.clone(),
                    reason: sync::SyncIncompleteSnapshotReason::ReplacedDuringObservation {
                        source: Box::new(FlatDirectoryError::IdentityChanged {
                            path: std::path::PathBuf::from("health/sync").join(name),
                        }),
                    },
                }),
            }
        }

        fn write_sync_heartbeat(
            &mut self,
            filename: &str,
            body: &[u8],
        ) -> Result<FileObservation, HeartbeatWriteError> {
            self.writes.push(format!("sync:{filename}"));
            match self.next_heartbeat_error.take() {
                Some(error) => Err(error),
                None => Ok(self.next_observation(filename, body)),
            }
        }

        fn write_lifecycle_artifact(
            &mut self,
            name: &'static str,
            body: &[u8],
        ) -> Result<FileObservation, LifecycleArtifactWriteError> {
            self.writes.push(format!("health:{name}"));
            match self.next_lifecycle_artifact_error.take() {
                Some(error) => Err(error),
                None => Ok(self.next_observation(name, body)),
            }
        }

        fn remove_observed(
            &mut self,
            directory: WindowsLifecycleDirectory,
            name: &OsStr,
            _observation: &FileObservation,
        ) -> Result<(), WindowsRemovalFailure> {
            self.removal_attempts.push((directory, name.to_os_string()));
            if let Some(error) = self.next_removal_error.take() {
                return Err(error);
            }
            self.removed.push((directory, name.to_os_string()));
            Ok(())
        }
    }

    fn writer_id() -> WriterId {
        WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID")
    }

    fn run_id() -> RunId {
        RunId::parse("11111111111111111111111111111111").expect("run ID")
    }

    fn observation(name: &str, bytes: Vec<u8>, inode: u64, mtime_seconds: i64) -> FileObservation {
        FileObservation {
            entry: FlatDirectoryEntry {
                name: OsString::from(name),
                kind: JournalEntryKind::RegularFile,
                device: 1,
                inode,
                size: bytes.len() as u64,
                mtime: NativeMtime {
                    seconds: mtime_seconds,
                    nanoseconds: 0,
                },
            },
            bytes,
        }
    }

    fn peer_observation(run: &str, inode: u64, mtime: i64) -> FileObservation {
        let heartbeat = sync::HeartbeatV2::new(
            WriterId::parse("fedcba9876543210fedcba9876543210").expect("peer writer ID"),
            RunId::parse(run).expect("peer run ID"),
            "peer".to_owned(),
            17,
            (NOW - 1.0).to_string(),
            "test".to_owned(),
            15,
            "/journal".to_owned(),
        );
        let name = sync::v2_heartbeat_filename(&heartbeat.writer_id, &heartbeat.run_id);
        observation(
            &name,
            serde_json::to_vec(&heartbeat).expect("heartbeat JSON"),
            inode,
            mtime,
        )
    }

    fn stale_peer(inode: u64) -> FileObservation {
        let mut peer = peer_observation("22222222222222222222222222222222", inode, 1);
        let heartbeat = sync::HeartbeatV2::new(
            WriterId::parse("fedcba9876543210fedcba9876543210").expect("peer writer ID"),
            RunId::parse("22222222222222222222222222222222").expect("peer run ID"),
            "peer".to_owned(),
            17,
            (NOW - super::super::STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0).to_string(),
            "test".to_owned(),
            15,
            "/journal".to_owned(),
        );
        peer.bytes = serde_json::to_vec(&heartbeat).expect("heartbeat JSON");
        peer.entry.size = peer.bytes.len() as u64;
        peer
    }

    fn boot(store: &mut FakeWindowsLifecycleStore) -> Result<WindowsBootState, LifecycleError> {
        boot_with_store(
            store,
            Path::new("/journal"),
            writer_id(),
            run_id(),
            "host".to_owned(),
            NOW,
            NOW,
        )
    }

    fn tick(
        store: &mut FakeWindowsLifecycleStore,
        gc: &mut StaleHeartbeatGc,
        retained: &mut Option<FileObservation>,
        last: &mut Option<SyncCheckResult>,
        now: f64,
        monotonic: f64,
    ) -> SyncTickOutcome {
        let filename = sync::v2_heartbeat_filename(&writer_id(), &run_id());
        tick_with_store(
            store,
            WindowsTickContext {
                journal: Path::new("/journal"),
                writer_id: &writer_id(),
                run_id: run_id(),
                heartbeat_filename: &filename,
                retained_self_heartbeat: retained,
                last_completed_sync_result: last,
                stale_heartbeat_gc: gc,
            },
            "host".to_owned(),
            None,
            now,
            monotonic,
        )
    }

    #[test]
    fn initial_live_peer_rejection_precedes_heartbeat_and_identity_publication() {
        let mut store =
            FakeWindowsLifecycleStore::with_scans([ScanStep::Entries(vec![peer_observation(
                "33333333333333333333333333333333",
                1,
                NOW as i64,
            )])]);

        assert!(matches!(
            boot(&mut store),
            Err(LifecycleError::AlreadyRunning)
        ));
        assert!(store.writes.is_empty());
    }

    #[test]
    fn initial_unverifiable_live_evidence_precedes_publication() {
        let mut store =
            FakeWindowsLifecycleStore::with_scans([ScanStep::Entries(vec![observation(
                "unverifiable.check",
                b"not heartbeat JSON".to_vec(),
                9,
                NOW as i64,
            )])]);

        assert!(matches!(
            boot(&mut store),
            Err(LifecycleError::AdmissionHeartbeatNeedsAttention { .. })
        ));
        assert!(store.writes.is_empty());
    }

    #[test]
    fn boot_has_no_admission_wait_dependency() {
        let mut store = FakeWindowsLifecycleStore::with_scans([
            ScanStep::Entries(Vec::new()),
            ScanStep::Entries(Vec::new()),
        ]);

        boot(&mut store).expect("one-pass Windows admission");

        assert_eq!(store.scans_completed, 2);
        assert!(store.writes.iter().all(|write| !write.contains("wait-v2")));
    }

    #[test]
    fn self_publication_is_followed_by_second_scan_before_identity() {
        let mut store = FakeWindowsLifecycleStore::with_scans([
            ScanStep::Entries(Vec::new()),
            ScanStep::Entries(Vec::new()),
        ]);

        boot(&mut store).expect("boot");

        assert_eq!(store.scans_completed, 2);
        assert_eq!(store.writes.len(), 3);
        assert!(store.writes[0].starts_with("sync:"));
        assert_eq!(store.writes[1], "health:supervisor.pid");
        assert_eq!(store.writes[2], "health:supervisor.start_time");
    }

    #[test]
    fn post_publication_conflict_cleans_heartbeat_before_identity() {
        let mut store = FakeWindowsLifecycleStore::with_scans([
            ScanStep::Entries(Vec::new()),
            ScanStep::Entries(vec![peer_observation(
                "44444444444444444444444444444444",
                2,
                NOW as i64,
            )]),
        ]);

        assert!(matches!(
            boot(&mut store),
            Err(LifecycleError::AlreadyRunning)
        ));
        assert_eq!(store.writes.len(), 1);
        assert_eq!(store.removed.len(), 1);
        assert_eq!(store.removed[0].0, WindowsLifecycleDirectory::Sync);
    }

    #[test]
    fn incomplete_second_scan_cleans_heartbeat_before_identity() {
        let mut store = FakeWindowsLifecycleStore::with_scans([
            ScanStep::Entries(Vec::new()),
            ScanStep::Failure(SyncScanFailure::CountCapExceeded {
                folder: "health/sync".into(),
                found_more_than: 257,
                maximum: 256,
            }),
        ]);

        assert!(matches!(boot(&mut store), Err(LifecycleError::SyncScan(_))));
        assert_eq!(store.writes.len(), 1);
        assert_eq!(store.removed.len(), 1);
    }

    #[test]
    fn heartbeat_publication_requires_matching_bounded_readback() {
        let mut store = FakeWindowsLifecycleStore::with_scans([ScanStep::Entries(Vec::new())]);
        store.next_heartbeat_error = Some(HeartbeatWriteError::ObservationBytesMismatched);

        assert!(matches!(
            boot(&mut store),
            Err(LifecycleError::HeartbeatWrite(
                HeartbeatWriteError::ObservationBytesMismatched
            ))
        ));
        assert_eq!(store.writes.len(), 1);
    }

    #[test]
    fn identity_publication_failure_cleans_up_self_heartbeat() {
        let mut store = FakeWindowsLifecycleStore::with_scans([
            ScanStep::Entries(Vec::new()),
            ScanStep::Entries(Vec::new()),
        ]);
        store.next_lifecycle_artifact_error =
            Some(LifecycleArtifactWriteError::ObservationMissing {
                name: SUPERVISOR_PID,
            });

        assert!(matches!(
            boot(&mut store),
            Err(LifecycleError::LifecycleArtifactWrite(
                LifecycleArtifactWriteError::ObservationMissing {
                    name: SUPERVISOR_PID
                }
            ))
        ));
        assert_eq!(
            store.removed,
            vec![(
                WindowsLifecycleDirectory::Sync,
                OsString::from(sync::v2_heartbeat_filename(&writer_id(), &run_id())),
            )]
        );
    }

    #[test]
    fn identical_stale_observations_remove_once_on_second_tick() {
        let stale = stale_peer(10);
        let mut store = FakeWindowsLifecycleStore::with_scans([
            ScanStep::Entries(vec![stale.clone()]),
            ScanStep::Entries(vec![stale]),
        ]);
        let mut gc = StaleHeartbeatGc::default();
        let mut retained = None;
        let mut last = None;

        assert!(matches!(
            tick(&mut store, &mut gc, &mut retained, &mut last, NOW, 1.0),
            SyncTickOutcome::Healthy
        ));
        assert!(store.removed.is_empty());
        assert!(matches!(
            tick(
                &mut store,
                &mut gc,
                &mut retained,
                &mut last,
                NOW + 1.0,
                2.0
            ),
            SyncTickOutcome::Healthy
        ));
        assert_eq!(store.removed.len(), 1);
    }

    #[test]
    fn replacement_identity_requires_two_new_identical_ticks() {
        let first = stale_peer(20);
        let replacement = stale_peer(21);
        let mut store = FakeWindowsLifecycleStore::with_scans([
            ScanStep::Entries(vec![first]),
            ScanStep::Entries(vec![replacement.clone()]),
            ScanStep::Entries(vec![replacement]),
        ]);
        let mut gc = StaleHeartbeatGc::default();
        let mut retained = None;
        let mut last = None;

        tick(&mut store, &mut gc, &mut retained, &mut last, NOW, 10.0);
        tick(
            &mut store,
            &mut gc,
            &mut retained,
            &mut last,
            NOW + 1.0,
            11.0,
        );
        assert!(store.removed.is_empty());
        tick(
            &mut store,
            &mut gc,
            &mut retained,
            &mut last,
            NOW + 2.0,
            12.0,
        );
        assert_eq!(store.removed.len(), 1);
    }

    #[test]
    fn clock_discontinuity_resets_stale_evidence_without_removal() {
        let stale = stale_peer(22);
        let mut store = FakeWindowsLifecycleStore::with_scans([
            ScanStep::Entries(vec![stale.clone()]),
            ScanStep::Entries(vec![stale.clone()]),
            ScanStep::Entries(vec![stale]),
        ]);
        let mut gc = StaleHeartbeatGc::default();
        let mut retained = None;
        let mut last = None;

        tick(&mut store, &mut gc, &mut retained, &mut last, NOW, 10.0);
        tick(
            &mut store,
            &mut gc,
            &mut retained,
            &mut last,
            NOW - 1.0,
            11.0,
        );
        tick(&mut store, &mut gc, &mut retained, &mut last, NOW, 12.0);

        assert!(store.removed.is_empty());
    }

    #[test]
    fn verify_then_delete_failure_is_reported_and_never_claimed_as_removal() {
        let stale = stale_peer(30);
        let mut store = FakeWindowsLifecycleStore::with_scans([
            ScanStep::Entries(vec![stale.clone()]),
            ScanStep::Entries(vec![stale]),
        ]);
        store.next_removal_error = Some(WindowsRemovalFailure::new("retained entry changed"));
        let mut gc = StaleHeartbeatGc::default();
        let mut retained = None;
        let mut last = None;

        tick(&mut store, &mut gc, &mut retained, &mut last, NOW, 1.0);
        assert!(matches!(
            tick(
                &mut store,
                &mut gc,
                &mut retained,
                &mut last,
                NOW + 1.0,
                2.0
            ),
            SyncTickOutcome::StaleHeartbeatCollectionFailure(
                StaleHeartbeatCollectionError::WindowsRemoval { .. }
            )
        ));
        assert_eq!(store.removal_attempts.len(), 1);
        assert!(store.removed.is_empty());
    }

    #[test]
    fn incomplete_bounded_read_never_enters_snapshot_or_gc() {
        let filename = OsString::from("peer.check");
        let mut store = FakeWindowsLifecycleStore::with_scans([ScanStep::ReadRaced {
            name: filename.clone(),
        }]);
        let mut gc = StaleHeartbeatGc::default();
        let mut retained = None;
        let mut last = None;

        assert!(matches!(
            tick(&mut store, &mut gc, &mut retained, &mut last, NOW, 1.0),
            SyncTickOutcome::CompleteScanFailure(SyncScanFailure::IncompleteSnapshot { .. })
        ));
        assert!(!store.assembled_files.contains(&filename));
        assert!(store.removal_attempts.is_empty());
    }
}
