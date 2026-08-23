// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use super::{
    CensusRow, InspectResult, InstanceCensus, ProcessBirth, ProcessInstanceSource,
    SystemProcessInstanceSource,
};

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
    pub descendant_births: HashMap<i32, ProcessBirth>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn snapshot(pid: i32) -> io::Result<ProcessTreeSnapshot> {
    match SystemProcessInstanceSource.census() {
        InstanceCensus::Complete(rows) => tree_from_census(pid, &rows),
        InstanceCensus::Incomplete(_) => Err(io::Error::other("process census incomplete")),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn tree_from_census(parent_pid: i32, rows: &[CensusRow]) -> io::Result<ProcessTreeSnapshot> {
    let by_parent = rows
        .iter()
        .fold(BTreeMap::<i32, Vec<&CensusRow>>::new(), |mut map, row| {
            if let Ok(ppid) = i32::try_from(row.ppid) {
                map.entry(ppid).or_default().push(row);
            }
            map
        });
    let parent = rows
        .iter()
        .find(|row| i32::try_from(row.instance.pid).ok() == Some(parent_pid))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "managed parent not found"))?;
    let parent_pgid = parent.pgid;
    let mut pending = vec![parent_pid];
    let mut descendants = BTreeSet::new();
    let mut descendant_births = HashMap::new();
    while let Some(current) = pending.pop() {
        if let Some(children) = by_parent.get(&current) {
            for child in children {
                let Ok(child_pid) = i32::try_from(child.instance.pid) else {
                    continue;
                };
                if descendants.insert(Descendant {
                    pid: child_pid,
                    pgid: Some(child.pgid),
                }) {
                    descendant_births.insert(child_pid, child.instance.birth);
                    pending.push(child_pid);
                }
            }
        }
    }
    Ok(ProcessTreeSnapshot {
        parent_pid,
        parent_pgid: Some(parent_pgid),
        descendants: descendants.into_iter().collect(),
        descendant_births,
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn own_pgid() -> Option<i32> {
    match SystemProcessInstanceSource.inspect(std::process::id()) {
        InspectResult::Present { pgid, .. } => pgid,
        InspectResult::Absent | InspectResult::Unverifiable => None,
    }
}
