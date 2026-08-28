// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::Duration;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Instant;

#[cfg(target_os = "macos")]
use crate::process::InstanceCensus;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::process::{
    InspectResult, InstanceVerdict, ProcessInstance, ProcessInstanceSource,
    SystemProcessInstanceSource,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrphanSweepReport {
    pub targeted: usize,
    pub reaped: usize,
    pub survivors: usize,
    pub skipped_unresolvable: usize,
    pub unproven: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrphanSweepOutcome {
    Completed(OrphanSweepReport),
    UnsupportedPlatform,
}

/// Linux qualifies candidates by procfs title, parent, uid, and journal path.
/// macOS cannot safely read another process's environment without a private
/// entitlement, so it preserves name, orphaned-parent, and same-uid matching
/// but drops journal-path qualification. A host running multiple journals under
/// one uid can therefore reap a same-named orphan from a different journal.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn sweep_orphans(journal: &Path, grace: Duration) -> OrphanSweepOutcome {
    use nix::sys::signal::Signal;

    let (targets, skipped_unresolvable, qualification_unproven) = sweep_targets(journal);
    let mut report = OrphanSweepReport {
        targeted: targets.len(),
        skipped_unresolvable,
        unproven: qualification_unproven,
        ..OrphanSweepReport::default()
    };
    let source = SystemProcessInstanceSource;
    let term = partition_by_observation(&targets, &source);
    report.reaped += term.already_gone;
    report.unproven += term.unverifiable;
    for instance in &term.confirmed {
        let _ = crate::process::signal_pid(instance.pid as i32, Signal::SIGTERM);
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline
        && term
            .confirmed
            .iter()
            .any(|instance| process_is_live(instance.pid))
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let kill = partition_by_observation(&term.confirmed, &source);
    report.reaped += kill.already_gone;
    report.unproven += kill.unverifiable;
    report.survivors += kill.confirmed.len();
    for instance in &kill.confirmed {
        let _ = crate::process::signal_pid(instance.pid as i32, Signal::SIGKILL);
    }
    OrphanSweepOutcome::Completed(report)
}

#[cfg(target_os = "linux")]
fn sweep_targets(journal: &Path) -> (Vec<ProcessInstance>, usize, usize) {
    let Ok(journal) = journal.canonicalize() else {
        return (Vec::new(), 1, 0);
    };
    let own_pid = std::process::id();
    let Some(own_uid) = uid_for("/proc/self/status") else {
        return (Vec::new(), 1, 0);
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return (Vec::new(), 0, 1);
    };
    let mut targets = Vec::new();
    let mut skipped_unresolvable = 0;
    let mut unproven = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            unproven += 1;
            continue;
        };
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == own_pid {
            continue;
        }
        let base = format!("/proc/{pid}");
        let Ok(comm) = std::fs::read_to_string(format!("{base}/comm")) else {
            continue;
        };
        if !sweepable_name(comm.trim_end()) {
            continue;
        }
        let instance = match qualify_orphan_observation(SystemProcessInstanceSource.inspect(pid)) {
            OrphanQualification::Eligible(instance) => instance,
            OrphanQualification::NotEligible => continue,
            OrphanQualification::Unproven => {
                unproven += 1;
                continue;
            }
        };
        let Some(candidate_uid) = uid_for(&format!("{base}/status")) else {
            skipped_unresolvable += 1;
            continue;
        };
        if candidate_uid != own_uid {
            continue;
        }
        let Some(candidate) = journal_for(&format!("{base}/environ")) else {
            skipped_unresolvable += 1;
            continue;
        };
        let Ok(candidate) = candidate.canonicalize() else {
            skipped_unresolvable += 1;
            continue;
        };
        if candidate != journal {
            continue;
        }
        targets.push(instance);
    }
    (targets, skipped_unresolvable, unproven)
}

#[cfg(target_os = "macos")]
fn sweep_targets(_journal: &Path) -> (Vec<ProcessInstance>, usize, usize) {
    let source = SystemProcessInstanceSource;
    let InstanceCensus::Complete(census) = source.census() else {
        return (Vec::new(), 0, 1);
    };
    let Some(rows) = crate::process::macos_sweep_table() else {
        return (Vec::new(), 0, 1);
    };
    let own_pid = std::process::id();
    let own_uid = nix::unistd::geteuid().as_raw();
    let mut targets = Vec::new();
    let mut skipped_unresolvable = 0;
    for row in rows {
        if row.pid == own_pid || row.ppid != 1 || row.uid != own_uid {
            continue;
        }
        let Some(name) = command_name(&row.command) else {
            skipped_unresolvable += 1;
            continue;
        };
        if !sweepable_name(name) {
            continue;
        }
        let Some(candidate) = census
            .iter()
            .find(|candidate| candidate.instance.pid == row.pid && candidate.ppid == 1)
        else {
            continue;
        };
        targets.push(candidate.instance);
    }
    (targets, skipped_unresolvable, 0)
}

#[cfg(target_os = "macos")]
fn command_name(command: &str) -> Option<&str> {
    let executable = command.split_whitespace().next()?;
    Path::new(executable).file_name()?.to_str()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn process_is_live(pid: u32) -> bool {
    matches!(
        SystemProcessInstanceSource.inspect(pid),
        InspectResult::Present { .. }
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn sweep_orphans(_journal: &Path, _grace: Duration) -> OrphanSweepOutcome {
    OrphanSweepOutcome::UnsupportedPlatform
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn sweepable_name(name: &str) -> bool {
    name.starts_with("journal:")
        || matches!(name, "llama-server" | "parakeet-server" | "mlx-vlm-server")
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrphanQualification {
    Eligible(ProcessInstance),
    NotEligible,
    Unproven,
}

#[cfg(any(target_os = "linux", test))]
fn qualify_orphan_observation(observation: InspectResult) -> OrphanQualification {
    match observation {
        InspectResult::Present {
            instance,
            ppid: Some(1),
            ..
        } => OrphanQualification::Eligible(instance),
        InspectResult::Present { .. } | InspectResult::Absent => OrphanQualification::NotEligible,
        InspectResult::Unverifiable => OrphanQualification::Unproven,
    }
}

#[cfg(target_os = "linux")]
fn uid_for(path: &str) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|line| line.split_whitespace().next()?.parse().ok())
}

#[cfg(target_os = "linux")]
fn journal_for(path: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    std::fs::read(path)
        .ok()?
        .split(|byte| *byte == 0)
        .find_map(|entry| {
            entry
                .strip_prefix(b"SOLSTONE_JOURNAL=")
                .map(std::ffi::OsStr::from_bytes)
        })
        .map(std::path::PathBuf::from)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct LivenessPartition {
    confirmed: Vec<ProcessInstance>,
    already_gone: usize,
    unverifiable: usize,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn partition_by_observation(
    targets: &[ProcessInstance],
    source: &dyn ProcessInstanceSource,
) -> LivenessPartition {
    let mut confirmed = Vec::new();
    let mut already_gone = 0;
    let mut unverifiable = 0;
    for instance in targets {
        match source.observe(instance) {
            InstanceVerdict::SameLive { .. } => confirmed.push(*instance),
            InstanceVerdict::NotSameOrExited => already_gone += 1,
            InstanceVerdict::Unverifiable => unverifiable += 1,
        }
    }
    LivenessPartition {
        confirmed,
        already_gone,
        unverifiable,
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::{
        LivenessPartition, OrphanQualification, partition_by_observation,
        qualify_orphan_observation,
    };
    use crate::process::{
        ExecutionState, InspectResult, InstanceCensus, ProcessBirth, ProcessInstance,
        ProcessInstanceSource,
    };

    struct FakeSource {
        result: InspectResult,
    }

    impl ProcessInstanceSource for FakeSource {
        fn inspect(&self, _pid: u32) -> InspectResult {
            self.result
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    fn live_instance(birth: ProcessBirth) -> ProcessInstance {
        ProcessInstance { pid: 42, birth }
    }

    #[test]
    fn partition_same_birth_live_is_confirmed() {
        let birth = ProcessBirth::linux(10, 100, 100);
        let instance = live_instance(birth);
        let source = FakeSource {
            result: InspectResult::Present {
                instance,
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(42),
            },
        };
        let LivenessPartition {
            confirmed,
            already_gone,
            unverifiable,
        } = partition_by_observation(&[instance], &source);
        assert_eq!(confirmed, vec![instance]);
        assert_eq!(already_gone, 0);
        assert_eq!(unverifiable, 0);
    }

    #[test]
    fn partition_different_birth_counts_already_gone() {
        let snapshotted = ProcessBirth::linux(10, 100, 100);
        let current = ProcessBirth::linux(99, 100, 100);
        let instance = live_instance(snapshotted);
        let source = FakeSource {
            result: InspectResult::Present {
                instance: ProcessInstance {
                    pid: 42,
                    birth: current,
                },
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(42),
            },
        };
        let LivenessPartition {
            confirmed,
            already_gone,
            unverifiable,
        } = partition_by_observation(&[instance], &source);
        assert!(confirmed.is_empty());
        assert_eq!(already_gone, 1);
        assert_eq!(unverifiable, 0);
    }

    #[test]
    fn partition_unverifiable_counts_unproven() {
        let instance = live_instance(ProcessBirth::linux(10, 100, 100));
        let source = FakeSource {
            result: InspectResult::Unverifiable,
        };
        let LivenessPartition {
            confirmed,
            already_gone,
            unverifiable,
        } = partition_by_observation(&[instance], &source);
        assert!(confirmed.is_empty());
        assert_eq!(already_gone, 0);
        assert_eq!(unverifiable, 1);
    }

    #[test]
    fn qualification_unverifiable_is_carried_as_unproven() {
        assert_eq!(
            qualify_orphan_observation(InspectResult::Unverifiable),
            OrphanQualification::Unproven
        );
    }

    #[test]
    fn qualification_present_orphan_retains_its_birth_identity() {
        let instance = live_instance(ProcessBirth::linux(10, 100, 100));
        assert_eq!(
            qualify_orphan_observation(InspectResult::Present {
                instance,
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(42),
            }),
            OrphanQualification::Eligible(instance)
        );
    }

    #[test]
    fn qualification_present_non_orphan_and_absent_are_not_candidates() {
        let instance = live_instance(ProcessBirth::linux(10, 100, 100));
        assert_eq!(
            qualify_orphan_observation(InspectResult::Present {
                instance,
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(2),
                pgid: Some(42),
            }),
            OrphanQualification::NotEligible
        );
        assert_eq!(
            qualify_orphan_observation(InspectResult::Absent),
            OrphanQualification::NotEligible
        );
    }
}
