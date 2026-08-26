// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::collections::HashMap;
use std::io;
use std::process::Child;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use thiserror::Error;

use super::descendants::Descendant;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::{
    InspectResult, InstanceVerdict, ProcessBirth, ProcessInstance, ProcessInstanceSource,
    SystemProcessInstanceSource,
    descendants::{ProcessTreeSnapshot, own_pgid, snapshot},
    signal_aware_exit_code,
};

/// Task cap enforcement's bounded graceful window.
pub const CAP_TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);
/// Future TaskQueue shutdown's distinct default window.
pub const TASK_QUEUE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// Long-lived service shutdown's distinct default window.
pub const SERVICE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
/// Unconditional bounded reap window after SIGKILL escalation.
pub const KILL_REAP_GRACE: Duration = Duration::from_millis(500);
/// Bounded drain-thread join after the child and descendants are reaped.
pub const DRAIN_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

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
    #[error("exact process identity is unavailable")]
    ExactInstanceUnavailable,
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

/// Terminate one birth-bound child tree without process-group fallback.
///
/// Every mutation is preceded by a fresh identity observation. A caller that
/// cannot prove the remembered parent still names the target must not signal.
pub fn terminate_exact_instance(
    child: &mut Child,
    expected: ProcessInstance,
    timeout: Duration,
    source: &dyn ProcessInstanceSource,
) -> Result<TerminationOutcome, TerminationError> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        terminate_exact_unix(child, expected, timeout, source)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (expected, source);
        terminate_without_descendant_coverage(child, timeout)
    }
}

/// Terminate one birth-bound child tree without waiting beyond `deadline`.
///
/// As with [`terminate_exact_instance`], every signal remains guarded by a
/// fresh exact-instance observation. A deadline expiry reports the existing
/// forced-shutdown error rather than widening authority to a replacement.
pub fn terminate_exact_instance_until(
    child: &mut Child,
    expected: ProcessInstance,
    deadline: Instant,
    source: &dyn ProcessInstanceSource,
) -> Result<TerminationOutcome, TerminationError> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        terminate_exact_unix_until(child, expected, deadline, source)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (child, expected, deadline, source);
        Err(TerminationError::DescendantCoverageUnavailable)
    }
}

/// Send a direct signal only after revalidating the exact remembered process.
pub fn signal_exact_instance(
    expected: ProcessInstance,
    signal: nix::sys::signal::Signal,
    source: &dyn ProcessInstanceSource,
) -> Result<(), TerminationError> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let guard = SignalGuard::current();
        signal_parent_exact(expected, signal, &guard, source)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (expected, signal, source);
        Err(TerminationError::DescendantCoverageUnavailable)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn terminate_exact_unix(
    child: &mut Child,
    expected: ProcessInstance,
    timeout: Duration,
    source: &dyn ProcessInstanceSource,
) -> Result<TerminationOutcome, TerminationError> {
    let parent_pid =
        i32::try_from(expected.pid).map_err(|_| io::Error::other("invalid child pid"))?;
    if i32::try_from(child.id()).ok() != Some(parent_pid) {
        return Err(TerminationError::ExactInstanceUnavailable);
    }
    let tree = snapshot(parent_pid).map_err(|_| TerminationError::ProcessTreeNotReaped {
        reason: "cleanup_unproven",
        survivors: Vec::new(),
    })?;
    let guard = SignalGuard::current();
    signal_tree_exact(&tree, expected, SignalKind::Terminate, &guard, source)?;
    let deadline = Instant::now() + timeout;
    let parent_exit = wait_for_child(child, deadline)?;
    let Some(parent_exit) = parent_exit else {
        signal_tree_exact(&tree, expected, SignalKind::Kill, &guard, source)?;
        let _ = wait_for_child(child, Instant::now() + KILL_REAP_GRACE)?;
        let _ = wait_for_descendants(&tree.descendants, Instant::now() + KILL_REAP_GRACE, source);
        return Err(TerminationError::ParentGraceTimeout);
    };
    let exit_code = Some(signal_aware_exit_code(&parent_exit));
    let survivors = wait_for_descendants(&tree.descendants, deadline, source);
    if survivors.is_empty() {
        return Ok(TerminationOutcome::Graceful { exit_code });
    }
    let (confirmed, unproven) =
        select_confirmed_descendants(&survivors, &tree.descendant_births, source);
    if !unproven.is_empty() {
        return Err(TerminationError::ProcessTreeNotReaped {
            reason: "cleanup_unproven",
            survivors: unproven,
        });
    }
    signal_descendants(&confirmed, SignalKind::Kill, &guard);
    let leftover = wait_for_descendants(&confirmed, Instant::now() + KILL_REAP_GRACE, source);
    if leftover.is_empty() {
        Ok(TerminationOutcome::EscalatedAndReaped { exit_code })
    } else {
        Err(TerminationError::ProcessTreeNotReaped {
            reason: "survived_sigkill",
            survivors: leftover,
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn terminate_exact_unix_until(
    child: &mut Child,
    expected: ProcessInstance,
    deadline: Instant,
    source: &dyn ProcessInstanceSource,
) -> Result<TerminationOutcome, TerminationError> {
    if Instant::now() >= deadline {
        return Err(TerminationError::ParentGraceTimeout);
    }
    let parent_pid =
        i32::try_from(expected.pid).map_err(|_| io::Error::other("invalid child pid"))?;
    if i32::try_from(child.id()).ok() != Some(parent_pid) {
        return Err(TerminationError::ExactInstanceUnavailable);
    }
    let tree = snapshot(parent_pid).map_err(|_| TerminationError::ProcessTreeNotReaped {
        reason: "cleanup_unproven",
        survivors: Vec::new(),
    })?;
    let guard = SignalGuard::current();
    signal_tree_exact(&tree, expected, SignalKind::Terminate, &guard, source)?;
    let parent_exit = wait_for_child(child, deadline)?;
    let Some(parent_exit) = parent_exit else {
        signal_tree_exact(&tree, expected, SignalKind::Kill, &guard, source)?;
        let _ = wait_for_child(child, deadline)?;
        let _ = wait_for_descendants(&tree.descendants, deadline, source);
        return Err(TerminationError::ParentGraceTimeout);
    };
    let exit_code = Some(signal_aware_exit_code(&parent_exit));
    let survivors = wait_for_descendants(&tree.descendants, deadline, source);
    if survivors.is_empty() {
        return Ok(TerminationOutcome::Graceful { exit_code });
    }
    let (confirmed, unproven) =
        select_confirmed_descendants(&survivors, &tree.descendant_births, source);
    if !unproven.is_empty() {
        return Err(TerminationError::ProcessTreeNotReaped {
            reason: "cleanup_unproven",
            survivors: unproven,
        });
    }
    signal_descendants(&confirmed, SignalKind::Kill, &guard);
    let leftover = wait_for_descendants(&confirmed, deadline, source);
    if leftover.is_empty() {
        Ok(TerminationOutcome::EscalatedAndReaped { exit_code })
    } else {
        Err(TerminationError::ProcessTreeNotReaped {
            reason: "survived_sigkill",
            survivors: leftover,
        })
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
        descendant_births: HashMap::new(),
    });
    let guard = SignalGuard::current();
    let source = SystemProcessInstanceSource;

    // Parent PID, parent PGID, and confirmed descendant PIDs.
    signal_tree(&tree, SignalKind::Terminate, &guard, &source);
    let deadline = Instant::now() + timeout;
    let parent_exit = wait_for_child(child, deadline)?;
    let Some(parent_exit) = parent_exit else {
        signal_tree(&tree, SignalKind::Kill, &guard, &source);
        let _ = wait_for_child(child, Instant::now() + KILL_REAP_GRACE)?;
        let _ = wait_for_descendants(&tree.descendants, Instant::now() + KILL_REAP_GRACE, &source);
        // Preserve Python: a missed graceful parent deadline remains distinct.
        return Err(TerminationError::ParentGraceTimeout);
    };
    let parent_exit_code = Some(signal_aware_exit_code(&parent_exit));

    if snapshot_uncertain {
        return Err(TerminationError::ProcessTreeNotReaped {
            reason: "cleanup_unproven",
            survivors: Vec::new(),
        });
    }

    let survivors = wait_for_descendants(&tree.descendants, deadline, &source);
    if survivors.is_empty() {
        return Ok(TerminationOutcome::Graceful {
            exit_code: parent_exit_code,
        });
    }

    let (confirmed, unproven) =
        select_confirmed_descendants(&survivors, &tree.descendant_births, &source);
    if !confirmed.is_empty() {
        signal_descendants(&confirmed, SignalKind::Kill, &guard);
    }
    let leftover = wait_for_descendants(&confirmed, Instant::now() + KILL_REAP_GRACE, &source);
    if !unproven.is_empty() {
        let mut reported = unproven;
        reported.extend(leftover);
        return Err(TerminationError::ProcessTreeNotReaped {
            reason: "cleanup_unproven",
            survivors: reported,
        });
    }
    if leftover.is_empty() {
        Ok(TerminationOutcome::EscalatedAndReaped {
            exit_code: parent_exit_code,
        })
    } else {
        Err(TerminationError::ProcessTreeNotReaped {
            reason: "survived_sigkill",
            survivors: leftover,
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
fn wait_for_descendants(
    descendants: &[Descendant],
    deadline: Instant,
    source: &dyn ProcessInstanceSource,
) -> Vec<Descendant> {
    let mut survivors = live_descendants(descendants, source);
    while !survivors.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        survivors = live_descendants(descendants, source);
    }
    survivors
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn live_descendants(
    descendants: &[Descendant],
    source: &dyn ProcessInstanceSource,
) -> Vec<Descendant> {
    descendants
        .iter()
        .copied()
        .filter(|descendant| descendant_present(descendant.pid, source))
        .collect()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn descendant_present(pid: i32, source: &dyn ProcessInstanceSource) -> bool {
    u32::try_from(pid).is_ok_and(|pid| !matches!(source.inspect(pid), InspectResult::Absent))
}

/// Shared same-birth gate behind all three `terminate_unix` signal sites
/// (initial TERM, parent-grace-timeout KILL, survivor KILL). Takes no
/// `SignalKind`, so one test suite covers every site that calls it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn select_confirmed_descendants(
    survivors: &[Descendant],
    births: &HashMap<i32, ProcessBirth>,
    source: &dyn ProcessInstanceSource,
) -> (Vec<Descendant>, Vec<Descendant>) {
    let mut confirmed = Vec::new();
    let mut unproven = Vec::new();
    for survivor in survivors {
        let Some(pid) = u32::try_from(survivor.pid).ok() else {
            unproven.push(*survivor);
            continue;
        };
        let Some(birth) = births.get(&survivor.pid).copied() else {
            unproven.push(*survivor);
            continue;
        };
        match source.observe(&ProcessInstance { pid, birth }) {
            InstanceVerdict::SameLive { .. } => confirmed.push(*survivor),
            InstanceVerdict::NotSameOrExited | InstanceVerdict::Unverifiable => {
                unproven.push(*survivor);
            }
        }
    }
    (confirmed, unproven)
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
fn signal_tree(
    tree: &ProcessTreeSnapshot,
    kind: SignalKind,
    guard: &SignalGuard,
    source: &dyn ProcessInstanceSource,
) {
    signal_pid_guarded(tree.parent_pid, kind, guard);
    if let Some(pgid) = tree.parent_pgid {
        signal_pgid(pgid, kind, guard);
    }
    let (confirmed, _unproven) =
        select_confirmed_descendants(&tree.descendants, &tree.descendant_births, source);
    // Sites 1/2 have no payload-carrying error to report unproven through;
    // site 3 re-derives the authoritative unproven set later.
    signal_descendants(&confirmed, kind, guard);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_tree_exact(
    tree: &ProcessTreeSnapshot,
    expected: ProcessInstance,
    kind: SignalKind,
    guard: &SignalGuard,
    source: &dyn ProcessInstanceSource,
) -> Result<(), TerminationError> {
    let (confirmed, unproven) =
        select_confirmed_descendants(&tree.descendants, &tree.descendant_births, source);
    if !unproven.is_empty() {
        return Err(TerminationError::ProcessTreeNotReaped {
            reason: "cleanup_unproven",
            survivors: unproven,
        });
    }
    let signal = nix_signal(kind);
    signal_parent_exact(expected, signal, guard, source)?;
    signal_descendants(&confirmed, kind, guard);
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_parent_exact(
    expected: ProcessInstance,
    signal: nix::sys::signal::Signal,
    guard: &SignalGuard,
    source: &dyn ProcessInstanceSource,
) -> Result<(), TerminationError> {
    if !matches!(source.observe(&expected), InstanceVerdict::SameLive { .. }) {
        return Err(TerminationError::ProcessTreeNotReaped {
            reason: "parent_unproven",
            survivors: Vec::new(),
        });
    }
    let pid = i32::try_from(expected.pid).map_err(|_| io::Error::other("invalid child pid"))?;
    if guard.permits_pid(pid) {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_descendants(descendants: &[Descendant], kind: SignalKind, guard: &SignalGuard) {
    for descendant in descendants {
        signal_pid_guarded(descendant.pid, kind, guard);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn signal_pid_guarded(pid: i32, kind: SignalKind, guard: &SignalGuard) {
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

/// Signal one already-identified process without taking ownership of a child.
///
/// Lifecycle orphan cleanup owns candidate selection and deliberately uses this
/// narrow crate-private seam rather than widening the public process API.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn signal_pid(pid: i32, signal: nix::sys::signal::Signal) -> nix::Result<()> {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal)
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use super::super::{ExecutionState, InstanceCensus};
    use super::*;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use std::collections::HashMap;

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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    struct FakeSource {
        result: InspectResult,
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl ProcessInstanceSource for FakeSource {
        fn inspect(&self, _pid: u32) -> InspectResult {
            self.result
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn case9_error(
        confirmed: Vec<Descendant>,
        unproven: Vec<Descendant>,
    ) -> Result<TerminationOutcome, TerminationError> {
        assert!(
            confirmed.is_empty(),
            "mismatch/unverifiable pids must not be selected for KILL"
        );
        Err(TerminationError::ProcessTreeNotReaped {
            reason: "cleanup_unproven",
            survivors: unproven,
        })
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn case9_kill_gate_mismatch_reports_cleanup_unproven_without_kill_targets() {
        let snapshotted = ProcessBirth::linux(10, 100, 100);
        let current = ProcessBirth::linux(99, 100, 100);
        let survivor = Descendant {
            pid: 42,
            pgid: Some(42),
        };
        let mut births = HashMap::new();
        births.insert(42, snapshotted);
        let source = FakeSource {
            result: InspectResult::Present {
                instance: ProcessInstance {
                    pid: 42,
                    birth: current,
                },
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(42),
            },
        };
        let (confirmed, unproven) = select_confirmed_descendants(&[survivor], &births, &source);
        let error = case9_error(confirmed, unproven).expect_err("cleanup_unproven");
        match error {
            TerminationError::ProcessTreeNotReaped { reason, survivors } => {
                assert_eq!(reason, "cleanup_unproven");
                assert_eq!(survivors, vec![survivor]);
            }
            other => panic!("expected ProcessTreeNotReaped, got {other:?}"),
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn case9_kill_gate_unverifiable_reports_cleanup_unproven_without_kill_targets() {
        let snapshotted = ProcessBirth::linux(10, 100, 100);
        let survivor = Descendant {
            pid: 42,
            pgid: Some(42),
        };
        let mut births = HashMap::new();
        births.insert(42, snapshotted);
        let source = FakeSource {
            result: InspectResult::Unverifiable,
        };
        let (confirmed, unproven) = select_confirmed_descendants(&[survivor], &births, &source);
        let error = case9_error(confirmed, unproven).expect_err("cleanup_unproven");
        match error {
            TerminationError::ProcessTreeNotReaped { reason, survivors } => {
                assert_eq!(reason, "cleanup_unproven");
                assert_eq!(survivors, vec![survivor]);
            }
            other => panic!("expected ProcessTreeNotReaped, got {other:?}"),
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn case9_same_birth_live_is_confirmed() {
        let birth = ProcessBirth::linux(10, 100, 100);
        let survivor = Descendant {
            pid: 42,
            pgid: Some(42),
        };
        let mut births = HashMap::new();
        births.insert(42, birth);
        let source = FakeSource {
            result: InspectResult::Present {
                instance: ProcessInstance { pid: 42, birth },
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(42),
            },
        };
        let (confirmed, unproven) = select_confirmed_descendants(&[survivor], &births, &source);
        assert_eq!(confirmed, vec![survivor]);
        assert!(unproven.is_empty());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn ac16_exact_parent_signal_refuses_a_reused_or_unverifiable_identity() {
        let expected = ProcessInstance {
            pid: 42,
            birth: ProcessBirth::linux(10, 100, 100),
        };
        let guard = SignalGuard::current();
        let reused = FakeSource {
            result: InspectResult::Present {
                instance: ProcessInstance {
                    pid: 42,
                    birth: ProcessBirth::linux(11, 100, 100),
                },
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(42),
            },
        };
        for source in [
            reused,
            FakeSource {
                result: InspectResult::Unverifiable,
            },
        ] {
            assert!(matches!(
                signal_parent_exact(expected, nix::sys::signal::Signal::SIGTERM, &guard, &source),
                Err(TerminationError::ProcessTreeNotReaped {
                    reason: "parent_unproven",
                    ..
                })
            ));
        }
    }
}
