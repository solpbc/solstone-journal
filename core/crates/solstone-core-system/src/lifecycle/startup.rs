// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Phased supervisor admission. Hosts that need an interlock between child
//! launch and readiness use these phases instead of the convenience `boot`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use solstone_core_journal_io::{
    BoundParentLock, FlatDirectory, JournalRoot, create_or_open_flat_directory_bound,
};

use super::clock::{AdmissionWaitClock, wall_clock_discontinuous};
use super::{
    LifecycleError, OrphanSweepOutcome, SupervisorLifecycle, epoch_seconds, hostname,
    self_heartbeat_filename,
};
use super::{RunId, WriterId};
use super::{state, sweep, sync};
use crate::process::{
    InspectResult, InstanceVerdict, ProcessInstance, ProcessInstanceSource,
    SystemProcessInstanceSource,
};

pub const ADMISSION_WAIT_TRANSIENT_COPY: &str = "a recent heartbeat from another run was found.\nthe solstone app is waiting while it checks whether that activity continues. it will check again shortly.";
pub const ADMISSION_WAIT_TERMINAL_COPY: &str = "a recent heartbeat from another run is present.\nthe solstone app did not start while that heartbeat was still present.\nwait a moment, then try again.";
pub const ADMISSION_WAIT_ACTIVE_COPY: &str = "another solstone start is waiting after finding recent journal activity.\nthis start did not continue while that wait was active.\nwait a moment, then try again.";
pub const ADMISSION_WAIT_UNVERIFIABLE_COPY: &str =
    "startup status couldn't be verified.\nwait a moment, then try again.";

struct SystemAdmissionWaitClock {
    started: Instant,
}

impl SystemAdmissionWaitClock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl AdmissionWaitClock for SystemAdmissionWaitClock {
    fn wall_seconds(&mut self) -> f64 {
        epoch_seconds()
    }

    fn monotonic_seconds(&mut self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn sleep_until(&mut self, deadline_seconds: f64) {
        let remaining = deadline_seconds - self.monotonic_seconds();
        if remaining.is_sign_positive() {
            std::thread::sleep(Duration::from_secs_f64(remaining));
        }
    }
}

/// Why an on-disk admission marker cannot safely be acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionWaitMarkerProblem {
    Malformed { filename: OsString },
    IdentityMismatch { filename: OsString },
    UnverifiableProcess { filename: OsString },
    MissingRetainedObservation { filename: OsString },
}

impl std::fmt::Display for AdmissionWaitMarkerProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed { filename } => {
                write!(formatter, "marker {filename:?} is malformed or unreadable")
            }
            Self::IdentityMismatch { filename } => {
                write!(
                    formatter,
                    "marker {filename:?} does not match its filename identity"
                )
            }
            Self::UnverifiableProcess { filename } => {
                write!(
                    formatter,
                    "marker {filename:?} has an unverifiable process identity"
                )
            }
            Self::MissingRetainedObservation { filename } => {
                write!(formatter, "marker {filename:?} has no retained observation")
            }
        }
    }
}

/// The reason the bounded recheck refused admission after marker cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionWaitTerminalReason {
    ActivityRemains,
    ClockDiscontinuity,
}

impl std::fmt::Display for AdmissionWaitTerminalReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ActivityRemains => ADMISSION_WAIT_TERMINAL_COPY,
            Self::ClockDiscontinuity => ADMISSION_WAIT_UNVERIFIABLE_COPY,
        })
    }
}

pub struct SupervisorBootAdmission {
    journal: PathBuf,
    root: JournalRoot,
    health: FlatDirectory,
    sync: FlatDirectory,
    lease: BoundParentLock,
    writer_id: WriterId,
    run_id: RunId,
    heartbeat_filename: String,
    admission_snapshot: sync::SyncSnapshot,
    now: f64,
}

pub struct PreReadySupervisorLifecycle {
    journal: PathBuf,
    root: JournalRoot,
    health: FlatDirectory,
    sync: FlatDirectory,
    lease: BoundParentLock,
    writer_id: WriterId,
    run_id: RunId,
    heartbeat_filename: String,
    identity: state::SupervisorIdentityArtifacts,
    retained_self_heartbeat: solstone_core_journal_io::FileObservation,
    last_completed_sync_result: sync::SyncCheckResult,
    last_orphan_sweep: OrphanSweepOutcome,
}

impl SupervisorBootAdmission {
    /// Bind `health`/`sync` beneath a resolved journal root, retain the
    /// singleton lock, and complete the bounded foreign-heartbeat admission
    /// sequence before any identity or real-heartbeat write.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn acquire(journal: impl AsRef<Path>, writer_id: WriterId) -> Result<Self, LifecycleError> {
        let mut clock = SystemAdmissionWaitClock::new();
        let process_source = SystemProcessInstanceSource;
        let mut emit_wait_copy = |copy| eprintln!("{copy}");
        Self::acquire_with(
            journal,
            writer_id,
            &mut clock,
            &process_source,
            &mut emit_wait_copy,
        )
    }

    /// Testable admission path with explicit clock, sleep, process-observation,
    /// and transient-copy dependencies.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn acquire_with(
        journal: impl AsRef<Path>,
        writer_id: WriterId,
        clock: &mut dyn AdmissionWaitClock,
        process_source: &dyn ProcessInstanceSource,
        emit_wait_copy: &mut dyn FnMut(&'static str),
    ) -> Result<Self, LifecycleError> {
        let journal = journal.as_ref().to_path_buf();
        let run_id = RunId::generate()?;
        let root = JournalRoot::open(&journal).map_err(|error| {
            LifecycleError::SyncScan(Box::new(sync::directory_binding_from_root(&journal, error)))
        })?;
        let lock = state::open_supervisor_lock(&root).map_err(|error| match error {
            state::SupervisorLockError::AlreadyRunning => LifecycleError::AlreadyRunning,
            state::SupervisorLockError::BindHealth(reason) => {
                LifecycleError::SyncScan(Box::new(sync::SyncScanFailure::DirectoryBinding {
                    path: root.canonical_path().join("health"),
                    operation: sync::SyncDirectoryOperation::BindHealth,
                    reason: Box::new(reason),
                }))
            }
            state::SupervisorLockError::Acquire(error) => LifecycleError::SupervisorLock(error),
        })?;
        let state::SupervisorLock { health, lease } = lock;
        let health_diagnostic = root.canonical_path().join("health");
        let sync = create_or_open_flat_directory_bound(
            &health,
            OsStr::new("sync"),
            0o700,
            &health_diagnostic,
        )
        .map_err(|reason| {
            LifecycleError::SyncScan(Box::new(sync::SyncScanFailure::DirectoryBinding {
                path: health_diagnostic.join("sync"),
                operation: sync::SyncDirectoryOperation::BindSync,
                reason: Box::new(reason),
            }))
        })?;

        let heartbeat_filename = self_heartbeat_filename(&writer_id, &run_id);
        let first_wall_seconds = clock.wall_seconds();
        let result = sync::scan_bound_sync(&sync, &heartbeat_filename, None, first_wall_seconds)
            .map_err(admission_wait_scan_failure)?;

        remove_stale_wait_markers(
            &sync,
            &result,
            &writer_id,
            first_wall_seconds,
            process_source,
        )?;
        reject_unverifiable_live_heartbeat(&result)?;
        if !result.is_boot_conflict() {
            return Ok(Self {
                journal,
                root,
                health,
                sync,
                lease,
                writer_id,
                run_id,
                heartbeat_filename,
                admission_snapshot: result.snapshot,
                now: first_wall_seconds,
            });
        }

        let process = current_process_instance(process_source)?;
        let marker_filename = sync::admission_wait_marker_filename(&writer_id, &run_id);
        let marker = sync::AdmissionWaitMarker::new(
            writer_id.clone(),
            run_id,
            process,
            sync::AdmissionWaitReason::FreshNonSelfHeartbeat,
        );
        let marker_body = serde_json::to_vec(&marker)?;
        let retained_marker = state::write_sync_heartbeat(&sync, &marker_filename, &marker_body)
            .map_err(LifecycleError::AdmissionWaitMarkerPublication)?;

        let wait_started_monotonic = clock.monotonic_seconds();
        let latest_horizon = sync::latest_live_freshness_horizon(&result)
            .expect("boot conflict has a live heartbeat observation");
        let wait_seconds =
            (latest_horizon - first_wall_seconds).clamp(0.0, sync::FRESH_WINDOW_SECONDS);
        emit_wait_copy(ADMISSION_WAIT_TRANSIENT_COPY);
        clock.sleep_until(wait_started_monotonic + wait_seconds);

        let second_wall_seconds = clock.wall_seconds();
        let second_monotonic = clock.monotonic_seconds();
        let rescan = sync::scan_bound_sync(
            &sync,
            &heartbeat_filename,
            Some(&result.snapshot),
            second_wall_seconds,
        );
        state::clear_admission_wait_marker(&sync, &marker_filename, Some(&retained_marker))?;
        let rescan = rescan.map_err(admission_wait_scan_failure)?;
        reject_unverifiable_live_heartbeat(&rescan)?;

        if wall_clock_discontinuous(
            first_wall_seconds,
            second_wall_seconds,
            wait_started_monotonic,
            second_monotonic,
        ) || rescan.is_boot_conflict()
        {
            return Err(LifecycleError::AdmissionWaitTerminal(
                if wall_clock_discontinuous(
                    first_wall_seconds,
                    second_wall_seconds,
                    wait_started_monotonic,
                    second_monotonic,
                ) {
                    AdmissionWaitTerminalReason::ClockDiscontinuity
                } else {
                    AdmissionWaitTerminalReason::ActivityRemains
                },
            ));
        }

        Ok(Self {
            journal,
            root,
            health,
            sync,
            lease,
            writer_id,
            run_id,
            heartbeat_filename,
            admission_snapshot: rescan.snapshot,
            now: second_wall_seconds,
        })
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn activate(self) -> Result<PreReadySupervisorLifecycle, LifecycleError> {
        let heartbeat = sync::HeartbeatV2::new(
            self.writer_id.clone(),
            self.run_id,
            hostname(),
            std::process::id(),
            self.now.to_string(),
            env!("CARGO_PKG_VERSION").to_owned(),
            sync::DEFAULT_INTERVAL_SECONDS as u32,
            self.journal.display().to_string(),
        );
        let heartbeat_bytes = serde_json::to_vec(&heartbeat)?;
        let retained_self_heartbeat =
            state::write_sync_heartbeat(&self.sync, &self.heartbeat_filename, &heartbeat_bytes)?;
        let post_publication = match sync::scan_bound_sync(
            &self.sync,
            &self.heartbeat_filename,
            Some(&self.admission_snapshot),
            self.now,
        ) {
            Ok(result) => result,
            Err(error) => {
                self.abort_after_publication(&retained_self_heartbeat)?;
                return Err(admission_wait_scan_failure(error));
            }
        };
        if let Err(error) = remove_stale_wait_markers(
            &self.sync,
            &post_publication,
            &self.writer_id,
            self.now,
            &SystemProcessInstanceSource,
        ) {
            self.abort_after_publication(&retained_self_heartbeat)?;
            return Err(error);
        }
        if let Err(error) = reject_unverifiable_live_heartbeat(&post_publication) {
            self.abort_after_publication(&retained_self_heartbeat)?;
            return Err(error);
        }
        if post_publication.is_boot_conflict() {
            self.abort_after_publication(&retained_self_heartbeat)?;
            return Err(LifecycleError::AdmissionWaitTerminal(
                AdmissionWaitTerminalReason::ActivityRemains,
            ));
        }
        let identity = match state::write_supervisor_identity(&self.health, std::process::id()) {
            Ok(identity) => identity,
            Err(error) => {
                self.abort_after_publication(&retained_self_heartbeat)?;
                return Err(error);
            }
        };
        let last_orphan_sweep =
            sweep::sweep_orphans(&self.journal, std::time::Duration::from_secs(1));
        Ok(PreReadySupervisorLifecycle {
            journal: self.journal,
            root: self.root,
            health: self.health,
            sync: self.sync,
            lease: self.lease,
            writer_id: self.writer_id,
            run_id: self.run_id,
            heartbeat_filename: self.heartbeat_filename,
            identity,
            retained_self_heartbeat,
            last_completed_sync_result: post_publication,
            last_orphan_sweep,
        })
    }

    fn abort_after_publication(
        &self,
        retained_self_heartbeat: &solstone_core_journal_io::FileObservation,
    ) -> Result<(), LifecycleError> {
        match state::clear_self_heartbeat(
            &self.sync,
            &self.heartbeat_filename,
            Some(retained_self_heartbeat),
        ) {
            Ok(state::SelfHeartbeatRemoval::Removed) => Ok(()),
            Ok(outcome) => Err(LifecycleError::PostPublicationHeartbeatCleanup(outcome)),
            Err(error) => Err(error),
        }
    }
}

fn reject_unverifiable_live_heartbeat(
    result: &sync::SyncCheckResult,
) -> Result<(), LifecycleError> {
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
    Ok(())
}

fn current_process_instance(
    process_source: &dyn ProcessInstanceSource,
) -> Result<ProcessInstance, LifecycleError> {
    match process_source.inspect(std::process::id()) {
        InspectResult::Present { instance, .. } => Ok(instance),
        InspectResult::Absent | InspectResult::Unverifiable => {
            Err(LifecycleError::AdmissionWaitProcessIdentity)
        }
    }
}

fn remove_stale_wait_markers(
    sync_directory: &FlatDirectory,
    result: &sync::SyncCheckResult,
    local_writer_id: &WriterId,
    now: f64,
    process_source: &dyn ProcessInstanceSource,
) -> Result<(), LifecycleError> {
    let mut stale_markers = Vec::new();
    let mut has_live_marker = false;
    for peer in &result.peer_observations {
        match &peer.classification {
            sync::HeartbeatClassification::AdmissionWaitMarkerMalformed => {
                return Err(LifecycleError::AdmissionWaitMarkerNeedsAttention(
                    AdmissionWaitMarkerProblem::Malformed {
                        filename: peer.source_filename.clone(),
                    },
                ));
            }
            sync::HeartbeatClassification::AdmissionWaitMarkerIdentityMismatch(_) => {
                return Err(LifecycleError::AdmissionWaitMarkerNeedsAttention(
                    AdmissionWaitMarkerProblem::IdentityMismatch {
                        filename: peer.source_filename.clone(),
                    },
                ));
            }
            sync::HeartbeatClassification::AdmissionWaitMarker(marker) => {
                let observation = result
                    .snapshot
                    .files
                    .get(&peer.source_filename)
                    .cloned()
                    .ok_or_else(|| {
                        LifecycleError::AdmissionWaitMarkerNeedsAttention(
                            AdmissionWaitMarkerProblem::MissingRetainedObservation {
                                filename: peer.source_filename.clone(),
                            },
                        )
                    })?;
                if &marker.writer_id != local_writer_id {
                    let modified = sync::native_mtime_seconds(&observation);
                    if now.is_finite()
                        && modified.is_finite()
                        && modified <= now
                        && now - modified > sync::FRESH_WINDOW_SECONDS
                    {
                        stale_markers.push((peer.source_filename.clone(), observation));
                    } else {
                        has_live_marker = true;
                    }
                    continue;
                }
                match process_source.observe(&marker.process) {
                    InstanceVerdict::Unverifiable => {
                        return Err(LifecycleError::AdmissionWaitMarkerNeedsAttention(
                            AdmissionWaitMarkerProblem::UnverifiableProcess {
                                filename: peer.source_filename.clone(),
                            },
                        ));
                    }
                    InstanceVerdict::SameLive { .. } => {
                        has_live_marker = true;
                    }
                    InstanceVerdict::NotSameOrExited => {
                        stale_markers.push((peer.source_filename.clone(), observation));
                    }
                }
            }
            _ => {}
        }
    }

    if has_live_marker {
        return Err(LifecycleError::AdmissionWaitMarkerLive);
    }

    for (filename, observation) in stale_markers {
        let filename = filename.to_str().ok_or_else(|| {
            LifecycleError::AdmissionWaitMarkerNeedsAttention(
                AdmissionWaitMarkerProblem::Malformed {
                    filename: filename.clone(),
                },
            )
        })?;
        state::clear_admission_wait_marker(sync_directory, filename, Some(&observation))?;
    }
    Ok(())
}

fn admission_wait_scan_failure(failure: sync::SyncScanFailure) -> LifecycleError {
    let marker_filename = match &failure {
        sync::SyncScanFailure::UnsafeEntry { name, .. }
        | sync::SyncScanFailure::IncompleteSnapshot { name, .. }
            if sync::is_admission_wait_marker_filename_candidate(name) =>
        {
            Some(name.clone())
        }
        sync::SyncScanFailure::DirectoryBinding { .. }
        | sync::SyncScanFailure::CountCapExceeded { .. }
        | sync::SyncScanFailure::UnsafeEntry { .. }
        | sync::SyncScanFailure::IncompleteSnapshot { .. } => None,
    };
    marker_filename.map_or_else(
        || LifecycleError::SyncScan(Box::new(failure)),
        |filename| {
            LifecycleError::AdmissionWaitMarkerNeedsAttention(
                AdmissionWaitMarkerProblem::Malformed { filename },
            )
        },
    )
}

impl PreReadySupervisorLifecycle {
    pub fn heartbeat_filename(&self) -> &str {
        &self.heartbeat_filename
    }

    pub fn into_lifecycle(self) -> SupervisorLifecycle {
        SupervisorLifecycle {
            journal: self.journal,
            writer_id: self.writer_id,
            run_id: self.run_id,
            heartbeat_filename: self.heartbeat_filename,
            last_orphan_sweep: self.last_orphan_sweep,
            _journal_root: self.root,
            _health: self.health,
            sync: self.sync,
            _lease: self.lease,
            identity: self.identity,
            retained_self_heartbeat: Some(self.retained_self_heartbeat),
            retained_readiness: None,
            last_completed_sync_result: Some(self.last_completed_sync_result),
            stale_heartbeat_gc: super::StaleHeartbeatGc::default(),
        }
    }

    pub fn abort_pre_ready(self) -> Result<(), LifecycleError> {
        let heartbeat = match state::clear_self_heartbeat(
            &self.sync,
            &self.heartbeat_filename,
            Some(&self.retained_self_heartbeat),
        ) {
            Ok(state::SelfHeartbeatRemoval::Removed) => Ok(()),
            Ok(outcome) => Err(LifecycleError::PostPublicationHeartbeatCleanup(outcome)),
            Err(error) => Err(error),
        };
        let identity = state::clear_supervisor_identity(&self.health, &self.identity);
        heartbeat?;
        identity
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;

    use filetime::{FileTime, set_file_mtime};
    use solstone_core_journal_io::{JournalRoot, create_or_open_flat_directory_bound};
    use tempfile::Builder;

    use super::*;
    use crate::process::{ExecutionState, InstanceCensus, ProcessBirth};

    struct FakeClock {
        walls: VecDeque<f64>,
        monotonic: VecDeque<f64>,
        sleep_deadlines: Vec<f64>,
        on_sleep: Option<Box<dyn FnOnce()>>,
    }

    impl FakeClock {
        fn new(
            walls: impl IntoIterator<Item = f64>,
            monotonic: impl IntoIterator<Item = f64>,
        ) -> Self {
            Self {
                walls: walls.into_iter().collect(),
                monotonic: monotonic.into_iter().collect(),
                sleep_deadlines: Vec::new(),
                on_sleep: None,
            }
        }
    }

    impl AdmissionWaitClock for FakeClock {
        fn wall_seconds(&mut self) -> f64 {
            self.walls.pop_front().expect("wall-clock sample")
        }

        fn monotonic_seconds(&mut self) -> f64 {
            self.monotonic.pop_front().expect("monotonic-clock sample")
        }

        fn sleep_until(&mut self, deadline_seconds: f64) {
            self.sleep_deadlines.push(deadline_seconds);
            if let Some(action) = self.on_sleep.take() {
                action();
            }
        }
    }

    #[test]
    fn terminal_copy_matches_the_observed_reason() {
        assert_eq!(
            AdmissionWaitTerminalReason::ActivityRemains.to_string(),
            ADMISSION_WAIT_TERMINAL_COPY
        );
        assert_eq!(
            AdmissionWaitTerminalReason::ClockDiscontinuity.to_string(),
            ADMISSION_WAIT_UNVERIFIABLE_COPY
        );
    }

    #[test]
    fn live_unverified_records_never_reach_heartbeat_specific_copy() {
        let mismatched = sync::HeartbeatV2::new(
            writer_id(),
            run_id(),
            "peer".to_owned(),
            7,
            "100".to_owned(),
            "test".to_owned(),
            15,
            "/journal".to_owned(),
        );
        for classification in [
            sync::HeartbeatClassification::BoundedMalformed,
            sync::HeartbeatClassification::IdentityMismatch(mismatched),
        ] {
            let peer = sync::SyncPeerObservation {
                source_filename: OsString::from("unverified.check"),
                classification,
                heartbeat: None,
                is_live: true,
            };
            let result = sync::SyncCheckResult {
                snapshot: sync::SyncSnapshot::default(),
                peer_observations: vec![peer.clone()],
                live_peer_observations: vec![peer],
            };
            assert!(matches!(
                reject_unverifiable_live_heartbeat(&result),
                Err(LifecycleError::AdmissionHeartbeatNeedsAttention { .. })
            ));
        }
    }

    #[test]
    fn final_admission_precedes_identity_publication_and_orphan_sweep() {
        let source = include_str!("startup.rs");
        let activate = source
            .split("pub fn activate(self)")
            .nth(1)
            .expect("activate source")
            .split("fn abort_after_publication")
            .next()
            .expect("activate body");
        let publication = activate
            .find("state::write_sync_heartbeat")
            .expect("heartbeat publication");
        let rescan = activate
            .find("sync::scan_bound_sync")
            .expect("post-publication scan");
        let identity = activate
            .find("state::write_supervisor_identity")
            .expect("identity publication");
        let sweep = activate.find("sweep::sweep_orphans").expect("orphan sweep");

        assert!(
            publication < rescan && rescan < identity && identity < sweep,
            "final admission must precede shared identity publication and orphan sweeping"
        );
    }

    #[derive(Clone, Copy)]
    enum MarkerVerdict {
        SameLive,
        NotSameOrExited,
        Unverifiable,
    }

    struct FakeProcessSource {
        current: ProcessInstance,
        marker_verdict: MarkerVerdict,
    }

    impl ProcessInstanceSource for FakeProcessSource {
        fn inspect(&self, pid: u32) -> InspectResult {
            if pid == self.current.pid {
                InspectResult::Present {
                    instance: self.current,
                    uid: 501,
                    execution: ExecutionState::Running,
                    ppid: Some(1),
                    pgid: Some(self.current.pid as i32),
                }
            } else {
                match self.marker_verdict {
                    MarkerVerdict::SameLive => InspectResult::Present {
                        instance: ProcessInstance {
                            pid,
                            birth: ProcessBirth::linux(10, 100, 100),
                        },
                        uid: 501,
                        execution: ExecutionState::Running,
                        ppid: Some(1),
                        pgid: Some(pid as i32),
                    },
                    MarkerVerdict::NotSameOrExited => InspectResult::Absent,
                    MarkerVerdict::Unverifiable => InspectResult::Unverifiable,
                }
            }
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    fn temporary() -> tempfile::TempDir {
        Builder::new()
            .prefix("solstone-admission-wait-")
            .tempdir_in("/var/tmp")
            .expect("temporary journal")
    }

    fn writer_id() -> WriterId {
        WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID")
    }

    fn run_id() -> RunId {
        RunId::parse("fedcba9876543210fedcba9876543210").expect("run ID")
    }

    fn process_source(verdict: MarkerVerdict) -> FakeProcessSource {
        FakeProcessSource {
            current: ProcessInstance {
                pid: std::process::id(),
                birth: ProcessBirth::linux(20, 200, 100),
            },
            marker_verdict: verdict,
        }
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

    fn marker(filename: &str) -> sync::AdmissionWaitMarker {
        let (writer_id, run_id) =
            sync::parse_admission_wait_marker_filename(filename).expect("marker filename");
        sync::AdmissionWaitMarker::new(
            writer_id,
            run_id,
            ProcessInstance {
                pid: 42,
                birth: ProcessBirth::linux(10, 100, 100),
            },
            sync::AdmissionWaitReason::FreshNonSelfHeartbeat,
        )
    }

    fn write_fresh_foreign_heartbeat(root: &Path, wall_seconds: i64) -> PathBuf {
        let path = root.join("health/sync/foreign.check");
        fs::create_dir_all(path.parent().expect("sync parent")).expect("sync directory");
        let heartbeat = sync::Heartbeat {
            schema: sync::HEARTBEAT_SCHEMA_V1,
            machine_id: "legacy".to_owned(),
            hostname: "foreign".to_owned(),
            pid: 7,
            wall_time: wall_seconds.to_string(),
            solstone_version: "test".to_owned(),
            interval_seconds: sync::DEFAULT_INTERVAL_SECONDS as u32,
            journal_path: root.display().to_string(),
        };
        fs::write(
            &path,
            serde_json::to_vec(&heartbeat).expect("heartbeat JSON"),
        )
        .expect("foreign heartbeat");
        set_file_mtime(&path, FileTime::from_unix_time(wall_seconds, 0)).expect("fixture mtime");
        path
    }

    fn write_fresh_v2_heartbeat(
        root: &Path,
        writer_id: WriterId,
        run_id: RunId,
        wall_seconds: i64,
    ) -> PathBuf {
        let filename = sync::v2_heartbeat_filename(&writer_id, &run_id);
        let path = root.join("health/sync").join(filename);
        fs::create_dir_all(path.parent().expect("sync parent")).expect("sync directory");
        let heartbeat = sync::HeartbeatV2::new(
            writer_id,
            run_id,
            "foreign".to_owned(),
            7,
            wall_seconds.to_string(),
            "test".to_owned(),
            sync::DEFAULT_INTERVAL_SECONDS as u32,
            root.display().to_string(),
        );
        fs::write(
            &path,
            serde_json::to_vec(&heartbeat).expect("v2 heartbeat JSON"),
        )
        .expect("foreign v2 heartbeat");
        set_file_mtime(&path, FileTime::from_unix_time(wall_seconds, 0)).expect("fixture mtime");
        path
    }

    #[test]
    fn marker_reading_only_removes_not_same_or_exited_markers() {
        let temporary = temporary();
        let sync_directory = bound_sync(temporary.path());
        let filename = sync::admission_wait_marker_filename(&writer_id(), &run_id());
        let body = serde_json::to_vec(&marker(&filename)).expect("marker JSON");

        for verdict in [
            MarkerVerdict::SameLive,
            MarkerVerdict::Unverifiable,
            MarkerVerdict::NotSameOrExited,
        ] {
            let path = temporary.path().join("health/sync").join(&filename);
            fs::write(&path, &body).expect("marker fixture");
            let result = sync::scan_bound_sync(&sync_directory, "self.check", None, 100.0)
                .expect("complete scan");
            let outcome = remove_stale_wait_markers(
                &sync_directory,
                &result,
                &writer_id(),
                100.0,
                &process_source(verdict),
            );
            match verdict {
                MarkerVerdict::SameLive => {
                    assert!(matches!(
                        outcome,
                        Err(LifecycleError::AdmissionWaitMarkerLive)
                    ));
                    assert!(path.exists());
                }
                MarkerVerdict::Unverifiable => {
                    assert!(matches!(
                        outcome,
                        Err(LifecycleError::AdmissionWaitMarkerNeedsAttention(
                            AdmissionWaitMarkerProblem::UnverifiableProcess { .. }
                        ))
                    ));
                    assert!(path.exists());
                }
                MarkerVerdict::NotSameOrExited => {
                    assert!(outcome.is_ok());
                    assert!(!path.exists());
                }
            }
            if path.exists() {
                fs::remove_file(path).expect("test cleanup");
            }
        }
    }

    #[test]
    fn foreign_wait_markers_use_age_instead_of_the_local_process_table() {
        let temporary = temporary();
        let sync_directory = bound_sync(temporary.path());
        let foreign_writer =
            WriterId::parse("11111111111111111111111111111111").expect("foreign writer ID");
        let filename = sync::admission_wait_marker_filename(&foreign_writer, &run_id());
        let path = temporary.path().join("health/sync").join(&filename);
        fs::write(
            &path,
            serde_json::to_vec(&marker(&filename)).expect("marker JSON"),
        )
        .expect("foreign marker fixture");
        set_file_mtime(&path, FileTime::from_unix_time(100, 0)).expect("fresh marker mtime");
        let fresh = sync::scan_bound_sync(&sync_directory, "self.check", None, 100.0)
            .expect("fresh marker scan");

        assert!(matches!(
            remove_stale_wait_markers(
                &sync_directory,
                &fresh,
                &writer_id(),
                100.0,
                &process_source(MarkerVerdict::NotSameOrExited),
            ),
            Err(LifecycleError::AdmissionWaitMarkerLive)
        ));
        assert!(
            path.exists(),
            "a foreign marker cannot be deleted from a local-process miss"
        );

        set_file_mtime(&path, FileTime::from_unix_time(39, 0)).expect("stale marker mtime");
        let stale = sync::scan_bound_sync(&sync_directory, "self.check", None, 100.0)
            .expect("stale marker scan");
        remove_stale_wait_markers(
            &sync_directory,
            &stale,
            &writer_id(),
            100.0,
            &process_source(MarkerVerdict::Unverifiable),
        )
        .expect("proven-old foreign marker cleanup");
        assert!(!path.exists());
    }

    #[test]
    fn malformed_marker_needs_attention_without_removal() {
        let temporary = temporary();
        let sync_directory = bound_sync(temporary.path());
        let filename = sync::admission_wait_marker_filename(&writer_id(), &run_id());
        let path = temporary.path().join("health/sync").join(&filename);
        fs::write(&path, b"not marker JSON").expect("malformed marker fixture");
        let result =
            sync::scan_bound_sync(&sync_directory, "self.check", None, 100.0).expect("scan");

        assert!(matches!(
            remove_stale_wait_markers(
                &sync_directory,
                &result,
                &writer_id(),
                100.0,
                &process_source(MarkerVerdict::NotSameOrExited),
            ),
            Err(LifecycleError::AdmissionWaitMarkerNeedsAttention(
                AdmissionWaitMarkerProblem::Malformed { .. }
            ))
        ));
        assert!(path.exists());
    }

    #[test]
    fn malformed_marker_dominates_an_earlier_live_marker() {
        let temporary = temporary();
        let sync_directory = bound_sync(temporary.path());
        let live_filename = sync::admission_wait_marker_filename(&writer_id(), &run_id());
        fs::write(
            temporary.path().join("health/sync").join(&live_filename),
            serde_json::to_vec(&marker(&live_filename)).expect("live marker JSON"),
        )
        .expect("live marker fixture");
        let malformed_filename = "solstone-wait-v2-zzzz.check";
        let malformed_path = temporary
            .path()
            .join("health/sync")
            .join(malformed_filename);
        fs::write(&malformed_path, b"not marker JSON").expect("malformed marker fixture");
        let result = sync::scan_bound_sync(&sync_directory, "self.check", None, 100.0)
            .expect("complete scan");

        assert!(matches!(
            remove_stale_wait_markers(
                &sync_directory,
                &result,
                &writer_id(),
                100.0,
                &process_source(MarkerVerdict::SameLive),
            ),
            Err(LifecycleError::AdmissionWaitMarkerNeedsAttention(
                AdmissionWaitMarkerProblem::Malformed { .. }
            ))
        ));
        assert!(malformed_path.exists());
    }

    #[test]
    fn admission_wait_sleeps_once_at_the_freshness_cap_and_removes_its_marker() {
        let temporary = temporary();
        write_fresh_foreign_heartbeat(temporary.path(), 1_000);
        let mut clock = FakeClock::new([1_000.0, 1_060.1], [10.0, 70.0]);
        let mut emitted = Vec::new();
        let source = process_source(MarkerVerdict::NotSameOrExited);

        let admission = SupervisorBootAdmission::acquire_with(
            temporary.path(),
            writer_id(),
            &mut clock,
            &source,
            &mut |copy| emitted.push(copy),
        )
        .expect("admission after heartbeat becomes stale");

        assert_eq!(clock.sleep_deadlines, vec![70.0]);
        assert_eq!(emitted, vec![ADMISSION_WAIT_TRANSIENT_COPY]);
        assert!(
            fs::read_dir(temporary.path().join("health/sync"))
                .expect("sync")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with("solstone-wait-v2-"))
        );
        drop(admission);
    }

    #[test]
    fn final_admission_refuses_an_interleaved_writer_before_shared_startup_mutations() {
        let temporary = temporary();
        let mut clock = FakeClock::new([100.0], []);
        let source = process_source(MarkerVerdict::NotSameOrExited);
        let admission = SupervisorBootAdmission::acquire_with(
            temporary.path(),
            writer_id(),
            &mut clock,
            &source,
            &mut |_| {},
        )
        .expect("empty first scan admits pre-publication");
        let self_filename = admission.heartbeat_filename.clone();
        fs::write(
            temporary.path().join("health/supervisor.pid"),
            b"winner pid",
        )
        .expect("winner pid fixture");
        fs::write(
            temporary.path().join("health/supervisor.start_time"),
            b"winner start time",
        )
        .expect("winner start-time fixture");
        fs::write(
            temporary.path().join("health/callosum.sock"),
            b"winner socket",
        )
        .expect("winner Callosum fixture");
        let foreign = write_fresh_v2_heartbeat(
            temporary.path(),
            WriterId::parse("11111111111111111111111111111111").expect("foreign writer ID"),
            RunId::parse("22222222222222222222222222222222").expect("foreign run ID"),
            100,
        );

        assert!(matches!(
            admission.activate(),
            Err(LifecycleError::AdmissionWaitTerminal(
                AdmissionWaitTerminalReason::ActivityRemains
            ))
        ));
        assert!(foreign.exists(), "the interleaved writer is never removed");
        assert!(
            !temporary
                .path()
                .join("health/sync")
                .join(self_filename)
                .exists(),
            "the refused run removes only its retained self heartbeat"
        );
        assert!(!temporary.path().join("health/supervisor.ready").exists());
        assert_eq!(
            fs::read(temporary.path().join("health/supervisor.pid")).expect("winner pid"),
            b"winner pid"
        );
        assert_eq!(
            fs::read(temporary.path().join("health/supervisor.start_time"))
                .expect("winner start time"),
            b"winner start time"
        );
        assert_eq!(
            fs::read(temporary.path().join("health/callosum.sock")).expect("winner Callosum"),
            b"winner socket"
        );
    }

    #[test]
    fn post_publication_snapshot_seeds_the_first_runtime_conflict_comparison() {
        let temporary = temporary();
        let mut clock = FakeClock::new([100.0], []);
        let source = process_source(MarkerVerdict::NotSameOrExited);
        let mut lifecycle = SupervisorBootAdmission::acquire_with(
            temporary.path(),
            writer_id(),
            &mut clock,
            &source,
            &mut |_| {},
        )
        .expect("empty first scan")
        .activate()
        .expect("activate")
        .into_lifecycle();
        assert!(lifecycle.last_completed_sync_result().is_some());

        write_fresh_v2_heartbeat(
            temporary.path(),
            WriterId::parse("33333333333333333333333333333333").expect("foreign writer ID"),
            RunId::parse("44444444444444444444444444444444").expect("foreign run ID"),
            101,
        );
        let mut tick_clock = FakeClock::new([101.0], [1.0]);
        assert!(matches!(
            lifecycle.tick_sync_with(None, &mut tick_clock),
            super::super::SyncTickOutcome::Conflict(_)
        ));
    }

    #[test]
    fn post_publication_scan_refuses_a_malformed_wait_marker_and_cleans_self() {
        let temporary = temporary();
        let mut clock = FakeClock::new([100.0], []);
        let source = process_source(MarkerVerdict::NotSameOrExited);
        let admission = SupervisorBootAdmission::acquire_with(
            temporary.path(),
            writer_id(),
            &mut clock,
            &source,
            &mut |_| {},
        )
        .expect("empty first scan admits pre-publication");
        let self_filename = admission.heartbeat_filename.clone();
        fs::write(
            temporary
                .path()
                .join("health/sync/solstone-wait-v2-zzzz.check"),
            b"not marker JSON",
        )
        .expect("malformed marker fixture");

        assert!(matches!(
            admission.activate(),
            Err(LifecycleError::AdmissionWaitMarkerNeedsAttention(
                AdmissionWaitMarkerProblem::Malformed { .. }
            ))
        ));
        assert!(
            !temporary
                .path()
                .join("health/sync")
                .join(self_filename)
                .exists()
        );
        assert!(!temporary.path().join("health/supervisor.pid").exists());
        assert!(
            !temporary
                .path()
                .join("health/supervisor.start_time")
                .exists()
        );
        assert!(!temporary.path().join("health/supervisor.ready").exists());
    }

    #[test]
    fn post_publication_scan_refuses_a_live_wait_marker_and_cleans_self() {
        let temporary = temporary();
        let mut clock = FakeClock::new([100.0], []);
        let source = process_source(MarkerVerdict::NotSameOrExited);
        let admission = SupervisorBootAdmission::acquire_with(
            temporary.path(),
            writer_id(),
            &mut clock,
            &source,
            &mut |_| {},
        )
        .expect("empty first scan admits pre-publication");
        let self_filename = admission.heartbeat_filename.clone();
        let marker_writer =
            WriterId::parse("11111111111111111111111111111111").expect("marker writer");
        let marker_run = RunId::parse("22222222222222222222222222222222").expect("marker run");
        let marker_filename = sync::admission_wait_marker_filename(&marker_writer, &marker_run);
        let marker = sync::AdmissionWaitMarker::new(
            marker_writer,
            marker_run,
            current_process_instance(&SystemProcessInstanceSource).expect("current process"),
            sync::AdmissionWaitReason::FreshNonSelfHeartbeat,
        );
        fs::write(
            temporary.path().join("health/sync").join(marker_filename),
            serde_json::to_vec(&marker).expect("marker JSON"),
        )
        .expect("live marker fixture");

        assert!(matches!(
            admission.activate(),
            Err(LifecycleError::AdmissionWaitMarkerLive)
        ));
        assert!(
            !temporary
                .path()
                .join("health/sync")
                .join(self_filename)
                .exists()
        );
        assert!(!temporary.path().join("health/supervisor.pid").exists());
        assert!(
            !temporary
                .path()
                .join("health/supervisor.start_time")
                .exists()
        );
        assert!(!temporary.path().join("health/supervisor.ready").exists());
    }

    #[test]
    fn publication_barrier_allows_one_writer_and_refuses_the_later_writer() {
        let temporary = temporary();
        let first_sync = bound_sync(temporary.path());
        let second_sync = bound_sync(temporary.path());
        let first_writer = writer_id();
        let first_run = run_id();
        let second_writer =
            WriterId::parse("11111111111111111111111111111111").expect("second writer");
        let second_run = RunId::parse("22222222222222222222222222222222").expect("second run");
        let first_filename = sync::v2_heartbeat_filename(&first_writer, &first_run);
        let second_filename = sync::v2_heartbeat_filename(&second_writer, &second_run);
        let now = super::epoch_seconds();
        let first_snapshot = sync::scan_bound_sync(&first_sync, &first_filename, None, now)
            .expect("first admission snapshot")
            .snapshot;
        let second_snapshot = sync::scan_bound_sync(&second_sync, &second_filename, None, now)
            .expect("second admission snapshot")
            .snapshot;

        let first_body = serde_json::to_vec(&sync::HeartbeatV2::new(
            first_writer,
            first_run,
            "first".to_owned(),
            1,
            now.to_string(),
            "test".to_owned(),
            sync::DEFAULT_INTERVAL_SECONDS as u32,
            temporary.path().display().to_string(),
        ))
        .expect("first heartbeat JSON");
        state::write_sync_heartbeat(&first_sync, &first_filename, &first_body)
            .expect("first publication");
        let first_result =
            sync::scan_bound_sync(&first_sync, &first_filename, Some(&first_snapshot), now)
                .expect("first publication scan");
        assert!(!first_result.is_boot_conflict());

        let second_body = serde_json::to_vec(&sync::HeartbeatV2::new(
            second_writer,
            second_run,
            "second".to_owned(),
            2,
            now.to_string(),
            "test".to_owned(),
            sync::DEFAULT_INTERVAL_SECONDS as u32,
            temporary.path().display().to_string(),
        ))
        .expect("second heartbeat JSON");
        state::write_sync_heartbeat(&second_sync, &second_filename, &second_body)
            .expect("second publication");
        let second_result =
            sync::scan_bound_sync(&second_sync, &second_filename, Some(&second_snapshot), now)
                .expect("second publication scan");
        assert!(second_result.is_boot_conflict());
    }

    #[test]
    fn publication_barrier_refuses_both_when_both_publish_before_scanning() {
        let temporary = temporary();
        let first_sync = bound_sync(temporary.path());
        let second_sync = bound_sync(temporary.path());
        let first_writer = writer_id();
        let first_run = run_id();
        let second_writer =
            WriterId::parse("11111111111111111111111111111111").expect("second writer");
        let second_run = RunId::parse("22222222222222222222222222222222").expect("second run");
        let first_filename = sync::v2_heartbeat_filename(&first_writer, &first_run);
        let second_filename = sync::v2_heartbeat_filename(&second_writer, &second_run);
        let now = super::epoch_seconds();
        let first_snapshot = sync::scan_bound_sync(&first_sync, &first_filename, None, now)
            .expect("first admission snapshot")
            .snapshot;
        let second_snapshot = sync::scan_bound_sync(&second_sync, &second_filename, None, now)
            .expect("second admission snapshot")
            .snapshot;
        let first_body = serde_json::to_vec(&sync::HeartbeatV2::new(
            first_writer,
            first_run,
            "first".to_owned(),
            1,
            now.to_string(),
            "test".to_owned(),
            sync::DEFAULT_INTERVAL_SECONDS as u32,
            temporary.path().display().to_string(),
        ))
        .expect("first heartbeat JSON");
        let second_body = serde_json::to_vec(&sync::HeartbeatV2::new(
            second_writer,
            second_run,
            "second".to_owned(),
            2,
            now.to_string(),
            "test".to_owned(),
            sync::DEFAULT_INTERVAL_SECONDS as u32,
            temporary.path().display().to_string(),
        ))
        .expect("second heartbeat JSON");
        state::write_sync_heartbeat(&first_sync, &first_filename, &first_body)
            .expect("first publication");
        state::write_sync_heartbeat(&second_sync, &second_filename, &second_body)
            .expect("second publication");

        let first_result =
            sync::scan_bound_sync(&first_sync, &first_filename, Some(&first_snapshot), now)
                .expect("first publication scan");
        let second_result =
            sync::scan_bound_sync(&second_sync, &second_filename, Some(&second_snapshot), now)
                .expect("second publication scan");
        assert!(first_result.is_boot_conflict());
        assert!(second_result.is_boot_conflict());
    }

    #[test]
    fn pre_ready_abort_never_removes_replaced_identity_artifacts() {
        let temporary = temporary();
        let mut clock = FakeClock::new([100.0], []);
        let source = process_source(MarkerVerdict::NotSameOrExited);
        let lifecycle = SupervisorBootAdmission::acquire_with(
            temporary.path(),
            writer_id(),
            &mut clock,
            &source,
            &mut |_| {},
        )
        .expect("admission")
        .activate()
        .expect("identity")
        .into_lifecycle();
        fs::write(temporary.path().join("health/supervisor.pid"), b"424242")
            .expect("replacement pid");
        fs::write(
            temporary.path().join("health/supervisor.start_time"),
            b"987654.25",
        )
        .expect("replacement start time");

        assert!(matches!(
            lifecycle.abort_before_ready(),
            Err(LifecycleError::LifecycleArtifactCleanup { .. })
        ));
        assert_eq!(
            fs::read(temporary.path().join("health/supervisor.pid")).expect("winner pid"),
            b"424242"
        );
        assert_eq!(
            fs::read(temporary.path().join("health/supervisor.start_time"))
                .expect("winner start time"),
            b"987654.25"
        );
        assert!(
            !temporary
                .path()
                .join("health/sync")
                .join(lifecycle.heartbeat_filename())
                .exists(),
            "the exact self heartbeat is still removed"
        );
    }

    #[test]
    fn pre_ready_abort_attempts_all_exact_cleanups_and_preserves_replacements() {
        let temporary = temporary();
        let mut clock = FakeClock::new([100.0], []);
        let source = process_source(MarkerVerdict::NotSameOrExited);
        let mut lifecycle = SupervisorBootAdmission::acquire_with(
            temporary.path(),
            writer_id(),
            &mut clock,
            &source,
            &mut |_| {},
        )
        .expect("admission")
        .activate()
        .expect("identity")
        .into_lifecycle();
        lifecycle
            .signal_ready(100.0, serde_json::Map::new())
            .expect("readiness");
        fs::write(
            temporary.path().join("health/supervisor.ready"),
            b"winner readiness",
        )
        .expect("replacement readiness");
        fs::write(temporary.path().join("health/supervisor.pid"), b"424242")
            .expect("replacement pid");
        fs::write(
            temporary.path().join("health/supervisor.start_time"),
            b"987654.25",
        )
        .expect("replacement start time");

        assert!(matches!(
            lifecycle.abort_before_ready(),
            Err(LifecycleError::LifecycleArtifactCleanup { .. })
        ));
        assert_eq!(
            fs::read(temporary.path().join("health/supervisor.ready")).expect("winner readiness"),
            b"winner readiness"
        );
        assert_eq!(
            fs::read(temporary.path().join("health/supervisor.pid")).expect("winner pid"),
            b"424242"
        );
        assert_eq!(
            fs::read(temporary.path().join("health/supervisor.start_time"))
                .expect("winner start time"),
            b"987654.25"
        );
    }

    #[test]
    fn wall_clock_discontinuity_refuses_after_the_one_monotonic_wait() {
        let temporary = temporary();
        write_fresh_foreign_heartbeat(temporary.path(), 100);
        let mut clock = FakeClock::new([100.0, 99.0], [10.0, 70.0]);
        let source = process_source(MarkerVerdict::NotSameOrExited);

        assert!(matches!(
            SupervisorBootAdmission::acquire_with(
                temporary.path(),
                writer_id(),
                &mut clock,
                &source,
                &mut |_| {},
            ),
            Err(LifecycleError::AdmissionWaitTerminal(
                AdmissionWaitTerminalReason::ClockDiscontinuity
            ))
        ));
        assert_eq!(clock.sleep_deadlines, vec![70.0]);
    }

    #[test]
    fn forward_wall_clock_discontinuity_does_not_change_the_monotonic_wait() {
        let temporary = temporary();
        write_fresh_foreign_heartbeat(temporary.path(), 100);
        let mut clock = FakeClock::new([100.0, 176.0], [10.0, 70.0]);
        let source = process_source(MarkerVerdict::NotSameOrExited);

        assert!(matches!(
            SupervisorBootAdmission::acquire_with(
                temporary.path(),
                writer_id(),
                &mut clock,
                &source,
                &mut |_| {},
            ),
            Err(LifecycleError::AdmissionWaitTerminal(
                AdmissionWaitTerminalReason::ClockDiscontinuity
            ))
        ));
        assert_eq!(clock.sleep_deadlines, vec![70.0]);
    }

    #[test]
    fn renewal_during_the_one_wait_is_caught_by_the_single_rescan() {
        let temporary = temporary();
        let foreign = write_fresh_foreign_heartbeat(temporary.path(), 100);
        let renewed = sync::Heartbeat {
            schema: sync::HEARTBEAT_SCHEMA_V1,
            machine_id: "legacy".to_owned(),
            hostname: "foreign".to_owned(),
            pid: 7,
            wall_time: "160".to_owned(),
            solstone_version: "test".to_owned(),
            interval_seconds: sync::DEFAULT_INTERVAL_SECONDS as u32,
            journal_path: temporary.path().display().to_string(),
        };
        let renewed = serde_json::to_vec(&renewed).expect("renewed heartbeat JSON");
        let mut clock = FakeClock::new([100.0, 161.0], [10.0, 70.0]);
        clock.on_sleep = Some(Box::new(move || {
            fs::write(&foreign, renewed).expect("renew heartbeat");
            set_file_mtime(&foreign, FileTime::from_unix_time(160, 0))
                .expect("renewed fixture mtime");
        }));
        let source = process_source(MarkerVerdict::NotSameOrExited);

        assert!(matches!(
            SupervisorBootAdmission::acquire_with(
                temporary.path(),
                writer_id(),
                &mut clock,
                &source,
                &mut |_| {},
            ),
            Err(LifecycleError::AdmissionWaitTerminal(
                AdmissionWaitTerminalReason::ActivityRemains
            ))
        ));
        assert_eq!(clock.sleep_deadlines, vec![70.0]);
    }
}
