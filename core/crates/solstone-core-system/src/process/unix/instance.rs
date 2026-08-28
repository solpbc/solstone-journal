// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::Duration;
#[cfg(any(target_os = "macos", test))]
use std::time::Instant;

use super::super::{
    CensusRow, ExecutionState, InspectResult, InstanceCensus, InstanceVerdict, ProcessBirth,
    ProcessInstance, ProcessInstanceSource, SystemProcessInstanceSource,
};
#[cfg(target_os = "linux")]
use nix::unistd::{SysconfVar, sysconf};

impl ProcessInstanceSource for SystemProcessInstanceSource {
    fn inspect(&self, pid: u32) -> InspectResult {
        inspect_native(pid)
    }

    fn census(&self) -> InstanceCensus {
        census_native()
    }

    #[cfg(target_os = "macos")]
    fn census_until(&self, deadline: Instant) -> InstanceCensus {
        census_macos_until(deadline)
    }

    #[cfg(target_os = "macos")]
    fn census_tree(&self, root_pid: u32, deadline: Option<Instant>) -> InstanceCensus {
        census_macos_tree(root_pid, deadline)
    }
}

/// Keep the creating thread alive while the child instance is live or unverifiable.
/// `PR_SET_PDEATHSIG` tracks the creating thread and would SIGKILL the child if we exit.
#[cfg(target_os = "linux")]
pub(crate) fn hold_while_instance_live(pid: u32) {
    let source = SystemProcessInstanceSource;
    let identity = loop {
        match source.inspect(pid) {
            InspectResult::Present { instance, .. } => break instance,
            InspectResult::Absent => return,
            InspectResult::Unverifiable => thread::park_timeout(Duration::from_secs(2)),
        }
    };
    while matches!(
        source.observe(&identity),
        InstanceVerdict::SameLive { .. } | InstanceVerdict::Unverifiable
    ) {
        thread::park_timeout(Duration::from_secs(2));
    }
}

#[cfg(target_os = "linux")]
fn inspect_native(pid: u32) -> InspectResult {
    inspect_linux(pid)
}

#[cfg(target_os = "macos")]
fn inspect_native(pid: u32) -> InspectResult {
    inspect_macos(pid)
}

/// iOS has neither Linux procfs nor a supported process-listing shellout.
/// iOS still has no supported process-start-time source.
#[cfg(target_os = "ios")]
fn inspect_native(_pid: u32) -> InspectResult {
    InspectResult::Unverifiable
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
fn inspect_native(_pid: u32) -> InspectResult {
    InspectResult::Unverifiable
}

#[cfg(target_os = "linux")]
fn census_native() -> InstanceCensus {
    census_linux()
}

#[cfg(target_os = "macos")]
fn census_native() -> InstanceCensus {
    census_macos()
}

#[cfg(target_os = "ios")]
fn census_native() -> InstanceCensus {
    InstanceCensus::Incomplete(Vec::new())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
fn census_native() -> InstanceCensus {
    InstanceCensus::Incomplete(Vec::new())
}

fn finalize_census(rows: Vec<CensusRow>, complete: bool) -> InstanceCensus {
    if complete {
        InstanceCensus::Complete(rows)
    } else {
        InstanceCensus::Incomplete(rows)
    }
}

#[cfg(any(target_os = "linux", test))]
struct LinuxStat {
    pid: u32,
    ppid: u32,
    pgid: i32,
    execution: ExecutionState,
    start_ticks: u64,
    zombie: bool,
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_stat(stat: &str) -> Option<LinuxStat> {
    let close = stat.rfind(')')?;
    let prefix = stat.get(..close)?.trim();
    let pid = prefix.split_whitespace().next()?.parse().ok()?;
    let fields: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    let state = fields.first()?.chars().next()?;
    let ppid = fields.get(1)?.parse().ok()?;
    let pgid = fields.get(2)?.parse().ok()?;
    let start_ticks = fields.get(19)?.parse().ok()?;
    Some(LinuxStat {
        pid,
        ppid,
        pgid,
        execution: match state {
            'T' | 't' => ExecutionState::Stopped,
            _ => ExecutionState::Running,
        },
        start_ticks,
        zombie: state == 'Z',
    })
}

#[cfg(target_os = "linux")]
fn parse_boot_time(stat: &str) -> Option<u64> {
    stat.lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .parse()
        .ok()
}

#[cfg(any(target_os = "linux", test))]
fn inspect_from_linux_stat(stat: &str, btime: u64, clk_tck: u64, uid: u32) -> InspectResult {
    let Some(parsed) = parse_linux_stat(stat) else {
        return InspectResult::Unverifiable;
    };
    if parsed.zombie {
        return InspectResult::Absent;
    }
    InspectResult::Present {
        instance: ProcessInstance {
            pid: parsed.pid,
            birth: ProcessBirth::linux(parsed.start_ticks, btime, clk_tck),
        },
        uid,
        execution: parsed.execution,
        ppid: Some(parsed.ppid),
        pgid: Some(parsed.pgid),
    }
}

#[cfg(target_os = "linux")]
fn linux_uid(pid: u32) -> Option<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn linux_boot_clock() -> Option<(u64, u64)> {
    let btime = parse_boot_time(&std::fs::read_to_string("/proc/stat").ok()?)?;
    let ticks = sysconf(SysconfVar::CLK_TCK).ok()??;
    let ticks = u64::try_from(ticks).ok().filter(|value| *value > 0)?;
    Some((btime, ticks))
}

#[cfg(target_os = "linux")]
fn inspect_linux(pid: u32) -> InspectResult {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return InspectResult::Absent;
        }
        Err(_) => return InspectResult::Unverifiable,
    };
    let Some((btime, clk_tck)) = linux_boot_clock() else {
        return InspectResult::Unverifiable;
    };
    let Some(uid) = linux_uid(pid) else {
        return InspectResult::Unverifiable;
    };
    inspect_from_linux_stat(&stat, btime, clk_tck, uid)
}

#[cfg(target_os = "linux")]
fn census_linux() -> InstanceCensus {
    let Some((btime, clk_tck)) = linux_boot_clock() else {
        return InstanceCensus::Incomplete(Vec::new());
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return InstanceCensus::Incomplete(Vec::new());
    };
    let mut rows = Vec::new();
    let mut complete = true;
    for entry in entries {
        let Ok(entry) = entry else {
            complete = false;
            continue;
        };
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                complete = false;
                continue;
            }
            Ok(stat) => {
                let Some(uid) = linux_uid(pid) else {
                    complete = false;
                    continue;
                };
                match inspect_from_linux_stat(&stat, btime, clk_tck, uid) {
                    InspectResult::Unverifiable => {
                        complete = false;
                    }
                    InspectResult::Absent => {}
                    InspectResult::Present {
                        instance,
                        uid,
                        execution,
                        ppid: Some(ppid),
                        pgid: Some(pgid),
                    } => rows.push(CensusRow {
                        instance,
                        uid,
                        ppid,
                        pgid,
                        execution,
                    }),
                    InspectResult::Present { .. } => {
                        complete = false;
                    }
                }
            }
        }
    }
    finalize_census(rows, complete)
}

#[cfg(target_os = "macos")]
fn inspect_macos(pid: u32) -> InspectResult {
    super::macos_proc::inspect_from_macos_bsd_info_result(super::macos_proc::read_bsd_info(pid))
}

#[cfg(target_os = "macos")]
fn census_macos() -> InstanceCensus {
    let output = match std::process::Command::new("/bin/ps")
        .args(["-axo", "pid="])
        .env("LC_ALL", "C")
        .output()
    {
        Ok(output) => output,
        Err(_) => return InstanceCensus::Incomplete(Vec::new()),
    };
    if !output.status.success() {
        return InstanceCensus::Incomplete(Vec::new());
    }
    let mut rows = Vec::new();
    let mut complete = true;
    for raw in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(pid) = raw.trim().parse::<u32>() else {
            complete = false;
            continue;
        };
        complete &= collect_macos_census_row(pid, &mut rows);
    }
    finalize_census(rows, complete)
}

#[cfg(target_os = "macos")]
fn census_macos_until(deadline: Instant) -> InstanceCensus {
    if Instant::now() >= deadline {
        return InstanceCensus::Incomplete(Vec::new());
    }
    let Some(pids) = list_macos_pids_until(deadline) else {
        return InstanceCensus::Incomplete(Vec::new());
    };
    let mut rows = Vec::new();
    let mut complete = true;
    for pid in pids {
        if Instant::now() >= deadline {
            return InstanceCensus::Incomplete(rows);
        }
        let Ok(pid) = u32::try_from(pid) else {
            complete = false;
            continue;
        };
        complete &= collect_macos_census_row(pid, &mut rows);
    }
    finalize_census(rows, complete)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn list_macos_pids_until(deadline: Instant) -> Option<Vec<libc::pid_t>> {
    if Instant::now() >= deadline {
        return None;
    }
    let capacity = usize::try_from(unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) })
        .ok()
        .filter(|capacity| *capacity > 0)?;
    if Instant::now() >= deadline {
        return None;
    }
    let byte_len = i32::try_from(capacity.checked_mul(std::mem::size_of::<libc::pid_t>())?).ok()?;
    let mut pids = vec![0; capacity];
    let listed =
        usize::try_from(unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), byte_len) })
            .ok()?;
    if listed == 0 || listed >= capacity {
        return None;
    }
    pids.truncate(listed);
    Some(pids)
}

#[cfg(target_os = "macos")]
fn collect_macos_census_row(pid: u32, rows: &mut Vec<CensusRow>) -> bool {
    match inspect_macos(pid) {
        InspectResult::Present {
            instance,
            uid,
            execution,
            ppid: Some(ppid),
            pgid: Some(pgid),
        } => {
            rows.push(CensusRow {
                instance,
                uid,
                ppid,
                pgid,
                execution,
            });
            true
        }
        InspectResult::Absent => true,
        InspectResult::Present { .. } | InspectResult::Unverifiable => false,
    }
}

/// Build one owned process tree through macOS's parent-scoped libproc query.
///
/// A global macOS process census is routinely incomplete for an unprivileged
/// owner because `proc_pidinfo` refuses unrelated system processes. Tree
/// termination does not need those rows: `proc_listpids(PROC_PPID_ONLY)` asks
/// the kernel for each owned node's direct children, and birth-bound
/// `inspect_macos` samples only that resulting subtree.
#[cfg(target_os = "macos")]
fn census_macos_tree(root_pid: u32, deadline: Option<Instant>) -> InstanceCensus {
    census_macos_tree_with(root_pid, deadline, inspect_macos, list_macos_child_pids)
}

#[cfg(any(target_os = "macos", test))]
fn census_macos_tree_with(
    root_pid: u32,
    deadline: Option<Instant>,
    mut inspect: impl FnMut(u32) -> InspectResult,
    mut list_children: impl FnMut(u32, Option<Instant>) -> Option<Vec<u32>>,
) -> InstanceCensus {
    let mut rows = Vec::new();
    let mut pending = vec![(root_pid, None)];
    let mut visited = std::collections::BTreeSet::new();
    while let Some((pid, enumerated_parent)) = pending.pop() {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return InstanceCensus::Incomplete(rows);
        }
        if !visited.insert(pid) {
            continue;
        }
        let (instance, uid, execution, ppid, pgid) = match inspect(pid) {
            InspectResult::Present {
                instance,
                uid,
                execution,
                ppid: Some(ppid),
                pgid: Some(pgid),
            } => (instance, uid, execution, ppid, pgid),
            InspectResult::Absent | InspectResult::Present { .. } | InspectResult::Unverifiable => {
                return InstanceCensus::Incomplete(rows);
            }
        };
        if enumerated_parent.is_some_and(|parent| ppid != parent) {
            return InstanceCensus::Incomplete(rows);
        }
        let Some(children) = list_children(pid, deadline) else {
            return InstanceCensus::Incomplete(rows);
        };
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return InstanceCensus::Incomplete(rows);
        }
        match inspect(pid) {
            InspectResult::Present {
                instance: current,
                ppid: Some(current_ppid),
                ..
            } if current.birth == instance.birth
                && enumerated_parent.is_none_or(|parent| current_ppid == parent) => {}
            InspectResult::Absent | InspectResult::Present { .. } | InspectResult::Unverifiable => {
                return InstanceCensus::Incomplete(rows);
            }
        }
        rows.push(CensusRow {
            instance,
            uid,
            ppid,
            pgid,
            execution,
        });
        pending.extend(children.into_iter().map(|child| (child, Some(pid))));
    }
    InstanceCensus::Complete(rows)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn list_macos_child_pids(parent_pid: u32, deadline: Option<Instant>) -> Option<Vec<u32>> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return None;
    }
    // Stable libproc selector from <sys/proc_info.h>. libc exposes the raw
    // proc_listpids call but not this constant. Use the raw API because
    // proc_listchildpids changes byte counts to PID counts.
    const PROC_PPID_ONLY: u32 = 6;
    let sizing_bytes =
        macos_proc_listpids_bytes(PROC_PPID_ONLY, parent_pid, std::ptr::null_mut(), 0)?;
    collect_macos_child_pids(sizing_bytes, deadline, |pids, buffer_bytes| {
        macos_proc_listpids_bytes(
            PROC_PPID_ONLY,
            parent_pid,
            pids.as_mut_ptr().cast(),
            buffer_bytes,
        )
    })
}

#[cfg(any(target_os = "macos", test))]
fn collect_macos_child_pids(
    sizing_bytes: usize,
    deadline: Option<Instant>,
    mut fill: impl FnMut(&mut [libc::pid_t], i32) -> Option<usize>,
) -> Option<Vec<u32>> {
    let pid_size = std::mem::size_of::<libc::pid_t>();
    let mut capacity = sizing_bytes.div_ceil(pid_size).saturating_add(16).max(16);
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return None;
        }
        let buffer_bytes = capacity
            .checked_mul(pid_size)
            .and_then(|bytes| i32::try_from(bytes).ok())?;
        let mut pids = vec![0; capacity];
        let written_bytes = fill(&mut pids, buffer_bytes)?;
        if written_bytes % pid_size != 0 || written_bytes > buffer_bytes as usize {
            return None;
        }
        let written = written_bytes / pid_size;
        if written < capacity {
            pids.truncate(written);
            return pids
                .into_iter()
                .filter(|pid| *pid > 0)
                .map(|pid| u32::try_from(pid).ok())
                .collect();
        }
        capacity = capacity.checked_mul(2)?;
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn macos_proc_listpids_bytes(
    selector: u32,
    selector_value: u32,
    buffer: *mut std::ffi::c_void,
    buffer_bytes: i32,
) -> Option<usize> {
    // Apple's wrapper maps the underlying syscall's -1 result to zero. Clear
    // errno immediately before the call so an empty result can be
    // distinguished from that collapsed error without trusting stale errno.
    let errno = unsafe { libc::__error() };
    if errno.is_null() {
        return None;
    }
    unsafe { *errno = 0 };
    let written =
        unsafe { libc::proc_listpids(selector, selector_value, buffer.cast(), buffer_bytes) };
    let call_errno = unsafe { *errno };
    interpret_macos_proc_listpids_result(written, call_errno)
}

#[cfg(any(target_os = "macos", test))]
fn interpret_macos_proc_listpids_result(written: i32, errno: i32) -> Option<usize> {
    if written == 0 && errno != 0 {
        None
    } else {
        usize::try_from(written).ok()
    }
}

/// macOS orphan-candidate table. Separate from the birth/liveness sample because
/// `command=` is an unbounded trailing field and matches custom argv[0] titles.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MacosSweepRow {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub command: String,
}

/// Parse a macOS sweep table from `ps` text. Any malformed non-empty line fails the whole table.
#[cfg(any(target_os = "macos", test))]
fn macos_sweep_table_from_text(text: &str) -> Option<Vec<MacosSweepRow>> {
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        rows.push(parse_macos_sweep_row(line)?);
    }
    Some(rows)
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_sweep_table() -> Option<Vec<MacosSweepRow>> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,uid=,command="])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    macos_sweep_table_from_text(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_sweep_row(line: &str) -> Option<MacosSweepRow> {
    let mut fields = line.split_whitespace();
    Some(MacosSweepRow {
        pid: fields.next()?.parse().ok()?,
        ppid: fields.next()?.parse().ok()?,
        uid: fields.next()?.parse().ok()?,
        command: fields.collect::<Vec<_>>().join(" "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn linux_stat(pid: u32, state: char, ppid: u32, pgrp: i32, start_ticks: u64) -> String {
        let mut fields = vec!["0".to_owned(); 22];
        fields[0] = state.to_string();
        fields[1] = ppid.to_string();
        fields[2] = pgrp.to_string();
        fields[19] = start_ticks.to_string();
        format!("{pid} (comm) {}", fields.join(" "))
    }

    fn present_running() -> InspectResult {
        InspectResult::Present {
            instance: ProcessInstance {
                pid: 7,
                birth: ProcessBirth::linux(10, 100, 100),
            },
            uid: 501,
            execution: ExecutionState::Running,
            ppid: Some(1),
            pgid: Some(7),
        }
    }

    #[test]
    fn process_instance_wire_round_trip_preserves_the_birth_token() {
        let instance = ProcessInstance {
            pid: 7,
            birth: ProcessBirth::linux(1234, 1_000, 100),
        };
        let decoded: ProcessInstance =
            serde_json::from_slice(&serde_json::to_vec(&instance).expect("serialize instance"))
                .expect("deserialize instance");
        assert_eq!(decoded, instance);
        assert_eq!(
            decoded.birth.epoch_seconds(),
            instance.birth.epoch_seconds()
        );
    }

    #[test]
    fn macos_proc_listpids_zero_is_empty_only_without_errno() {
        assert_eq!(interpret_macos_proc_listpids_result(0, 0), Some(0));
        assert_eq!(interpret_macos_proc_listpids_result(0, libc::EPERM), None);
        assert_eq!(interpret_macos_proc_listpids_result(-1, 0), None);
        assert_eq!(
            interpret_macos_proc_listpids_result(8, libc::EINTR),
            Some(8)
        );
    }

    #[test]
    fn macos_child_pid_collection_grows_on_saturation_and_rejects_partial_bytes() {
        let pid_size = std::mem::size_of::<libc::pid_t>();
        let mut rounds = 0;
        let pids = collect_macos_child_pids(pid_size, None, |buffer, _| {
            rounds += 1;
            if rounds == 1 {
                for (index, pid) in buffer.iter_mut().enumerate() {
                    *pid = i32::try_from(index + 1).expect("synthetic PID fits i32");
                }
                Some(std::mem::size_of_val(buffer))
            } else {
                buffer[0] = 41;
                buffer[1] = 42;
                Some(2 * pid_size)
            }
        })
        .expect("saturated buffer grows");
        assert_eq!(rounds, 2);
        assert_eq!(pids, vec![41, 42]);
        assert!(
            collect_macos_child_pids(pid_size, None, |_, _| Some(1)).is_none(),
            "partial PID bytes must fail closed"
        );
    }

    #[test]
    fn macos_child_pid_collection_honors_an_expired_deadline() {
        assert!(
            collect_macos_child_pids(4, Some(Instant::now()), |_, _| {
                panic!("expired collection must not call the native fill")
            })
            .is_none()
        );
    }

    #[test]
    fn macos_targeted_census_rejects_parent_mismatch_and_birth_reuse() {
        use std::collections::{BTreeMap, VecDeque};

        fn present(pid: u32, birth: u64, ppid: u32) -> InspectResult {
            InspectResult::Present {
                instance: ProcessInstance {
                    pid,
                    birth: ProcessBirth::linux(birth, 1, 100),
                },
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(ppid),
                pgid: Some(i32::try_from(pid).expect("synthetic PGID fits i32")),
            }
        }

        let mut mismatch = BTreeMap::from([
            (10, VecDeque::from([present(10, 1, 1), present(10, 1, 1)])),
            (11, VecDeque::from([present(11, 2, 99)])),
        ]);
        let mismatch = census_macos_tree_with(
            10,
            None,
            |pid| {
                mismatch
                    .get_mut(&pid)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or(InspectResult::Unverifiable)
            },
            |pid, _| Some(if pid == 10 { vec![11] } else { Vec::new() }),
        );
        assert!(matches!(mismatch, InstanceCensus::Incomplete(_)));

        let mut reused = VecDeque::from([present(10, 1, 1), present(10, 2, 1)]);
        let reused = census_macos_tree_with(
            10,
            None,
            |_| reused.pop_front().unwrap_or(InspectResult::Unverifiable),
            |_, _| Some(vec![11]),
        );
        assert!(matches!(reused, InstanceCensus::Incomplete(rows) if rows.is_empty()));
    }

    #[test]
    fn case1_well_formed_linux_stat_is_present_running() {
        let stat = linux_stat(42, 'S', 1, 42, 1234);
        match inspect_from_linux_stat(&stat, 1_000, 100, 501) {
            InspectResult::Present {
                instance,
                uid,
                execution,
                ppid,
                pgid,
            } => {
                assert_eq!(instance.pid, 42);
                assert_eq!(instance.birth, ProcessBirth::linux(1234, 1_000, 100));
                assert_eq!(uid, 501);
                assert_eq!(execution, ExecutionState::Running);
                assert_eq!(ppid, Some(1));
                assert_eq!(pgid, Some(42));
            }
            other => panic!("expected Present, got {other:?}"),
        }
    }

    #[test]
    fn case2_linux_stopped_state_is_present_stopped() {
        let stat = linux_stat(9, 'T', 1, 9, 50);
        match inspect_from_linux_stat(&stat, 1_000, 100, 501) {
            InspectResult::Present { execution, .. } => {
                assert_eq!(execution, ExecutionState::Stopped);
            }
            other => panic!("expected Present, got {other:?}"),
        }
        let lowercase = linux_stat(9, 't', 1, 9, 50);
        match inspect_from_linux_stat(&lowercase, 1_000, 100, 501) {
            InspectResult::Present { execution, .. } => {
                assert_eq!(execution, ExecutionState::Stopped);
            }
            other => panic!("expected Present, got {other:?}"),
        }
    }

    #[test]
    fn case3_observe_birth_mismatch_is_not_same_or_exited() {
        let expected = ProcessInstance {
            pid: 7,
            birth: ProcessBirth::linux(1, 100, 100),
        };
        let source = FakeSource {
            result: present_running(),
        };
        assert_eq!(source.observe(&expected), InstanceVerdict::NotSameOrExited);
    }

    #[test]
    fn case4_observe_absent_is_not_same_or_exited() {
        let expected = ProcessInstance {
            pid: 7,
            birth: ProcessBirth::linux(10, 100, 100),
        };
        let source = FakeSource {
            result: InspectResult::Absent,
        };
        assert_eq!(source.observe(&expected), InstanceVerdict::NotSameOrExited);
    }

    #[test]
    fn case5_malformed_linux_stat_is_unverifiable() {
        assert_eq!(
            inspect_from_linux_stat("no-close-paren 1 2 3", 1_000, 100, 501),
            InspectResult::Unverifiable
        );
        assert_eq!(
            inspect_from_linux_stat("1 (comm) S 1 1", 1_000, 100, 501),
            InspectResult::Unverifiable
        );
        assert!(parse_linux_stat("1 (comm").is_none());
    }

    #[test]
    fn case6_zombie_linux_stat_is_absent() {
        let stat = linux_stat(4, 'Z', 1, 4, 9);
        assert_eq!(
            inspect_from_linux_stat(&stat, 1_000, 100, 501),
            InspectResult::Absent
        );
    }

    #[test]
    fn case7_incomplete_census_is_not_complete_empty() {
        let row = CensusRow {
            instance: ProcessInstance {
                pid: 3,
                birth: ProcessBirth::linux(1, 1, 100),
            },
            uid: 501,
            ppid: 1,
            pgid: 3,
            execution: ExecutionState::Running,
        };
        assert_eq!(
            finalize_census(Vec::new(), false),
            InstanceCensus::Incomplete(Vec::new())
        );
        assert_ne!(
            finalize_census(vec![row], false),
            InstanceCensus::Complete(Vec::new())
        );
        assert_eq!(
            finalize_census(vec![row], true),
            InstanceCensus::Complete(vec![row])
        );
    }

    #[test]
    fn case8_observe_unverifiable_when_inspect_is_unverifiable() {
        let expected = ProcessInstance {
            pid: 7,
            birth: ProcessBirth::linux(10, 100, 100),
        };
        let source = FakeSource {
            result: InspectResult::Unverifiable,
        };
        assert_eq!(source.observe(&expected), InstanceVerdict::Unverifiable);
        let matched = ProcessInstance {
            pid: 7,
            birth: ProcessBirth::linux(10, 100, 100),
        };
        let live = FakeSource {
            result: present_running(),
        };
        assert_eq!(
            live.observe(&matched),
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running
            }
        );
    }

    #[test]
    fn linux_birth_equality_ignores_clk_tck() {
        assert_eq!(
            ProcessBirth::linux(8, 20, 100),
            ProcessBirth::linux(8, 20, 250)
        );
        assert_ne!(
            ProcessBirth::linux(8, 20, 100),
            ProcessBirth::linux(9, 20, 100)
        );
    }

    #[test]
    fn macos_sweep_row_keeps_full_command() {
        let row = parse_macos_sweep_row("10 1 501 /usr/bin/journal:think extra").unwrap();
        assert_eq!(row.pid, 10);
        assert_eq!(row.ppid, 1);
        assert_eq!(row.uid, 501);
        assert_eq!(row.command, "/usr/bin/journal:think extra");
    }

    #[test]
    fn macos_sweep_table_from_text_returns_all_well_formed_rows() {
        let text = "10 1 501 /usr/bin/journal:think extra\n\n11 1 501 /usr/bin/other\n";
        let rows = macos_sweep_table_from_text(text).expect("well-formed table");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pid, 10);
        assert_eq!(rows[0].command, "/usr/bin/journal:think extra");
        assert_eq!(rows[1].pid, 11);
    }

    #[test]
    fn macos_sweep_table_from_text_malformed_row_fails_the_table() {
        let text =
            "10 1 501 /usr/bin/journal:think extra\nbad 1 501 /bin/x\n11 1 501 /usr/bin/other\n";
        assert!(macos_sweep_table_from_text(text).is_none());
    }
}
