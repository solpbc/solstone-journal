// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Instant,
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
pub fn snapshot(
    pid: i32,
    source: &dyn ProcessInstanceSource,
    deadline: Option<Instant>,
) -> io::Result<ProcessTreeSnapshot> {
    let root_pid = u32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid managed parent pid"))?;
    match source.census_tree(root_pid, deadline) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CensusCall {
        Unbounded,
        Bounded,
    }

    struct FakeSource {
        census: InstanceCensus,
        census_until: InstanceCensus,
        calls: Mutex<Vec<CensusCall>>,
    }

    impl ProcessInstanceSource for FakeSource {
        fn inspect(&self, _pid: u32) -> InspectResult {
            InspectResult::Unverifiable
        }

        fn census(&self) -> InstanceCensus {
            self.calls
                .lock()
                .expect("calls lock")
                .push(CensusCall::Unbounded);
            self.census.clone()
        }

        fn census_until(&self, _deadline: Instant) -> InstanceCensus {
            self.calls
                .lock()
                .expect("calls lock")
                .push(CensusCall::Bounded);
            self.census_until.clone()
        }
    }

    struct PanicCensusSource;

    impl ProcessInstanceSource for PanicCensusSource {
        fn inspect(&self, _pid: u32) -> InspectResult {
            InspectResult::Unverifiable
        }

        fn census(&self) -> InstanceCensus {
            panic!("expired deadline must not call census")
        }
    }

    fn row(pid: u32, ppid: u32, pgid: i32) -> CensusRow {
        CensusRow {
            instance: super::super::ProcessInstance {
                pid,
                birth: ProcessBirth::linux(u64::from(pid), 1, 100),
            },
            ppid,
            pgid,
            execution: super::super::ExecutionState::Running,
        }
    }

    #[test]
    fn snapshot_without_deadline_uses_complete_unbounded_census() {
        let source = FakeSource {
            census: InstanceCensus::Complete(vec![row(10, 1, 10), row(11, 10, 10)]),
            census_until: InstanceCensus::Incomplete(Vec::new()),
            calls: Mutex::new(Vec::new()),
        };

        let tree = snapshot(10, &source, None).expect("complete census snapshot");

        assert_eq!(tree.parent_pid, 10);
        assert_eq!(
            tree.descendants,
            vec![Descendant {
                pid: 11,
                pgid: Some(10)
            }]
        );
        assert_eq!(
            *source.calls.lock().expect("calls lock"),
            vec![CensusCall::Unbounded]
        );
    }

    #[test]
    fn snapshot_with_deadline_uses_bounded_census_and_rejects_partial_rows() {
        let source = FakeSource {
            census: InstanceCensus::Complete(vec![row(10, 1, 10)]),
            census_until: InstanceCensus::Incomplete(vec![row(10, 1, 10)]),
            calls: Mutex::new(Vec::new()),
        };

        let error = snapshot(
            10,
            &source,
            Some(Instant::now() + std::time::Duration::from_secs(1)),
        )
        .expect_err("incomplete census must fail closed");

        assert_eq!(error.to_string(), "process census incomplete");
        assert_eq!(
            *source.calls.lock().expect("calls lock"),
            vec![CensusCall::Bounded]
        );
    }

    #[test]
    fn census_until_short_circuits_an_expired_deadline() {
        assert_eq!(
            PanicCensusSource.census_until(Instant::now()),
            InstanceCensus::Incomplete(Vec::new())
        );
    }
}
