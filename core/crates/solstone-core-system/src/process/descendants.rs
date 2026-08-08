// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::collections::{BTreeMap, BTreeSet};
use std::io;

/// A descendant PID together with the process group observed before signaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Descendant {
    pub pid: i32,
    pub pgid: Option<i32>,
}

/// Process tree captured before any termination signal is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessTreeSnapshot {
    pub parent_pid: i32,
    pub parent_pgid: Option<i32>,
    pub descendants: Vec<Descendant>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Clone, Copy)]
struct ProcessRow {
    pid: i32,
    ppid: i32,
    pgid: i32,
}

#[cfg(target_os = "linux")]
pub fn snapshot(pid: i32) -> io::Result<ProcessTreeSnapshot> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(pid_text) = name.to_str() else {
            continue;
        };
        let Ok(row_pid) = pid_text.parse::<i32>() else {
            continue;
        };
        let stat = match std::fs::read_to_string(format!("/proc/{row_pid}/stat")) {
            Ok(stat) => stat,
            Err(_) => continue,
        };
        if let Some(row) = parse_linux_stat(&stat) {
            rows.push(row);
        }
    }
    tree_from_rows(pid, &rows)
}

#[cfg(target_os = "linux")]
fn parse_linux_stat(stat: &str) -> Option<ProcessRow> {
    let close = stat.rfind(')')?;
    let prefix = stat.get(..close)?.trim();
    let pid = prefix.split_whitespace().next()?.parse().ok()?;
    let fields: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    // Field 3 is state, field 4 ppid, field 5 pgrp.
    let ppid = fields.get(1)?.parse().ok()?;
    let pgid = fields.get(2)?.parse().ok()?;
    Some(ProcessRow { pid, ppid, pgid })
}

#[cfg(target_os = "macos")]
pub fn snapshot(pid: i32) -> io::Result<ProcessTreeSnapshot> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pgid="])
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("ps process listing failed"));
    }
    let rows = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            Some(ProcessRow {
                pid: fields.first()?.parse().ok()?,
                ppid: fields.get(1)?.parse().ok()?,
                pgid: fields.get(2)?.parse().ok()?,
            })
        })
        .collect::<Vec<_>>();
    tree_from_rows(pid, &rows)
}

#[cfg(target_os = "ios")]
pub fn snapshot(_pid: i32) -> io::Result<ProcessTreeSnapshot> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "descendant enumeration is unavailable on iOS",
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
pub fn snapshot(_pid: i32) -> io::Result<ProcessTreeSnapshot> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "descendant enumeration is unsupported on this platform",
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn tree_from_rows(parent_pid: i32, rows: &[ProcessRow]) -> io::Result<ProcessTreeSnapshot> {
    let by_parent = rows
        .iter()
        .fold(BTreeMap::<i32, Vec<ProcessRow>>::new(), |mut map, row| {
            map.entry(row.ppid).or_default().push(*row);
            map
        });
    let parent_pgid = rows
        .iter()
        .find(|row| row.pid == parent_pid)
        .map(|row| row.pgid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "managed parent not found"))?;
    let mut pending = vec![parent_pid];
    let mut descendants = BTreeSet::new();
    while let Some(current) = pending.pop() {
        if let Some(children) = by_parent.get(&current) {
            for child in children {
                if descendants.insert(Descendant {
                    pid: child.pid,
                    pgid: Some(child.pgid),
                }) {
                    pending.push(child.pid);
                }
            }
        }
    }
    Ok(ProcessTreeSnapshot {
        parent_pid,
        parent_pgid: Some(parent_pgid),
        descendants: descendants.into_iter().collect(),
    })
}

#[cfg(target_os = "linux")]
pub fn own_pgid() -> Option<i32> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    parse_linux_stat(&stat).map(|row| row.pgid)
}

#[cfg(target_os = "macos")]
pub fn own_pgid() -> Option<i32> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "pgid=", "-p", &std::process::id().to_string()])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn own_pgid() -> Option<i32> {
    None
}
