// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::collections::BTreeSet;
use std::io;
use std::process::Child;
use std::time::Duration;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::{process::ExitStatus, time::Instant};

use thiserror::Error;

use super::descendants::Descendant;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::descendants::{ProcessTreeSnapshot, own_pgid, snapshot};

/// Task cap enforcement's bounded graceful window.
pub const CAP_TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);
/// Future TaskQueue shutdown's distinct default window.
pub const TASK_QUEUE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// Long-lived service shutdown's distinct default window.
pub const SERVICE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
/// Unconditional bounded reap window after SIGKILL escalation.
pub const KILL_REAP_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescendantCoverage {
    Proven,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationOutcome {
    Graceful { exit_code: Option<i32> },
    EscalatedAndReaped { exit_code: Option<i32> },
}

#[derive(Debug, Error)]
pub enum TerminationError {
    #[error("managed parent missed the graceful termination window")]
    ParentGraceTimeout,
    #[error("process tree not reaped: {reason}; survivors={survivors:?}")]
    ProcessTreeNotReaped {
        reason: &'static str,
        survivors: Vec<Descendant>,
    },
    #[error("descendant coverage unavailable on this platform")]
    DescendantCoverageUnavailable,
    #[error("process lifecycle I/O failed: {0}")]
    Io(#[from] io::Error),
}

/// Snapshot before every signal so escaped descendants retain direct PID targets.
pub fn terminate(
    child: &mut Child,
    timeout: Duration,
) -> Result<TerminationOutcome, TerminationError> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        terminate_unix(child, timeout)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        terminate_without_descendant_coverage(child, timeout)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn terminate_unix(
    child: &mut Child,
    timeout: Duration,
) -> Result<TerminationOutcome, TerminationError> {
    let parent_pid =
        i32::try_from(child.id()).map_err(|_| io::Error::other("invalid child pid"))?;
    let snapshot_result = snapshot(parent_pid);
    let snapshot_uncertain = snapshot_result.is_err();
    let tree = snapshot_result.unwrap_or(ProcessTreeSnapshot {
        parent_pid,
        parent_pgid: Some(parent_pid),
        descendants: Vec::new(),
    });
    let guard = SignalGuard::current();

    // Four target classes: parent PID, parent PGID, descendant PIDs, descendant PGIDs.
    signal_tree(&tree, SignalKind::Terminate, &guard);
    let deadline = Instant::now() + timeout;
    let parent_exit = wait_for_child(child, deadline)?;
    let Some(parent_exit) = parent_exit else {
        signal_tree(&tree, SignalKind::Kill, &guard);
        let _ = wait_for_child(child, Instant::now() + KILL_REAP_GRACE)?;
        let _ = wait_for_descendants(&tree.descendants, Instant::now() + KILL_REAP_GRACE);
        // Preserve Python: a missed graceful parent deadline remains distinct.
        return Err(TerminationError::ParentGraceTimeout);
    };
    let parent_exit_code = parent_exit.code();

    if snapshot_uncertain {
        return Err(TerminationError::ProcessTreeNotReaped {
            reason: "cleanup_unproven",
            survivors: Vec::new(),
        });
    }

    let survivors = wait_for_descendants(&tree.descendants, deadline);
    if survivors.is_empty() {
        return Ok(TerminationOutcome::Graceful {
            exit_code: parent_exit_code,
        });
    }

    signal_descendants(&survivors, SignalKind::Kill, &guard);
    let survivors = wait_for_descendants(&survivors, Instant::now() + KILL_REAP_GRACE);
    if survivors.is_empty() {
        Ok(TerminationOutcome::EscalatedAndReaped {
            exit_code: parent_exit_code,
        })
    } else {
        Err(TerminationError::ProcessTreeNotReaped {
            reason: "survived_sigkill",
            survivors,
        })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn terminate_without_descendant_coverage(
    child: &mut Child,
    _timeout: Duration,
) -> Result<TerminationOutcome, TerminationError> {
    child.kill()?;
    let _ = child.wait()?;
    Err(TerminationError::DescendantCoverageUnavailable)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn wait_for_child(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn wait_for_descendants(descendants: &[Descendant], deadline: Instant) -> Vec<Descendant> {
    let mut survivors = live_descendants(descendants);
    while !survivors.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        survivors = live_descendants(descendants);
    }
    survivors
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn live_descendants(descendants: &[Descendant]) -> Vec<Descendant> {
    descendants
        .iter()
        .copied()
        .filter(|descendant| process_alive(descendant.pid))
        .collect()
}

#[cfg(target_os = "linux")]
fn process_alive(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(close) = stat.rfind(')') else {
        return true;
    };
    stat.get(close + 1..)
        .and_then(|tail| tail.split_whitespace().next())
        != Some("Z")
}

#[cfg(target_os = "macos")]
fn process_alive(pid: i32) -> bool {
    std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Clone, Copy)]
enum SignalKind {
    Terminate,
    Kill,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Clone, Copy)]
struct SignalGuard {
    own_pid: i32,
    own_pgid: Option<i32>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl SignalGuard {
    fn current() -> Self {
        Self {
            own_pid: i32::try_from(std::process::id()).unwrap_or(i32::MAX),
            own_pgid: own_pgid(),
        }
    }

    fn permits_pid(self, pid: i32) -> bool {
        pid > 1 && pid != self.own_pid
    }

    fn permits_pgid(self, pgid: i32) -> bool {
        pgid > 1 && pgid != self.own_pid && self.own_pgid.is_some_and(|own| pgid != own)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_tree(tree: &ProcessTreeSnapshot, kind: SignalKind, guard: &SignalGuard) {
    signal_pid(tree.parent_pid, kind, guard);
    if let Some(pgid) = tree.parent_pgid {
        signal_pgid(pgid, kind, guard);
    }
    signal_descendants(&tree.descendants, kind, guard);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_descendants(descendants: &[Descendant], kind: SignalKind, guard: &SignalGuard) {
    for descendant in descendants {
        signal_pid(descendant.pid, kind, guard);
    }
    let mut pgids = BTreeSet::new();
    for descendant in descendants {
        if let Some(pgid) = descendant.pgid {
            pgids.insert(pgid);
        }
    }
    for pgid in pgids {
        signal_pgid(pgid, kind, guard);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_pid(pid: i32, kind: SignalKind, guard: &SignalGuard) {
    if !guard.permits_pid(pid) {
        return;
    }
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix_signal(kind));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_pgid(pgid: i32, kind: SignalKind, guard: &SignalGuard) {
    if !guard.permits_pgid(pgid) {
        return;
    }
    let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), nix_signal(kind));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn nix_signal(kind: SignalKind) -> nix::sys::signal::Signal {
    match kind {
        SignalKind::Terminate => nix::sys::signal::Signal::SIGTERM,
        SignalKind::Kill => nix::sys::signal::Signal::SIGKILL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac15_signal_guard_never_targets_the_caller_or_its_process_group() {
        let guard = SignalGuard::current();
        assert!(!guard.permits_pid(0));
        assert!(!guard.permits_pid(1));
        assert!(!guard.permits_pid(guard.own_pid));
        assert!(guard.permits_pid(guard.own_pid.saturating_add(10_000)));

        assert!(!guard.permits_pgid(0));
        assert!(!guard.permits_pgid(1));
        if let Some(own_pgid) = guard.own_pgid {
            assert!(!guard.permits_pgid(own_pgid));
        }
        assert!(guard.permits_pgid(guard.own_pid.saturating_add(10_000)));
    }
}
