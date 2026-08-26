// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Supervisor lifecycle primitives. This module owns `health/` operational
//! state but deliberately does not provide a supervisor binary or CLI.

mod parent;
mod readiness;
mod shutdown;
mod startup;
mod state;
mod sweep;
mod sync;

use std::path::{Path, PathBuf};

use solstone_core_journal_io::{
    BoundParentLock, ExistingParentLockError, FileObservation, FlatDirectory, JournalRoot,
};
use thiserror::Error;

pub use parent::{
    DeclaredParent, ParentAdmissionFailure, ParentLossReason, ParentWatch, ParentWatchStatus,
};
pub use readiness::{ReadinessMarker, START_TIME_TOLERANCE_SECONDS};
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use readiness::{readiness_is_valid, wait_ready, wait_ready_with};
pub use shutdown::{
    ShutdownDisposition, ShutdownDriver, ShutdownPhase, ShutdownRegime, ShutdownReport, shutdown,
};
pub use startup::{PreReadySupervisorLifecycle, SupervisorBootAdmission};
pub use state::{
    HeartbeatWriteError, SelfHeartbeatRemoval, append_supervisor_log,
    clear_ready as clear_readiness, clear_self_heartbeat, compact_log_if_oversized,
    recorded_supervisor_pid, write_sync_heartbeat,
};
pub use sweep::{OrphanSweepOutcome, OrphanSweepReport, sweep_orphans};
pub use sync::{
    DEFAULT_INTERVAL_SECONDS, FRESH_WINDOW_MULTIPLIER, HEARTBEAT_SCHEMA_V1, Heartbeat,
    HeartbeatClassification, MAX_SYNC_DIRECTORY_ENTRIES, MAX_SYNC_HEARTBEAT_BYTES, SyncCheckResult,
    SyncConflictEvent, SyncDirectoryOperation, SyncIncompleteSnapshotReason, SyncPeerObservation,
    SyncReadOperation, SyncRescan, SyncScanFailure, SyncSnapshot, SyncUnsafeReason,
    format_conflict_message, machine_id, rescan_sync_read_only, sanitize_hostname,
    sync_conflict_event,
};

/// Result of a supervisor heartbeat renewal and complete peer scan.
#[derive(Debug)]
pub enum SyncTickOutcome {
    Healthy,
    Conflict(Box<SyncCheckResult>),
    RenewalFailure(HeartbeatWriteError),
    CompleteScanFailure(SyncScanFailure),
    RetainedObservationFailure(HeartbeatWriteError),
}

/// Lifecycle failures that are meaningful to a supervisor host.
#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error("supervisor already running")]
    AlreadyRunning,
    #[error("lifecycle I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("lifecycle system call failed: {0}")]
    Nix(#[from] nix::errno::Errno),
    #[error("lifecycle JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid supervisor identity: {0}")]
    Identity(&'static str),
    #[error("invalid heartbeat filename")]
    InvalidHeartbeatFilename,
    #[error("sync scan failed: {0}")]
    SyncScan(#[source] Box<SyncScanFailure>),
    #[error("could not acquire bound supervisor lock: {0}")]
    SupervisorLock(#[source] ExistingParentLockError),
    #[error("heartbeat publication or retention failed: {0}")]
    HeartbeatWrite(#[from] HeartbeatWriteError),
    #[error("another solstone writer is active on this journal")]
    SyncConflict(Box<SyncCheckResult>),
}

/// Held singleton admission and retained descriptor capabilities.
pub struct SupervisorLifecycle {
    journal: PathBuf,
    heartbeat_filename: String,
    last_orphan_sweep: OrphanSweepOutcome,
    _journal_root: JournalRoot,
    _health: FlatDirectory,
    sync: FlatDirectory,
    _lease: BoundParentLock,
    retained_self_heartbeat: Option<FileObservation>,
    last_completed_sync_result: Option<SyncCheckResult>,
}

/// Enter supervisor lifecycle ownership and retain the singleton lease.
pub fn boot(journal: impl AsRef<Path>) -> Result<SupervisorLifecycle, LifecycleError> {
    SupervisorLifecycle::boot(journal)
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
    pub fn boot(journal: impl AsRef<Path>) -> Result<Self, LifecycleError> {
        SupervisorBootAdmission::acquire(journal)?
            .activate()?
            .publish_heartbeat()
    }

    /// iOS and other unsupported targets have no supported process-start-time
    /// identity reader.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn boot(_journal: impl AsRef<Path>) -> Result<Self, LifecycleError> {
        Err(LifecycleError::Identity(
            "supervisor lifecycle is unsupported on this platform",
        ))
    }

    pub fn signal_ready(
        &self,
        ready_at: f64,
        extra: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), LifecycleError> {
        state::write_readiness(&self.journal, ready_at, extra)?;
        sd_notify("READY=1");
        Ok(())
    }

    pub fn clear_ready(&self) -> Result<(), LifecycleError> {
        state::clear_ready(&self.journal)
    }

    pub fn last_orphan_sweep(&self) -> &OrphanSweepOutcome {
        &self.last_orphan_sweep
    }

    /// Publish and retain the self heartbeat, then complete a peer scan.
    ///
    /// A completed result is retained only for `Healthy` and `Conflict`, so
    /// callers cannot mistake a failed scan for a new snapshot.
    pub fn tick_sync(&mut self, previous: Option<&SyncSnapshot>, now: f64) -> SyncTickOutcome {
        let current_machine_id = sync::machine_id();
        let heartbeat = sync::Heartbeat {
            schema: sync::HEARTBEAT_SCHEMA_V1,
            machine_id: current_machine_id.clone(),
            hostname: hostname(),
            pid: std::process::id(),
            wall_time: now.to_string(),
            solstone_version: env!("CARGO_PKG_VERSION").to_owned(),
            interval_seconds: sync::DEFAULT_INTERVAL_SECONDS as u32,
            journal_path: self.journal.display().to_string(),
        };
        let body = serde_json::to_vec(&heartbeat).expect("heartbeat serializes");
        let observation =
            match state::write_sync_heartbeat(&self.sync, &self.heartbeat_filename, &body) {
                Ok(observation) => observation,
                Err(
                    error @ (HeartbeatWriteError::Publish { .. }
                    | HeartbeatWriteError::DurabilityUncertain { .. }),
                ) => {
                    return SyncTickOutcome::RenewalFailure(error);
                }
                Err(
                    error @ (HeartbeatWriteError::ObservationMissing
                    | HeartbeatWriteError::Observation { .. }
                    | HeartbeatWriteError::ObservationBytesMismatched),
                ) => {
                    return SyncTickOutcome::RetainedObservationFailure(error);
                }
                Err(HeartbeatWriteError::InvalidFilename) => {
                    return SyncTickOutcome::RenewalFailure(HeartbeatWriteError::InvalidFilename);
                }
            };
        self.retained_self_heartbeat = Some(observation);

        let result = match sync::scan_bound_sync(
            &self.sync,
            &self.heartbeat_filename,
            &current_machine_id,
            previous,
            now,
        ) {
            Ok(result) => result,
            Err(error) => return SyncTickOutcome::CompleteScanFailure(error),
        };
        let conflict = result.is_tick_conflict(previous);
        self.last_completed_sync_result = Some(result.clone());
        if conflict {
            SyncTickOutcome::Conflict(Box::new(result))
        } else {
            SyncTickOutcome::Healthy
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
    ) -> Result<ShutdownReport, LifecycleError> {
        if let Err(error) = state::clear_ready(&self.journal) {
            eprintln!("supervisor readiness cleanup failed: {error}");
        }
        if !sync_conflict {
            match state::clear_self_heartbeat(
                &self.sync,
                &self.heartbeat_filename,
                self.retained_self_heartbeat.as_ref(),
            ) {
                Ok(SelfHeartbeatRemoval::Removed | SelfHeartbeatRemoval::NoCleanupAuthority) => {
                    if let Err(error) = state::clear_supervisor_identity(&self.journal) {
                        eprintln!("supervisor identity cleanup failed: {error}");
                    }
                }
                Ok(outcome) => {
                    eprintln!("supervisor heartbeat cleanup did not complete cleanly: {outcome}");
                }
                Err(error) => {
                    eprintln!("supervisor heartbeat cleanup failed: {error}");
                }
            }
        }
        Ok(shutdown::shutdown(driver, regime))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn hostname() -> String {
    let raw = nix::unistd::gethostname().unwrap_or_default();
    sync::sanitize_hostname(&raw.to_string_lossy())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn self_heartbeat_filename() -> String {
    format!("{}.check", hostname())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |value| value.as_secs_f64())
}

/// Preserve Python's conservative pid-file status contract.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn is_supervisor_up(journal: impl AsRef<Path>) -> bool {
    is_supervisor_up_with_start_time(journal, state::process_start_time_epoch_seconds)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn is_supervisor_up(_journal: impl AsRef<Path>) -> bool {
    false
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn is_supervisor_up_with_start_time(
    journal: impl AsRef<Path>,
    process_start_time: impl Fn(u32) -> Result<f64, LifecycleError>,
) -> bool {
    let health = journal.as_ref().join("health");
    let Ok(pid) = std::fs::read_to_string(health.join("supervisor.pid")).and_then(|text| {
        text.trim()
            .parse::<u32>()
            .map_err(|_| std::io::Error::other("invalid pid"))
    }) else {
        return false;
    };
    let Ok(recorded) =
        std::fs::read_to_string(health.join("supervisor.start_time")).and_then(|text| {
            text.trim()
                .parse::<f64>()
                .map_err(|_| std::io::Error::other("invalid start time"))
        })
    else {
        return false;
    };
    let Ok(actual) = process_start_time(pid) else {
        return false;
    };
    (recorded - actual).abs() <= START_TIME_TOLERANCE_SECONDS
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
        LifecycleError, ShutdownDisposition, ShutdownDriver, ShutdownPhase, ShutdownRegime,
        SupervisorLifecycle, SyncTickOutcome, is_supervisor_up_with_start_time, state,
    };

    fn temporary_journal() -> tempfile::TempDir {
        Builder::new()
            .prefix("solstone-lifecycle-tick-")
            .tempdir_in("/var/tmp")
            .expect("temporary journal")
    }

    #[test]
    fn ac3_injected_start_time_rejects_reused_pid() {
        let root =
            state::test_supervisor_journal("supervisor-probe", std::process::id(), 100.0, None);
        assert!(is_supervisor_up_with_start_time(&root, |_| Ok(100.0)));
        assert!(!is_supervisor_up_with_start_time(&root, |_| Ok(101.6)));
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
        assert!(!is_supervisor_up_with_start_time(&root, |_| {
            Err(LifecycleError::Identity("process start time"))
        }));
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
    fn tick_sync_maps_renewal_retention_scan_conflict_and_healthy_outcomes() {
        let healthy_journal = temporary_journal();
        let mut healthy = SupervisorLifecycle::boot(healthy_journal.path()).expect("boot");
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
        let mut renewal = SupervisorLifecycle::boot(renewal_journal.path()).expect("boot");
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
        let mut retention = SupervisorLifecycle::boot(retention_journal.path()).expect("boot");
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
        let mut scan = SupervisorLifecycle::boot(scan_journal.path()).expect("boot");
        fs::create_dir(scan_journal.path().join("health/sync/unsafe")).expect("unsafe entry");
        assert!(matches!(
            scan.tick_sync(None, 10.0),
            SyncTickOutcome::CompleteScanFailure(_)
        ));
    }

    #[test]
    fn shutdown_runs_driver_after_non_clean_heartbeat_cleanup() {
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

        let journal = temporary_journal();
        let lifecycle = SupervisorLifecycle::boot(journal.path()).expect("boot");
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
        let report = result.expect("non-clean heartbeat cleanup is auxiliary");
        assert_eq!(
            driver.0,
            vec!["reap", "drain", "children", "bus"],
            "the shutdown driver must run after an ambiguous claim cleanup"
        );
        assert_eq!(report.phases.last(), Some(&ShutdownPhase::JoinBusCompleted));
    }
}
