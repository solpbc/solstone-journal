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

pub const ADMISSION_WAIT_TRANSIENT_COPY: &str = "a recent heartbeat from another run is present.\nsolstone is waiting to protect your journal. it should clear on its own shortly.";
pub const ADMISSION_WAIT_TERMINAL_COPY: &str = "a recent heartbeat from another run is present.\nto protect your journal, solstone did not start while it is present.\nwait a moment, then try again.";
pub const ADMISSION_WAIT_UNVERIFIABLE_COPY: &str =
    "startup status couldn't be verified.\nwait a moment, then try again.";

/// Time and sleep dependencies used by the bounded admission wait.
///
/// The monotonic deadline makes the one wait independent of wall-clock changes.
pub trait AdmissionWaitClock {
    fn wall_seconds(&mut self) -> f64;
    fn monotonic_seconds(&mut self) -> f64;
    fn sleep_until(&mut self, deadline_seconds: f64);
}

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
        formatter.write_str(ADMISSION_WAIT_TERMINAL_COPY)
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
    now: f64,
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

        remove_stale_wait_markers(&sync, &result, process_source)?;
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
            now: second_wall_seconds,
        })
    }

    pub fn activate(self) -> Result<PreReadySupervisorLifecycle, LifecycleError> {
        state::write_supervisor_identity(&self.journal, std::process::id())?;
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
            now: self.now,
            last_orphan_sweep,
        })
    }
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
    process_source: &dyn ProcessInstanceSource,
) -> Result<(), LifecycleError> {
    let mut stale_markers = Vec::new();
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
                match process_source.observe(&marker.process) {
                    InstanceVerdict::Unverifiable => {
                        return Err(LifecycleError::AdmissionWaitMarkerNeedsAttention(
                            AdmissionWaitMarkerProblem::UnverifiableProcess {
                                filename: peer.source_filename.clone(),
                            },
                        ));
                    }
                    InstanceVerdict::SameLive { .. } => {
                        return Err(LifecycleError::AdmissionWaitMarkerLive);
                    }
                    InstanceVerdict::NotSameOrExited => {
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
                        stale_markers.push((peer.source_filename.clone(), observation));
                    }
                }
            }
            _ => {}
        }
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

pub(crate) fn wall_clock_discontinuous(
    first_wall_seconds: f64,
    second_wall_seconds: f64,
    first_monotonic_seconds: f64,
    second_monotonic_seconds: f64,
) -> bool {
    let wall_elapsed = second_wall_seconds - first_wall_seconds;
    let monotonic_elapsed = second_monotonic_seconds - first_monotonic_seconds;
    wall_elapsed.is_sign_negative()
        || monotonic_elapsed.is_sign_negative()
        || wall_elapsed > monotonic_elapsed + sync::DEFAULT_INTERVAL_SECONDS
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

    pub fn publish_heartbeat(self) -> Result<SupervisorLifecycle, LifecycleError> {
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
        let heartbeat_bytes = match serde_json::to_vec(&heartbeat) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = self.abort_pre_ready();
                return Err(error.into());
            }
        };
        let retained_self_heartbeat = match state::write_sync_heartbeat(
            &self.sync,
            &self.heartbeat_filename,
            &heartbeat_bytes,
        ) {
            Ok(observation) => observation,
            Err(error) => {
                let _ = self.abort_pre_ready();
                return Err(error.into());
            }
        };
        Ok(SupervisorLifecycle {
            journal: self.journal,
            writer_id: self.writer_id,
            run_id: self.run_id,
            heartbeat_filename: self.heartbeat_filename,
            last_orphan_sweep: self.last_orphan_sweep,
            _journal_root: self.root,
            _health: self.health,
            sync: self.sync,
            _lease: self.lease,
            retained_self_heartbeat: Some(retained_self_heartbeat),
            last_completed_sync_result: None,
            stale_heartbeat_gc: super::StaleHeartbeatGc::default(),
        })
    }

    pub fn abort_pre_ready(self) -> Result<(), LifecycleError> {
        state::clear_ready(&self.journal)?;
        state::clear_supervisor_identity(&self.journal)
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
            let outcome =
                remove_stale_wait_markers(&sync_directory, &result, &process_source(verdict));
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
                &process_source(MarkerVerdict::NotSameOrExited),
            ),
            Err(LifecycleError::AdmissionWaitMarkerNeedsAttention(
                AdmissionWaitMarkerProblem::Malformed { .. }
            ))
        ));
        assert!(path.exists());
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
        let mut clock = FakeClock::new([100.0, 161.0], [10.0, 70.0]);
        clock.on_sleep = Some(Box::new(move || {
            fs::write(&foreign, b"renewed").expect("renew heartbeat");
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
