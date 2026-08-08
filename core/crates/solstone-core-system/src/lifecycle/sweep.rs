// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::Duration;
#[cfg(target_os = "linux")]
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

/// Linux procfs is the only supported source for target env, uid, and title.
/// Other targets refuse instead of risking a proctitle-only destructive sweep.
#[cfg(target_os = "linux")]
pub fn sweep_orphans(journal: &Path, grace: Duration) -> OrphanSweepOutcome {
    use nix::sys::signal::Signal;

    let Ok(journal) = journal.canonicalize() else {
        return OrphanSweepOutcome::Completed(OrphanSweepReport {
            skipped_unresolvable: 1,
            ..OrphanSweepReport::default()
        });
    };
    let own_pid = std::process::id();
    let own_uid = uid_for("/proc/self/status");
    let mut report = OrphanSweepReport::default();
    let mut targets = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return OrphanSweepOutcome::Completed(report);
    };
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
        if parent_pid(&format!("{base}/stat")) != Some(1)
            || uid_for(&format!("{base}/status")) != own_uid
        {
            continue;
        }
        let Some(candidate) = journal_for(&format!("{base}/environ")) else {
            report.skipped_unresolvable += 1;
            continue;
        };
        let Ok(candidate) = candidate.canonicalize() else {
            report.skipped_unresolvable += 1;
            continue;
        };
        if candidate != journal {
            continue;
        }
        targets.push(pid);
    }
    report.targeted = targets.len();
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
fn process_is_live(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat.rfind(')')
        .and_then(|end| stat[end + 1..].split_whitespace().next())
        != Some("Z")
}

#[cfg(not(target_os = "linux"))]
pub fn sweep_orphans(_journal: &Path, _grace: Duration) -> OrphanSweepOutcome {
    OrphanSweepOutcome::UnsupportedPlatform
}

#[cfg(target_os = "linux")]
fn sweepable_name(name: &str) -> bool {
    name.starts_with("journal:")
        || matches!(name, "llama-server" | "parakeet-server" | "mlx-vlm-server")
}

#[cfg(target_os = "linux")]
fn parent_pid(path: &str) -> Option<u32> {
    let stat = std::fs::read_to_string(path).ok()?;
    stat[stat.rfind(')')? + 1..]
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
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
