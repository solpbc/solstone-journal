// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::Duration;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Instant;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrphanSweepReport {
    pub targeted: usize,
    pub reaped: usize,
    pub survivors: usize,
    pub skipped_unresolvable: usize,
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

    let (targets, skipped_unresolvable) = sweep_targets(journal);
    let mut report = OrphanSweepReport {
        targeted: targets.len(),
        skipped_unresolvable,
        ..OrphanSweepReport::default()
    };
    for pid in &targets {
        let _ = crate::process::signal_pid(*pid as i32, Signal::SIGTERM);
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && targets.iter().any(|pid| process_is_live(*pid)) {
        std::thread::sleep(Duration::from_millis(10));
    }
    for pid in targets {
        if process_is_live(pid) {
            report.survivors += 1;
            let _ = crate::process::signal_pid(pid as i32, Signal::SIGKILL);
        } else {
            report.reaped += 1;
        }
    }
    OrphanSweepOutcome::Completed(report)
}

#[cfg(target_os = "linux")]
fn sweep_targets(journal: &Path) -> (Vec<u32>, usize) {
    let Ok(journal) = journal.canonicalize() else {
        return (Vec::new(), 1);
    };
    let own_pid = std::process::id();
    let own_uid = uid_for("/proc/self/status");
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return (Vec::new(), 0);
    };
    let mut targets = Vec::new();
    let mut skipped_unresolvable = 0;
    for entry in entries.flatten() {
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
        if parent_pid(pid) != Some(1) || uid_for(&format!("{base}/status")) != own_uid {
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
        targets.push(pid);
    }
    (targets, skipped_unresolvable)
}

#[cfg(target_os = "macos")]
fn sweep_targets(_journal: &Path) -> (Vec<u32>, usize) {
    let Some(rows) = crate::process::macos_sweep_table() else {
        return (Vec::new(), 0);
    };
    let own_pid = std::process::id();
    let own_uid = nix::unistd::geteuid().as_raw();
    let targets = rows
        .into_iter()
        .filter(|row| {
            row.pid != own_pid
                && row.ppid == 1
                && row.uid == own_uid
                && command_name(&row.command).is_some_and(sweepable_name)
        })
        .map(|row| row.pid)
        .collect();
    (targets, 0)
}

#[cfg(target_os = "macos")]
fn command_name(command: &str) -> Option<&str> {
    let executable = command.split_whitespace().next()?;
    Path::new(executable).file_name()?.to_str()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn process_is_live(pid: u32) -> bool {
    use crate::process::{InspectResult, ProcessInstanceSource, SystemProcessInstanceSource};

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

#[cfg(target_os = "linux")]
fn parent_pid(pid: u32) -> Option<u32> {
    use crate::process::{InspectResult, ProcessInstanceSource, SystemProcessInstanceSource};

    match SystemProcessInstanceSource.inspect(pid) {
        InspectResult::Present { ppid, .. } => ppid,
        InspectResult::Absent | InspectResult::Unverifiable => None,
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
