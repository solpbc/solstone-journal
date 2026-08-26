// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Supervisor lifecycle primitives.  This module owns `health/` operational
//! state but deliberately does not provide a supervisor binary or CLI.

mod admission;
mod parent;
mod readiness;
mod shutdown;
mod startup;
mod state;
mod sweep;
mod sync;

use std::path::{Path, PathBuf};

use thiserror::Error;

pub use admission::SupervisorLease;
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
    append_supervisor_log, clear_ready as clear_readiness, clear_self_heartbeat,
    compact_log_if_oversized, recorded_supervisor_pid, write_sync_heartbeat,
};
pub use sweep::{OrphanSweepOutcome, OrphanSweepReport, sweep_orphans};
pub use sync::{
    DEFAULT_INTERVAL_SECONDS, FRESH_WINDOW_MULTIPLIER, ForeignWriter, Heartbeat, SyncCheckResult,
    SyncConflictEvent, SyncSnapshot, check as check_sync, format_conflict_message, machine_id,
    sanitize_hostname, sync_conflict_event,
};

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
    #[error("another solstone writer is active on this journal")]
    SyncConflict(Box<SyncCheckResult>),
}

/// Held singleton admission and its journal root.
pub struct SupervisorLifecycle {
    journal: PathBuf,
    heartbeat_filename: String,
    last_orphan_sweep: OrphanSweepOutcome,
    _lease: SupervisorLease,
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

    /// Acquire admission, reject live foreign writers, record identity, sweep
    /// matching orphans, and publish this process's self-heartbeat.
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
        // After the readiness marker exists: convey has been waited on, and
        // the control socket is bound. Type=notify units stay activating
        // until this datagram arrives.
        sd_notify("READY=1");
        Ok(())
    }

    pub fn clear_ready(&self) -> Result<(), LifecycleError> {
        state::clear_ready(&self.journal)
    }

    pub fn last_orphan_sweep(&self) -> &OrphanSweepOutcome {
        &self.last_orphan_sweep
    }

    pub fn shutdown(
        &self,
        driver: &mut dyn ShutdownDriver,
        regime: ShutdownRegime,
        sync_conflict: bool,
    ) -> Result<ShutdownReport, LifecycleError> {
        state::clear_ready(&self.journal)?;
        if !sync_conflict {
            state::clear_self_heartbeat(&self.journal, &self.heartbeat_filename)?;
            state::clear_supervisor_identity(&self.journal)?;
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

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::{LifecycleError, is_supervisor_up_with_start_time, state};

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
        // Fail-closed through the injected start-time source. This does not by
        // itself prove the deleted kill(None) probe is gone: the fixture PID is
        // this process, so the old probe would have succeeded and still reached
        // the closure.
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
        assert!(
            signal_ready.contains("sd_notify(\"READY=1\")"),
            "Type=notify units stay activating unless signal_ready sends READY=1"
        );
        assert!(
            signal_ready.find("write_readiness").expect("marker write")
                < signal_ready.find("sd_notify(\"READY=1\")").expect("notify"),
            "READY=1 must follow the readiness marker, not precede it"
        );
    }
}
