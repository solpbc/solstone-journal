// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Supervisor lifecycle primitives. This module owns `health/` operational
//! state but deliberately does not provide a supervisor binary or CLI.

mod clock;
#[cfg(target_os = "macos")]
mod darwin_parent_watch;
mod hosted_service;
mod parent;
mod parent_loss_admission;
mod parent_loss_coordinator;
mod parent_loss_ledger;
mod readiness;
mod shutdown;
#[cfg(unix)]
mod startup;
mod state;
mod sweep;
mod sync;
mod windows;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use solstone_core_journal_io::{
    BoundParentLock, ExistingParentLockError, FileObservation, JournalEntryKind, JournalRoot,
};
#[cfg(unix)]
use solstone_core_journal_io::{
    ClaimName, ClaimRemovalError, ClaimRemovalOutcome, FlatDirectory, claim_and_remove_observed,
};
use thiserror::Error;

pub use clock::AdmissionWaitClock;
#[cfg(target_os = "macos")]
pub use darwin_parent_watch::{DarwinParentExitWatcher, DarwinParentWatchError};
pub use hosted_service::{
    HostedServiceAdmissionFailure, HostedServiceParentLossError, HostedServiceParentRuntime,
    HostedServiceShutdownEvidence, HostedServiceWatchError, admit_hosted_service_parent,
};
pub use parent::{
    DeclaredParent, ParentAdmissionFailure, ParentExitWatchError, ParentLossReason, ParentWatch,
    ParentWatchStatus, PlatformParentExitWatcher,
};
pub use parent_loss_admission::{
    AdmissionAcknowledgement, AdmissionIdentity, AdmissionIntent, AdmissionResult,
    AdmissionResultState, HOSTED_GENERATION_ENV, HOSTED_LAUNCH_ID_ENV, HOSTED_PARENT_LAUNCH_ID_ENV,
    ParentLossAdmissionError, ParentLossServiceWitnessDrop, acknowledge_hosted_child_admission,
    acknowledge_parent_loss_admission, read_parent_loss_admission_acknowledgement,
    write_parent_loss_admission_intent, write_parent_loss_admission_result,
    write_parent_loss_service_witness,
};
pub use parent_loss_coordinator::{
    CoordinatorBootstrap, CoordinatorBootstrapError, CoordinatorBootstrapReady,
    PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE, ParentLossCoordinator, ParentLossCoordinatorError,
    write_retire_expected_control,
};
pub use parent_loss_ledger::{
    ActiveGeneration, BootstrapReservation, HostedServiceKind, PARENT_LOSS_LEDGER_SCHEMA_V1,
    ParentLossGeneration, ParentLossLedger, ParentLossLedgerError, ParentLossPhase,
    ParentLossReaderOutcome, ParentLossTerminalDisposition, ParentLossUnresolvedReason,
    read_parent_loss_outcome,
};
pub use readiness::{ReadinessMarker, START_TIME_TOLERANCE_SECONDS};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use readiness::{readiness_is_valid, wait_ready, wait_ready_with};
pub use shutdown::{
    ArtifactClearOutcome, ShutdownDisposition, ShutdownDriver, ShutdownOutcome, ShutdownPhase,
    ShutdownRegime, ShutdownReport, shutdown,
};
#[cfg(unix)]
pub use startup::{
    ADMISSION_WAIT_ACTIVE_COPY, ADMISSION_WAIT_TERMINAL_COPY, ADMISSION_WAIT_TRANSIENT_COPY,
    ADMISSION_WAIT_UNVERIFIABLE_COPY, AdmissionWaitMarkerProblem, AdmissionWaitTerminalReason,
    PreReadySupervisorLifecycle, SupervisorBootAdmission,
};
#[cfg(unix)]
pub use state::{AdmissionWaitMarkerCleanupError, clear_self_heartbeat, write_sync_heartbeat};
pub use state::{
    HeartbeatWriteError, LifecycleArtifactWriteError, SelfHeartbeatRemoval, append_supervisor_log,
    clear_ready as clear_readiness, compact_log_if_oversized, recorded_supervisor_pid,
};
pub use sweep::{OrphanSweepOutcome, OrphanSweepReport, sweep_orphans};
pub use sync::{
    ADMISSION_WAIT_MARKER_SCHEMA_V2, AdmissionWaitMarker, AdmissionWaitReason,
    DEFAULT_INTERVAL_SECONDS, FRESH_WINDOW_MULTIPLIER, FRESH_WINDOW_SECONDS, HEARTBEAT_SCHEMA_V1,
    HEARTBEAT_SCHEMA_V2, Heartbeat, HeartbeatClassification, HeartbeatV2,
    MAX_SYNC_DIRECTORY_ENTRIES, MAX_SYNC_HEARTBEAT_BYTES, RunId, RunIdGenerationError,
    SyncCheckResult, SyncConflictEvent, SyncDirectoryOperation, SyncIncompleteSnapshotReason,
    SyncPeerDiagnostic, SyncPeerIdentity, SyncPeerObservation, SyncReadOperation, SyncScanFailure,
    SyncSnapshot, SyncUnsafeReason, V2HeartbeatFilenameError, WriterId, WriterIdParseError,
    admission_wait_marker_filename, is_admission_wait_marker_filename_candidate,
    parse_admission_wait_marker_filename, parse_v2_heartbeat_filename, sanitize_hostname,
    sync_conflict_event, sync_peer_diagnostic, v2_heartbeat_filename,
};
#[cfg(unix)]
pub use sync::{SyncRescan, rescan_sync_read_only};
#[cfg(all(windows, feature = "test-hooks"))]
pub use windows::{
    run_with_windows_lifecycle_checkpoint, run_with_windows_lifecycle_deletion_attempt_witness,
};

/// Result of a supervisor heartbeat renewal and complete peer scan.
#[derive(Debug)]
pub enum SyncTickOutcome {
    Healthy,
    Conflict(Box<SyncCheckResult>),
    RenewalFailure(HeartbeatWriteError),
    CompleteScanFailure(SyncScanFailure),
    RetainedObservationFailure(HeartbeatWriteError),
    StaleHeartbeatCollectionFailure(StaleHeartbeatCollectionError),
}

/// A failed identity-safe collection of a proven stale v2 heartbeat.
#[derive(Debug, Error)]
pub enum StaleHeartbeatCollectionError {
    #[cfg(unix)]
    #[error("could not generate a stale-heartbeat claim name: {source}")]
    ClaimName {
        #[source]
        source: Box<LifecycleError>,
    },
    #[cfg(unix)]
    #[error("stale heartbeat {filename:?} was not cleanly removed: {outcome:?}")]
    Outcome {
        filename: OsString,
        outcome: ClaimRemovalOutcome,
    },
    #[cfg(unix)]
    #[error("could not claim and remove stale heartbeat {filename:?}: {source}")]
    Claim {
        filename: OsString,
        #[source]
        source: Box<ClaimRemovalError>,
    },
    #[error("could not verify and remove stale heartbeat {filename:?}: {reason}")]
    WindowsRemoval { filename: OsString, reason: String },
    #[error("stale-heartbeat successful-tick sequence exhausted")]
    TickSequenceExhausted,
}

/// Lifecycle failures that are meaningful to a supervisor host.
#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error("supervisor already running")]
    AlreadyRunning,
    #[error("lifecycle I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[cfg(unix)]
    #[error("lifecycle system call failed: {0}")]
    Nix(#[from] nix::errno::Errno),
    #[error("host capability unavailable: {needed}")]
    CapabilityUnavailable { needed: &'static str },
    #[error("lifecycle JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid supervisor identity: {0}")]
    Identity(&'static str),
    #[error("could not generate supervisor run identity: {0}")]
    RunIdGeneration(#[from] RunIdGenerationError),
    #[error("invalid heartbeat filename")]
    InvalidHeartbeatFilename,
    #[error("sync scan failed: {0}")]
    SyncScan(#[source] Box<SyncScanFailure>),
    #[error("could not acquire bound supervisor lock: {0}")]
    SupervisorLock(#[source] ExistingParentLockError),
    #[error("heartbeat publication or retention failed: {0}")]
    HeartbeatWrite(#[from] HeartbeatWriteError),
    #[error("admission-wait marker publication or retention failed: {0}")]
    AdmissionWaitMarkerPublication(#[source] HeartbeatWriteError),
    #[cfg(unix)]
    #[error("admission-wait marker cleanup failed: {0}")]
    AdmissionWaitMarkerCleanup(#[source] AdmissionWaitMarkerCleanupError),
    #[cfg(unix)]
    #[error("admission-wait marker needs attention: {0}")]
    AdmissionWaitMarkerNeedsAttention(AdmissionWaitMarkerProblem),
    #[error("another admitting solstone process is waiting on this journal")]
    AdmissionWaitMarkerLive,
    #[error("heartbeat evidence {filename:?} needs attention: {reason}")]
    AdmissionHeartbeatNeedsAttention {
        filename: OsString,
        reason: &'static str,
    },
    #[error("could not establish this process's identity for an admission-wait marker")]
    AdmissionWaitProcessIdentity,
    #[cfg(unix)]
    #[error("{0}")]
    AdmissionWaitTerminal(AdmissionWaitTerminalReason),
    #[error("post-publication self-heartbeat cleanup failed: {0}")]
    PostPublicationHeartbeatCleanup(SelfHeartbeatRemoval),
    #[error("lifecycle artifact publication failed: {0}")]
    LifecycleArtifactWrite(#[from] LifecycleArtifactWriteError),
    #[error("lifecycle artifact {name} cleanup failed: {outcome}")]
    LifecycleArtifactCleanup {
        name: &'static str,
        outcome: SelfHeartbeatRemoval,
    },
}

/// Supervisor liveness distinguishes a proven down state from unavailable proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorLiveness {
    Up,
    Down,
    Unverifiable,
}

/// Held singleton admission and retained descriptor capabilities.
pub struct SupervisorLifecycle {
    journal: PathBuf,
    writer_id: WriterId,
    run_id: RunId,
    heartbeat_filename: String,
    last_orphan_sweep: OrphanSweepOutcome,
    _journal_root: JournalRoot,
    #[cfg(unix)]
    _health: FlatDirectory,
    #[cfg(unix)]
    sync: FlatDirectory,
    _lease: BoundParentLock,
    identity: state::SupervisorIdentityArtifacts,
    retained_self_heartbeat: Option<FileObservation>,
    retained_readiness: Option<FileObservation>,
    last_completed_sync_result: Option<SyncCheckResult>,
    stale_heartbeat_gc: StaleHeartbeatGc,
}

const STALE_HEARTBEAT_MINIMUM_AGE_SECONDS: f64 = 24.0 * 60.0 * 60.0;

#[derive(Clone)]
struct StaleHeartbeatSample {
    observation: FileObservation,
    tick: u64,
}

struct StaleHeartbeatCandidate {
    first: StaleHeartbeatSample,
    second: Option<StaleHeartbeatSample>,
}

#[derive(Clone, Copy)]
struct TickClockSample {
    wall_seconds: f64,
    monotonic_seconds: f64,
}

#[derive(Default)]
pub(crate) struct StaleHeartbeatGc {
    candidates: BTreeMap<OsString, StaleHeartbeatCandidate>,
    last_tick_clock: Option<TickClockSample>,
    successful_tick: u64,
}

impl StaleHeartbeatGc {
    pub(crate) fn begin_tick(&mut self, wall_seconds: f64, monotonic_seconds: f64) -> bool {
        let discontinuous = self.last_tick_clock.is_some_and(|previous| {
            clock::wall_clock_discontinuous(
                previous.wall_seconds,
                wall_seconds,
                previous.monotonic_seconds,
                monotonic_seconds,
            )
        });
        self.last_tick_clock = Some(TickClockSample {
            wall_seconds,
            monotonic_seconds,
        });
        if discontinuous {
            self.clear_candidates();
        }
        discontinuous
    }

    pub(crate) fn clear_candidates(&mut self) {
        self.candidates.clear();
    }

    fn observe_completed_tick(
        &mut self,
        result: &SyncCheckResult,
        self_filename: &str,
        now: f64,
        mut remove: impl FnMut(&OsStr, &FileObservation) -> Result<(), StaleHeartbeatCollectionError>,
    ) -> Result<Vec<OsString>, StaleHeartbeatCollectionError> {
        let Some(tick) = self.successful_tick.checked_add(1) else {
            self.clear_candidates();
            return Err(StaleHeartbeatCollectionError::TickSequenceExhausted);
        };
        self.successful_tick = tick;

        let eligible = eligible_stale_observations(result, self_filename, now);
        self.candidates
            .retain(|filename, _| eligible.contains_key(filename));

        let mut ready_to_collect = Vec::new();
        for (filename, observation) in eligible {
            let matches_previous = self.candidates.get(&filename).is_some_and(|candidate| {
                candidate.first.tick.checked_add(1) == Some(tick)
                    && candidate.first.observation == observation
            });
            if matches_previous {
                let candidate = self
                    .candidates
                    .get_mut(&filename)
                    .expect("candidate was checked before update");
                candidate.second = Some(StaleHeartbeatSample {
                    observation: observation.clone(),
                    tick,
                });
                let second = candidate
                    .second
                    .as_ref()
                    .expect("second observation was retained");
                debug_assert_eq!(second.tick, tick);
                ready_to_collect.push((filename, second.observation.clone()));
            } else {
                self.candidates.insert(
                    filename,
                    StaleHeartbeatCandidate {
                        first: StaleHeartbeatSample { observation, tick },
                        second: None,
                    },
                );
            }
        }

        let mut removed = Vec::new();
        for (filename, observation) in ready_to_collect {
            match remove(&filename, &observation) {
                Ok(()) => {
                    self.candidates.remove(&filename);
                    removed.push(filename);
                }
                Err(error) => {
                    self.clear_candidates();
                    return Err(error);
                }
            }
        }
        Ok(removed)
    }
}

fn eligible_stale_observations(
    result: &SyncCheckResult,
    self_filename: &str,
    now: f64,
) -> BTreeMap<OsString, FileObservation> {
    result
        .peer_observations
        .iter()
        .filter_map(|peer| {
            if peer.source_filename == OsStr::new(self_filename) {
                return None;
            }
            let observation = result.snapshot.files.get(&peer.source_filename)?;
            if observation.entry.kind != JournalEntryKind::RegularFile {
                return None;
            }
            let timestamp = match &peer.classification {
                HeartbeatClassification::SchemaV1(heartbeat)
                | HeartbeatClassification::UnknownFuture(heartbeat) => {
                    heartbeat.wall_time.parse::<f64>().ok()?
                }
                HeartbeatClassification::SchemaV2(heartbeat)
                | HeartbeatClassification::IdentityMismatch(heartbeat) => {
                    heartbeat.wall_time.parse::<f64>().ok()?
                }
                HeartbeatClassification::BoundedMalformed => {
                    if cfg!(windows) {
                        return None;
                    }
                    sync::native_mtime_seconds(observation)
                }
                HeartbeatClassification::AdmissionWaitMarker(_)
                | HeartbeatClassification::AdmissionWaitMarkerIdentityMismatch(_)
                | HeartbeatClassification::AdmissionWaitMarkerMalformed => return None,
            };
            timestamp_is_strictly_stale(timestamp, now)
                .then(|| (peer.source_filename.clone(), observation.clone()))
        })
        .collect()
}

fn timestamp_is_strictly_stale(timestamp: f64, now: f64) -> bool {
    now.is_finite()
        && timestamp.is_finite()
        && timestamp <= now
        && now - timestamp > STALE_HEARTBEAT_MINIMUM_AGE_SECONDS
}

pub(crate) fn remove_collected_from_sync_result(
    result: &mut SyncCheckResult,
    removed: &[OsString],
) {
    for filename in removed {
        result.snapshot.files.remove(filename);
    }
    result
        .peer_observations
        .retain(|peer| !removed.contains(&peer.source_filename));
    result
        .live_peer_observations
        .retain(|peer| !removed.contains(&peer.source_filename));
}

#[cfg(unix)]
fn next_stale_heartbeat_claim_name(
    snapshot: &SyncSnapshot,
) -> Result<ClaimName, StaleHeartbeatCollectionError> {
    loop {
        let claim = state::next_claim_name().map_err(|source| {
            StaleHeartbeatCollectionError::ClaimName {
                source: Box::new(source),
            }
        })?;
        if !snapshot
            .files
            .keys()
            .any(|filename| filename.as_os_str() == claim.as_os_str())
        {
            return Ok(claim);
        }
    }
}

/// Enter supervisor lifecycle ownership and retain the singleton lease.
pub fn boot(
    journal: impl AsRef<Path>,
    writer_id: WriterId,
) -> Result<SupervisorLifecycle, LifecycleError> {
    SupervisorLifecycle::boot(journal, writer_id)
}

impl SupervisorLifecycle {
    pub fn journal(&self) -> &Path {
        &self.journal
    }

    /// Filename of the self-heartbeat established during lifecycle boot.
    pub fn heartbeat_filename(&self) -> &str {
        &self.heartbeat_filename
    }

    /// Acquire descriptor-bound admission, reject live foreign writers, record
    /// identity, sweep matching orphans, and retain the self heartbeat proof.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn boot(journal: impl AsRef<Path>, writer_id: WriterId) -> Result<Self, LifecycleError> {
        SupervisorBootAdmission::acquire(journal, writer_id)?
            .activate()
            .map(PreReadySupervisorLifecycle::into_lifecycle)
    }

    #[cfg(windows)]
    pub fn boot(journal: impl AsRef<Path>, writer_id: WriterId) -> Result<Self, LifecycleError> {
        windows::boot(journal, writer_id)
    }

    /// Other unsupported targets have no supported lifecycle substrate.
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    pub fn boot(_journal: impl AsRef<Path>, _writer_id: WriterId) -> Result<Self, LifecycleError> {
        Err(LifecycleError::Identity(
            "supervisor lifecycle is unsupported on this platform",
        ))
    }

    pub fn signal_ready(
        &mut self,
        ready_at: f64,
        extra: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), LifecycleError> {
        #[cfg(unix)]
        {
            let observation =
                state::write_readiness(&self._health, &self.identity, ready_at, extra)?;
            self.retained_readiness = Some(observation);
            sd_notify("READY=1");
            Ok(())
        }
        #[cfg(windows)]
        {
            let mut store = windows::filesystem_store(&self._journal_root);
            let observation =
                windows::write_readiness_with_store(&mut store, &self.identity, ready_at, extra)?;
            self.retained_readiness = Some(observation);
            sd_notify("READY=1");
            Ok(())
        }
    }

    pub fn clear_ready(&mut self) -> Result<(), LifecycleError> {
        let Some(observation) = self.retained_readiness.clone() else {
            return Ok(());
        };
        #[cfg(unix)]
        {
            let result = state::require_lifecycle_artifact_removed(
                "supervisor.ready",
                state::clear_lifecycle_artifact(
                    &self._health,
                    "supervisor.ready",
                    Some(&observation),
                ),
            );
            if result.is_ok() {
                self.retained_readiness = None;
            }
            result
        }
        #[cfg(windows)]
        {
            let mut store = windows::filesystem_store(&self._journal_root);
            let result = windows::clear_ready_with_store(&mut store, &observation);
            if result.is_ok() {
                self.retained_readiness = None;
            }
            result
        }
    }

    /// Refuse a boot that published its heartbeat but did not publish
    /// readiness. Every cleanup is attempted; the earliest failure wins.
    pub fn abort_before_ready(&self) -> Result<(), LifecycleError> {
        #[cfg(unix)]
        {
            let heartbeat = match state::clear_self_heartbeat(
                &self.sync,
                &self.heartbeat_filename,
                self.retained_self_heartbeat.as_ref(),
            ) {
                Ok(state::SelfHeartbeatRemoval::Removed) => Ok(()),
                Ok(outcome) => Err(LifecycleError::PostPublicationHeartbeatCleanup(outcome)),
                Err(error) => Err(error),
            };
            let readiness = match self.retained_readiness.as_ref() {
                Some(observation) => state::require_lifecycle_artifact_removed(
                    "supervisor.ready",
                    state::clear_lifecycle_artifact(
                        &self._health,
                        "supervisor.ready",
                        Some(observation),
                    ),
                ),
                None => Ok(()),
            };
            let identity = state::clear_supervisor_identity(&self._health, &self.identity);
            heartbeat?;
            readiness?;
            identity
        }
        #[cfg(windows)]
        {
            let mut store = windows::filesystem_store(&self._journal_root);
            windows::abort_before_ready_with_store(
                &mut store,
                &self.heartbeat_filename,
                self.retained_self_heartbeat.as_ref(),
                self.retained_readiness.as_ref(),
                &self.identity,
            )
        }
    }

    pub fn last_orphan_sweep(&self) -> &OrphanSweepOutcome {
        &self.last_orphan_sweep
    }

    /// Publish and retain the self heartbeat, then complete a peer scan.
    ///
    /// A completed result is retained only for `Healthy` and `Conflict`, so
    /// callers cannot mistake a failed scan for a new snapshot.
    pub fn tick_sync(&mut self, previous: Option<&SyncSnapshot>, now: f64) -> SyncTickOutcome {
        self.tick_sync_at(previous, now, monotonic_seconds())
    }

    /// Testable heartbeat renewal and scan with explicitly injected wall and
    /// monotonic clocks. The tick path never sleeps.
    pub fn tick_sync_with(
        &mut self,
        previous: Option<&SyncSnapshot>,
        clock: &mut dyn AdmissionWaitClock,
    ) -> SyncTickOutcome {
        self.tick_sync_at(previous, clock.wall_seconds(), clock.monotonic_seconds())
    }

    fn tick_sync_at(
        &mut self,
        previous: Option<&SyncSnapshot>,
        now: f64,
        monotonic_now: f64,
    ) -> SyncTickOutcome {
        #[cfg(windows)]
        {
            let mut store = windows::filesystem_store(&self._journal_root);
            return windows::tick_with_store(
                &mut store,
                windows::WindowsTickContext {
                    journal: &self.journal,
                    writer_id: &self.writer_id,
                    run_id: self.run_id,
                    heartbeat_filename: &self.heartbeat_filename,
                    retained_self_heartbeat: &mut self.retained_self_heartbeat,
                    last_completed_sync_result: &mut self.last_completed_sync_result,
                    stale_heartbeat_gc: &mut self.stale_heartbeat_gc,
                },
                windows::hostname(),
                previous,
                now,
                monotonic_now,
            );
        }
        #[cfg(unix)]
        {
            let previous = previous.cloned().or_else(|| {
                self.last_completed_sync_result
                    .as_ref()
                    .map(|result| result.snapshot.clone())
            });
            let clock_discontinuous = self.stale_heartbeat_gc.begin_tick(now, monotonic_now);
            let heartbeat = sync::HeartbeatV2::new(
                self.writer_id.clone(),
                self.run_id,
                hostname(),
                std::process::id(),
                now.to_string(),
                env!("CARGO_PKG_VERSION").to_owned(),
                sync::DEFAULT_INTERVAL_SECONDS as u32,
                self.journal.display().to_string(),
            );
            let body = serde_json::to_vec(&heartbeat).expect("heartbeat serializes");
            let observation =
                match state::write_sync_heartbeat(&self.sync, &self.heartbeat_filename, &body) {
                    Ok(observation) => observation,
                    Err(
                        error @ (HeartbeatWriteError::Publish { .. }
                        | HeartbeatWriteError::DurabilityUncertain { .. }),
                    ) => {
                        self.stale_heartbeat_gc.clear_candidates();
                        return SyncTickOutcome::RenewalFailure(error);
                    }
                    Err(
                        error @ (HeartbeatWriteError::ObservationMissing
                        | HeartbeatWriteError::Observation { .. }
                        | HeartbeatWriteError::ObservationBytesMismatched
                        | HeartbeatWriteError::PublicationObservationUncertain { .. }),
                    ) => {
                        self.stale_heartbeat_gc.clear_candidates();
                        return SyncTickOutcome::RetainedObservationFailure(error);
                    }
                    Err(HeartbeatWriteError::InvalidFilename) => {
                        self.stale_heartbeat_gc.clear_candidates();
                        return SyncTickOutcome::RenewalFailure(
                            HeartbeatWriteError::InvalidFilename,
                        );
                    }
                };
            self.retained_self_heartbeat = Some(observation);

            let result = match sync::scan_bound_sync(
                &self.sync,
                &self.heartbeat_filename,
                previous.as_ref(),
                now,
            ) {
                Ok(result) => result,
                Err(error) => {
                    self.stale_heartbeat_gc.clear_candidates();
                    return SyncTickOutcome::CompleteScanFailure(error);
                }
            };
            let mut result = result;
            if !clock_discontinuous {
                let self_filename = self.heartbeat_filename.clone();
                let collected = self.stale_heartbeat_gc.observe_completed_tick(
                    &result,
                    &self_filename,
                    now,
                    |filename, prior| {
                        let claim = next_stale_heartbeat_claim_name(&result.snapshot)?;
                        match claim_and_remove_observed(&self.sync, filename, prior, &claim) {
                            Ok(ClaimRemovalOutcome::Removed) => Ok(()),
                            Ok(outcome) => Err(StaleHeartbeatCollectionError::Outcome {
                                filename: filename.to_os_string(),
                                outcome,
                            }),
                            Err(source) => Err(StaleHeartbeatCollectionError::Claim {
                                filename: filename.to_os_string(),
                                source: Box::new(source),
                            }),
                        }
                    },
                );
                let collected = match collected {
                    Ok(collected) => collected,
                    Err(error) => return SyncTickOutcome::StaleHeartbeatCollectionFailure(error),
                };
                remove_collected_from_sync_result(&mut result, &collected);
            }
            let conflict = result.is_tick_conflict(previous.as_ref());
            self.last_completed_sync_result = Some(result.clone());
            if conflict {
                SyncTickOutcome::Conflict(Box::new(result))
            } else {
                SyncTickOutcome::Healthy
            }
        }
    }

    /// The complete result from the latest healthy or conflicting tick.
    pub fn last_completed_sync_result(&self) -> Option<&SyncCheckResult> {
        self.last_completed_sync_result.as_ref()
    }

    pub fn shutdown(
        &self,
        driver: &mut dyn ShutdownDriver,
        regime: ShutdownRegime,
        sync_conflict: bool,
    ) -> ShutdownOutcome {
        #[cfg(windows)]
        {
            let mut store = windows::filesystem_store(&self._journal_root);
            let (readiness, self_heartbeat, identity) = windows::shutdown_cleanup_with_store(
                &mut store,
                &self.heartbeat_filename,
                self.retained_self_heartbeat.as_ref(),
                self.retained_readiness.as_ref(),
                &self.identity,
                sync_conflict,
            );
            return ShutdownOutcome {
                report: shutdown::shutdown(driver, regime),
                readiness,
                self_heartbeat,
                identity,
            };
        }
        #[cfg(unix)]
        {
            let readiness = match self.retained_readiness.as_ref() {
                Some(observation) => match state::require_lifecycle_artifact_removed(
                    "supervisor.ready",
                    state::clear_lifecycle_artifact(
                        &self._health,
                        "supervisor.ready",
                        Some(observation),
                    ),
                ) {
                    Ok(()) => ArtifactClearOutcome::Cleared,
                    Err(error) => ArtifactClearOutcome::Failed(error.to_string()),
                },
                None => ArtifactClearOutcome::Skipped,
            };
            let clear_identity =
                || match state::clear_supervisor_identity(&self._health, &self.identity) {
                    Ok(()) => ArtifactClearOutcome::Cleared,
                    Err(error) => ArtifactClearOutcome::Failed(error.to_string()),
                };
            let (self_heartbeat, identity) = if sync_conflict {
                (ArtifactClearOutcome::Skipped, ArtifactClearOutcome::Skipped)
            } else {
                match state::clear_self_heartbeat(
                    &self.sync,
                    &self.heartbeat_filename,
                    self.retained_self_heartbeat.as_ref(),
                ) {
                    Ok(SelfHeartbeatRemoval::Removed) => {
                        (ArtifactClearOutcome::Cleared, clear_identity())
                    }
                    Ok(SelfHeartbeatRemoval::NoCleanupAuthority) => {
                        (ArtifactClearOutcome::Skipped, clear_identity())
                    }
                    Ok(outcome) => (
                        ArtifactClearOutcome::Failed(outcome.to_string()),
                        ArtifactClearOutcome::Skipped,
                    ),
                    Err(error) => (
                        ArtifactClearOutcome::Failed(error.to_string()),
                        ArtifactClearOutcome::Skipped,
                    ),
                }
            };
            ShutdownOutcome {
                report: shutdown::shutdown(driver, regime),
                readiness,
                self_heartbeat,
                identity,
            }
        }
    }
}

#[cfg(unix)]
fn hostname() -> String {
    let raw = nix::unistd::gethostname().unwrap_or_default();
    sync::sanitize_hostname(&raw.to_string_lossy())
}

#[cfg(unix)]
fn self_heartbeat_filename(writer_id: &WriterId, run_id: &RunId) -> String {
    sync::v2_heartbeat_filename(writer_id, run_id)
}

#[cfg(unix)]
fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |value| value.as_secs_f64())
}

fn monotonic_seconds() -> f64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Preserve Python's conservative pid-file status contract.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn supervisor_liveness(journal: impl AsRef<Path>) -> SupervisorLiveness {
    supervisor_liveness_with_start_time(journal, state::process_start_time_epoch_seconds)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn supervisor_liveness(_journal: impl AsRef<Path>) -> SupervisorLiveness {
    SupervisorLiveness::Unverifiable
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn supervisor_liveness_with_start_time(
    journal: impl AsRef<Path>,
    process_start_time: impl Fn(u32) -> Result<f64, LifecycleError>,
) -> SupervisorLiveness {
    let health = journal.as_ref().join("health");
    let Ok(pid) = std::fs::read_to_string(health.join("supervisor.pid")).and_then(|text| {
        text.trim()
            .parse::<u32>()
            .map_err(|_| std::io::Error::other("invalid pid"))
    }) else {
        return SupervisorLiveness::Down;
    };
    let Ok(recorded) =
        std::fs::read_to_string(health.join("supervisor.start_time")).and_then(|text| {
            text.trim()
                .parse::<f64>()
                .map_err(|_| std::io::Error::other("invalid start time"))
        })
    else {
        return SupervisorLiveness::Down;
    };
    let Ok(actual) = process_start_time(pid) else {
        return SupervisorLiveness::Down;
    };
    if (recorded - actual).abs() <= START_TIME_TOLERANCE_SECONDS {
        SupervisorLiveness::Up
    } else {
        SupervisorLiveness::Down
    }
}

/// Best-effort systemd readiness notification.
pub fn sd_notify(state_value: &str) {
    let Ok(address) = std::env::var("NOTIFY_SOCKET") else {
        return;
    };
    sd_notify_to(&address, state_value);
}

/// Send `state_value` to a systemd notify socket address (`@abstract` or a
/// filesystem path). Missing or unusable sockets are ignored.
pub fn sd_notify_to(address: &str, state_value: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixDatagram;
        let result = (|| -> std::io::Result<()> {
            let socket = UnixDatagram::unbound()?;
            #[cfg(target_os = "linux")]
            if let Some(name) = address.strip_prefix('@') {
                use std::os::linux::net::SocketAddrExt;
                let target = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
                socket.send_to_addr(state_value.as_bytes(), &target)?;
                return Ok(());
            }
            socket.send_to(state_value.as_bytes(), address)?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("sd_notify failed: {error}");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = address;
        let _ = state_value;
    }
}

#[cfg(all(test, unix))]
mod stale_heartbeat_gc_tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    use crate::process::{ProcessBirth, ProcessInstance};
    use solstone_core_journal_io::{
        ClaimDurability, ClaimRemovalError, ClaimRemovalOutcome, ClaimUnchangedReason,
        FileObservation, FlatDirectoryEntry, IdentityChangeDisposition, JournalEntryKind,
        NativeMtime,
    };

    use super::{
        AdmissionWaitMarker, AdmissionWaitReason, Heartbeat, HeartbeatClassification, HeartbeatV2,
        RunId, STALE_HEARTBEAT_MINIMUM_AGE_SECONDS, StaleHeartbeatCollectionError,
        StaleHeartbeatGc, SyncCheckResult, SyncPeerObservation, SyncSnapshot, WriterId,
        eligible_stale_observations,
    };

    const NOW: f64 = 100_000.0;

    fn writer_id() -> WriterId {
        WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID")
    }

    fn run_id(value: &str) -> RunId {
        RunId::parse(value).expect("run ID")
    }

    fn observation(
        name: String,
        bytes: Vec<u8>,
        inode: u64,
        mtime_seconds: i64,
    ) -> FileObservation {
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

    fn v2_entry(
        wall_time: f64,
        run: &str,
        hostname: &str,
        inode: u64,
        mtime_seconds: i64,
    ) -> (HeartbeatClassification, FileObservation) {
        let heartbeat = HeartbeatV2::new(
            writer_id(),
            run_id(run),
            hostname.to_owned(),
            7,
            wall_time.to_string(),
            "test".to_owned(),
            15,
            "/journal".to_owned(),
        );
        let filename = super::v2_heartbeat_filename(&heartbeat.writer_id, &heartbeat.run_id);
        let bytes = serde_json::to_vec(&heartbeat).expect("v2 JSON");
        (
            HeartbeatClassification::SchemaV2(heartbeat),
            observation(filename, bytes, inode, mtime_seconds),
        )
    }

    fn complete(
        classification: HeartbeatClassification,
        observation: FileObservation,
    ) -> SyncCheckResult {
        let source_filename = observation.entry.name.clone();
        let peer = SyncPeerObservation {
            source_filename: source_filename.clone(),
            classification,
            heartbeat: None,
            is_live: false,
        };
        SyncCheckResult {
            snapshot: SyncSnapshot {
                files: BTreeMap::from([(source_filename, observation)]),
            },
            peer_observations: vec![peer],
            live_peer_observations: Vec::new(),
        }
    }

    fn empty_complete() -> SyncCheckResult {
        SyncCheckResult {
            snapshot: SyncSnapshot::default(),
            peer_observations: Vec::new(),
            live_peer_observations: Vec::new(),
        }
    }

    #[test]
    fn stale_v2_age_gate_is_strict_and_ignores_future_timestamps() {
        let exactly_24h = complete(
            v2_entry(
                NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS,
                "11111111111111111111111111111111",
                "peer",
                1,
                1,
            )
            .0,
            v2_entry(
                NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS,
                "11111111111111111111111111111111",
                "peer",
                1,
                1,
            )
            .1,
        );
        assert!(eligible_stale_observations(&exactly_24h, "self.check", NOW).is_empty());

        let old = v2_entry(
            NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0,
            "22222222222222222222222222222222",
            "peer",
            2,
            2,
        );
        let old = complete(old.0, old.1);
        assert_eq!(
            eligible_stale_observations(&old, "self.check", NOW).len(),
            1
        );

        let future = v2_entry(NOW + 1.0, "33333333333333333333333333333333", "peer", 3, 3);
        let future = complete(future.0, future.1);
        assert!(eligible_stale_observations(&future, "self.check", NOW).is_empty());
    }

    #[test]
    fn stable_candidates_need_two_consecutive_successful_ticks() {
        let entry = v2_entry(
            NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0,
            "44444444444444444444444444444444",
            "peer",
            4,
            4,
        );
        let result = complete(entry.0, entry.1);
        let mut gc = StaleHeartbeatGc::default();
        assert!(
            gc.observe_completed_tick(&result, "self.check", NOW, |_, _| unreachable!())
                .expect("first observation")
                .is_empty()
        );
        assert_eq!(gc.candidates.len(), 1);

        let removed = gc
            .observe_completed_tick(&result, "self.check", NOW + 1.0, |_, _| Ok(()))
            .expect("second identical observation");
        assert_eq!(removed.len(), 1);
        assert!(gc.candidates.is_empty());

        let first = v2_entry(
            NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0,
            "55555555555555555555555555555555",
            "before",
            5,
            5,
        );
        let changed = v2_entry(
            NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0,
            "55555555555555555555555555555555",
            "after",
            5,
            6,
        );
        let first = complete(first.0, first.1);
        let changed = complete(changed.0, changed.1);
        let mut gc = StaleHeartbeatGc::default();
        gc.observe_completed_tick(&first, "self.check", NOW, |_, _| unreachable!())
            .expect("first observation");
        assert!(
            gc.observe_completed_tick(&changed, "self.check", NOW + 1.0, |_, _| unreachable!())
                .expect("changed observation resets evidence")
                .is_empty()
        );
        assert_eq!(gc.candidates.len(), 1);
        assert_eq!(
            gc.candidates
                .values()
                .next()
                .expect("replacement candidate")
                .first
                .observation,
            changed
                .snapshot
                .files
                .values()
                .next()
                .expect("changed file")
                .clone()
        );

        gc.observe_completed_tick(
            &empty_complete(),
            "self.check",
            NOW + 2.0,
            |_, _| unreachable!(),
        )
        .expect("missing candidate only drops its evidence");
        assert!(gc.candidates.is_empty());
    }

    #[test]
    fn discontinuities_clear_all_candidate_evidence_and_skip_that_tick() {
        let entry = v2_entry(
            NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0,
            "66666666666666666666666666666666",
            "peer",
            6,
            6,
        );
        let result = complete(entry.0, entry.1);

        for (wall, monotonic, next_wall, next_monotonic) in [
            (NOW - 1.0, 11.0, NOW + 1.0, 13.0),
            (NOW + 17.0, 11.0, NOW + 18.0, 12.0),
        ] {
            let mut gc = StaleHeartbeatGc::default();
            assert!(!gc.begin_tick(NOW, 10.0));
            gc.observe_completed_tick(&result, "self.check", NOW, |_, _| unreachable!())
                .expect("first observation");
            assert_eq!(gc.candidates.len(), 1);
            assert!(gc.begin_tick(wall, monotonic));
            assert!(gc.candidates.is_empty());
            assert!(!gc.begin_tick(next_wall, next_monotonic));
            assert!(
                gc.observe_completed_tick(&result, "self.check", next_wall, |_, _| {
                    unreachable!()
                })
                .expect("next stable tick starts over")
                .is_empty()
            );
            assert_eq!(gc.candidates.len(), 1);
        }
    }

    #[test]
    fn non_removed_and_claim_errors_clear_candidate_evidence() {
        let entry = v2_entry(
            NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0,
            "77777777777777777777777777777777",
            "peer",
            7,
            7,
        );
        let result = complete(entry.0, entry.1);
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
            let mut gc = StaleHeartbeatGc::default();
            gc.observe_completed_tick(&result, "self.check", NOW, |_, _| unreachable!())
                .expect("first observation");
            assert!(matches!(
                gc.observe_completed_tick(&result, "self.check", NOW + 1.0, |_, _| {
                    Err(StaleHeartbeatCollectionError::Outcome {
                        filename: OsString::from("stale.check"),
                        outcome: outcome.clone(),
                    })
                }),
                Err(StaleHeartbeatCollectionError::Outcome { .. })
            ));
            assert!(gc.candidates.is_empty());
        }

        let mut gc = StaleHeartbeatGc::default();
        gc.observe_completed_tick(&result, "self.check", NOW, |_, _| unreachable!())
            .expect("first observation");
        assert!(matches!(
            gc.observe_completed_tick(&result, "self.check", NOW + 1.0, |filename, _| {
                Err(StaleHeartbeatCollectionError::Claim {
                    filename: filename.to_os_string(),
                    source: Box::new(ClaimRemovalError::ObservationNameMismatch {
                        original: filename.to_os_string(),
                        observed: OsString::from("different.check"),
                    }),
                })
            }),
            Err(StaleHeartbeatCollectionError::Claim { .. })
        ));
        assert!(gc.candidates.is_empty());
    }

    #[test]
    fn all_bounded_regular_classes_except_wait_markers_can_be_candidates() {
        let old = NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0;
        let legacy = Heartbeat {
            schema: 1,
            machine_id: "legacy".to_owned(),
            hostname: "peer".to_owned(),
            pid: 7,
            wall_time: old.to_string(),
            solstone_version: "test".to_owned(),
            interval_seconds: 15,
            journal_path: "/journal".to_owned(),
        };
        let valid_v2 = HeartbeatV2::new(
            writer_id(),
            run_id("88888888888888888888888888888888"),
            "peer".to_owned(),
            7,
            old.to_string(),
            "test".to_owned(),
            15,
            "/journal".to_owned(),
        );
        let marker = AdmissionWaitMarker::new(
            writer_id(),
            run_id("99999999999999999999999999999999"),
            ProcessInstance {
                pid: 7,
                birth: ProcessBirth::linux(10, 100, 100),
            },
            AdmissionWaitReason::FreshNonSelfHeartbeat,
        );
        let candidate_classifications = vec![
            (HeartbeatClassification::SchemaV1(legacy.clone()), 1),
            (HeartbeatClassification::UnknownFuture(legacy), 1),
            (
                HeartbeatClassification::IdentityMismatch(valid_v2.clone()),
                1,
            ),
            (
                HeartbeatClassification::BoundedMalformed,
                if cfg!(windows) { 0 } else { 1 },
            ),
        ];
        for (index, (classification, expected_candidates)) in
            candidate_classifications.into_iter().enumerate()
        {
            let result = complete(
                classification,
                observation(
                    format!("candidate-{index}.check"),
                    b"old".to_vec(),
                    100 + index as u64,
                    (NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0) as i64,
                ),
            );
            let mut gc = StaleHeartbeatGc::default();
            gc.observe_completed_tick(&result, "self.check", NOW, |_, _| unreachable!())
                .expect("first candidate tick");
            assert_eq!(gc.candidates.len(), expected_candidates);
        }

        let marker_classifications = vec![
            HeartbeatClassification::AdmissionWaitMarker(marker.clone()),
            HeartbeatClassification::AdmissionWaitMarkerIdentityMismatch(marker),
            HeartbeatClassification::AdmissionWaitMarkerMalformed,
        ];
        for (index, classification) in marker_classifications.into_iter().enumerate() {
            let result = complete(
                classification,
                observation(
                    format!("marker-{index}.check"),
                    b"old".to_vec(),
                    200 + index as u64,
                    (NOW - STALE_HEARTBEAT_MINIMUM_AGE_SECONDS - 1.0) as i64,
                ),
            );
            let mut gc = StaleHeartbeatGc::default();
            gc.observe_completed_tick(&result, "self.check", NOW, |_, _| unreachable!())
                .expect("first non-candidate tick");
            gc.observe_completed_tick(&result, "self.check", NOW + 1.0, |_, _| unreachable!())
                .expect("second non-candidate tick");
            assert!(gc.candidates.is_empty());
        }

        let self_entry = v2_entry(old, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "self", 200, 0);
        let self_result = complete(self_entry.0, self_entry.1);
        let self_filename = self_result
            .snapshot
            .files
            .keys()
            .next()
            .expect("self filename")
            .to_str()
            .expect("UTF-8 filename");
        assert!(eligible_stale_observations(&self_result, self_filename, NOW).is_empty());
    }
}

#[cfg(all(
    test,
    feature = "test-hooks",
    any(target_os = "linux", target_os = "macos")
))]
mod tests {
    use std::fs;

    use solstone_core_journal_io::{
        BoundPublicationPrimitive, ClaimRemovalPrimitive, run_with_bound_publication_barrier,
        run_with_bound_publication_fault, run_with_claim_removal_barrier,
    };
    use tempfile::Builder;

    use super::{
        ArtifactClearOutcome, LifecycleError, ShutdownDisposition, ShutdownDriver, ShutdownPhase,
        ShutdownRegime, SupervisorLifecycle, SupervisorLiveness, SyncTickOutcome, WriterId, state,
        supervisor_liveness_with_start_time,
    };

    fn temporary_journal() -> tempfile::TempDir {
        Builder::new()
            .prefix("solstone-lifecycle-tick-")
            .tempdir_in("/var/tmp")
            .expect("temporary journal")
    }

    struct Driver(Vec<&'static str>);

    impl ShutdownDriver for Driver {
        fn reap_managed(&mut self, _: std::time::Duration) -> ShutdownDisposition {
            self.0.push("reap");
            ShutdownDisposition::Orderly
        }

        fn drain_tasks(&mut self, _: std::time::Duration) -> ShutdownDisposition {
            self.0.push("drain");
            ShutdownDisposition::Orderly
        }

        fn stop_children(&mut self, _: Option<std::time::Duration>) -> ShutdownDisposition {
            self.0.push("children");
            ShutdownDisposition::Orderly
        }

        fn join_bus(&mut self, _: std::time::Duration) -> ShutdownDisposition {
            self.0.push("bus");
            ShutdownDisposition::Orderly
        }
    }

    fn assert_driver_completed(driver: &Driver, phase: Option<&ShutdownPhase>) {
        assert_eq!(
            driver.0,
            vec!["reap", "drain", "children", "bus"],
            "the shutdown driver must run after cleanup failure"
        );
        assert_eq!(phase, Some(&ShutdownPhase::JoinBusCompleted));
    }

    fn writer_id() -> WriterId {
        WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID")
    }

    #[test]
    fn ac3_injected_start_time_rejects_reused_pid() {
        let root =
            state::test_supervisor_journal("supervisor-probe", std::process::id(), 100.0, None);
        assert_eq!(
            supervisor_liveness_with_start_time(&root, |_| Ok(100.0)),
            SupervisorLiveness::Up
        );
        assert_eq!(
            supervisor_liveness_with_start_time(&root, |_| Ok(101.6)),
            SupervisorLiveness::Down
        );
        state::remove_test_supervisor_journal(root);
    }

    #[test]
    fn ac3_unavailable_start_time_is_not_up() {
        let root = state::test_supervisor_journal(
            "supervisor-start-time-err",
            std::process::id(),
            100.0,
            None,
        );
        assert_eq!(
            supervisor_liveness_with_start_time(&root, |_| {
                Err(LifecycleError::Identity("process start time"))
            }),
            SupervisorLiveness::Down
        );
        state::remove_test_supervisor_journal(root);
    }

    #[test]
    fn signal_ready_notifies_systemd_after_the_marker_is_written() {
        let source = include_str!("mod.rs");
        let signal_ready = source
            .split("pub fn signal_ready(")
            .nth(1)
            .and_then(|rest| rest.split("pub fn clear_ready(").next())
            .expect("signal_ready body");
        assert!(signal_ready.contains("sd_notify(\"READY=1\")"));
        assert!(
            signal_ready.find("write_readiness").expect("marker write")
                < signal_ready.find("sd_notify(\"READY=1\")").expect("notify")
        );
    }

    #[test]
    fn clear_ready_is_idempotent_and_shutdown_does_not_reuse_consumed_authority() {
        let journal = temporary_journal();
        let mut lifecycle = SupervisorLifecycle::boot(journal.path(), writer_id()).expect("boot");
        lifecycle
            .signal_ready(100.0, serde_json::Map::new())
            .expect("publish readiness");

        lifecycle.clear_ready().expect("first clear");
        lifecycle.clear_ready().expect("idempotent clear");
        let mut driver = Driver(Vec::new());
        let outcome = lifecycle.shutdown(&mut driver, ShutdownRegime::Standard, false);

        assert!(matches!(outcome.readiness, ArtifactClearOutcome::Skipped));
        assert_driver_completed(&driver, outcome.report.phases.last());
    }

    #[test]
    fn tick_sync_maps_renewal_retention_scan_conflict_and_healthy_outcomes() {
        let healthy_journal = temporary_journal();
        let mut healthy =
            SupervisorLifecycle::boot(healthy_journal.path(), writer_id()).expect("boot");
        assert!(matches!(
            healthy.tick_sync(None, 10.0),
            SyncTickOutcome::Healthy
        ));
        let previous = healthy
            .last_completed_sync_result()
            .expect("completed healthy scan")
            .snapshot
            .clone();
        fs::write(
            healthy_journal.path().join("health/sync/foreign.check"),
            br#"{"schema":1,"machine_id":"foreign","hostname":"foreign","pid":7,"wall_time":"now","solstone_version":"test","interval_seconds":15,"journal_path":"/journal"}"#,
        )
        .expect("foreign heartbeat");
        assert!(matches!(
            healthy.tick_sync(Some(&previous), 11.0),
            SyncTickOutcome::Conflict(_)
        ));

        let renewal_journal = temporary_journal();
        let mut renewal =
            SupervisorLifecycle::boot(renewal_journal.path(), writer_id()).expect("boot");
        let (renewal_outcome, renewal_fault) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::Write,
            1,
            nix::errno::Errno::EIO as i32,
            || renewal.tick_sync(None, 10.0),
        );
        assert!(renewal_fault);
        assert!(matches!(
            renewal_outcome,
            SyncTickOutcome::RenewalFailure(_)
        ));

        let retention_journal = temporary_journal();
        let mut retention =
            SupervisorLifecycle::boot(retention_journal.path(), writer_id()).expect("boot");
        let self_path = retention_journal
            .path()
            .join("health/sync")
            .join(retention.heartbeat_filename());
        let (retention_outcome, retention_barrier) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::ParentSync,
            1,
            move || fs::remove_file(&self_path).expect("remove just-published heartbeat"),
            || retention.tick_sync(None, 10.0),
        );
        assert!(retention_barrier);
        assert!(matches!(
            retention_outcome,
            SyncTickOutcome::RetainedObservationFailure(_)
        ));

        let scan_journal = temporary_journal();
        let mut scan = SupervisorLifecycle::boot(scan_journal.path(), writer_id()).expect("boot");
        fs::create_dir(scan_journal.path().join("health/sync/unsafe")).expect("unsafe entry");
        assert!(matches!(
            scan.tick_sync(None, 10.0),
            SyncTickOutcome::CompleteScanFailure(_)
        ));
    }

    #[test]
    fn shutdown_runs_driver_after_non_clean_heartbeat_cleanup() {
        let journal = temporary_journal();
        let lifecycle = SupervisorLifecycle::boot(journal.path(), writer_id()).expect("boot");
        let heartbeat = journal
            .path()
            .join("health/sync")
            .join(lifecycle.heartbeat_filename());
        let mut driver = Driver(Vec::new());
        let (result, barrier_fired) = run_with_claim_removal_barrier(
            ClaimRemovalPrimitive::BeforeClaim,
            1,
            move || fs::write(&heartbeat, b"replacement").expect("replace heartbeat"),
            || lifecycle.shutdown(&mut driver, ShutdownRegime::Standard, false),
        );

        assert!(barrier_fired);
        assert!(matches!(
            result.self_heartbeat,
            ArtifactClearOutcome::Failed(_)
        ));
        assert_driver_completed(&driver, result.report.phases.last());
    }

    #[test]
    fn shutdown_reports_readiness_cleanup_failure_and_runs_driver() {
        let journal = temporary_journal();
        let mut lifecycle = SupervisorLifecycle::boot(journal.path(), writer_id()).expect("boot");
        lifecycle
            .signal_ready(100.0, serde_json::Map::new())
            .expect("publish readiness");
        let readiness = journal.path().join("health/supervisor.ready");
        fs::remove_file(&readiness).expect("remove retained readiness");
        fs::create_dir(&readiness).expect("readiness directory");
        fs::write(readiness.join("marker"), b"blocked").expect("non-empty readiness directory");
        let mut driver = Driver(Vec::new());

        let result = lifecycle.shutdown(&mut driver, ShutdownRegime::Standard, false);

        assert!(matches!(result.readiness, ArtifactClearOutcome::Failed(_)));
        assert_driver_completed(&driver, result.report.phases.last());
    }

    #[test]
    fn shutdown_reports_identity_cleanup_failure_and_runs_driver() {
        let journal = temporary_journal();
        let lifecycle = SupervisorLifecycle::boot(journal.path(), writer_id()).expect("boot");
        let identity = journal.path().join("health/supervisor.pid");
        fs::remove_file(&identity).expect("remove supervisor pid");
        fs::create_dir(&identity).expect("identity directory");
        fs::write(identity.join("marker"), b"blocked").expect("non-empty identity directory");
        let mut driver = Driver(Vec::new());

        let result = lifecycle.shutdown(&mut driver, ShutdownRegime::Standard, false);

        assert!(matches!(result.identity, ArtifactClearOutcome::Failed(_)));
        assert_driver_completed(&driver, result.report.phases.last());
    }
}
