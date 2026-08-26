// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Coherent process-instance observation: one native sample per inspect/census.

#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use nix::unistd::{SysconfVar, sysconf};

/// PID together with the native birth token observed for that PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessInstance {
    pub pid: u32,
    pub birth: ProcessBirth,
}

/// Opaque start-time identity. Equality is exact (tick-level on Linux,
/// microsecond-level proc_bsdinfo on macOS). [`ProcessBirth::epoch_seconds`] is only for
/// supervisor pid-file identity, which applies `START_TIME_TOLERANCE_SECONDS`.
#[derive(Debug, Clone, Copy)]
pub struct ProcessBirth {
    inner: ProcessBirthInner,
}

#[derive(Debug, Clone, Copy)]
enum ProcessBirthInner {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Linux {
        start_ticks: u64,
        btime: u64,
        clk_tck: u64,
    },
    #[allow(dead_code)]
    Macos { epoch_micros: i64 },
}

impl PartialEq for ProcessBirth {
    fn eq(&self, other: &Self) -> bool {
        match (self.inner, other.inner) {
            (
                ProcessBirthInner::Linux {
                    start_ticks: left_ticks,
                    btime: left_btime,
                    ..
                },
                ProcessBirthInner::Linux {
                    start_ticks: right_ticks,
                    btime: right_btime,
                    ..
                },
            ) => left_ticks == right_ticks && left_btime == right_btime,
            (
                ProcessBirthInner::Macos { epoch_micros: left },
                ProcessBirthInner::Macos {
                    epoch_micros: right,
                },
            ) => left == right,
            _ => false,
        }
    }
}

impl Eq for ProcessBirth {}

impl ProcessBirth {
    pub fn epoch_seconds(&self) -> f64 {
        match self.inner {
            ProcessBirthInner::Linux {
                start_ticks,
                btime,
                clk_tck,
            } => btime as f64 + start_ticks as f64 / clk_tck as f64,
            ProcessBirthInner::Macos { epoch_micros } => epoch_micros as f64 / 1_000_000.0,
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn linux(start_ticks: u64, btime: u64, clk_tck: u64) -> Self {
        Self {
            inner: ProcessBirthInner::Linux {
                start_ticks,
                btime,
                clk_tck,
            },
        }
    }

    #[allow(dead_code)]
    pub(crate) fn macos(epoch_micros: i64) -> Self {
        Self {
            inner: ProcessBirthInner::Macos { epoch_micros },
        }
    }
}

/// Live execution state. Zombies are not live and surface as [`InspectResult::Absent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    Running,
    Stopped,
}

/// Result of comparing a remembered identity against one native sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceVerdict {
    SameLive { execution: ExecutionState },
    NotSameOrExited,
    Unverifiable,
}

/// One PID sample without a remembered identity to compare against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectResult {
    Present {
        instance: ProcessInstance,
        execution: ExecutionState,
        ppid: Option<u32>,
        pgid: Option<i32>,
    },
    Absent,
    Unverifiable,
}

/// One live row from a process-table sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CensusRow {
    pub instance: ProcessInstance,
    pub ppid: u32,
    pub pgid: i32,
    pub execution: ExecutionState,
}

/// Process-table sample. Incomplete must never be treated as an empty table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceCensus {
    Complete(Vec<CensusRow>),
    Incomplete(Vec<CensusRow>),
}

/// Injected process-instance source. Production uses [`SystemProcessInstanceSource`].
pub trait ProcessInstanceSource: Send + Sync {
    fn inspect(&self, pid: u32) -> InspectResult;
    fn census(&self) -> InstanceCensus;

    fn observe(&self, expected: &ProcessInstance) -> InstanceVerdict {
        match self.inspect(expected.pid) {
            InspectResult::Unverifiable => InstanceVerdict::Unverifiable,
            InspectResult::Absent => InstanceVerdict::NotSameOrExited,
            InspectResult::Present {
                instance,
                execution,
                ..
            } if instance.birth == expected.birth => InstanceVerdict::SameLive { execution },
            InspectResult::Present { .. } => InstanceVerdict::NotSameOrExited,
        }
    }
}

/// Native observer for the current target.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProcessInstanceSource;

impl ProcessInstanceSource for SystemProcessInstanceSource {
    fn inspect(&self, pid: u32) -> InspectResult {
        inspect_native(pid)
    }

    fn census(&self) -> InstanceCensus {
        census_native()
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
fn inspect_from_linux_stat(stat: &str, btime: u64, clk_tck: u64) -> InspectResult {
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
        execution: parsed.execution,
        ppid: Some(parsed.ppid),
        pgid: Some(parsed.pgid),
    }
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
    inspect_from_linux_stat(&stat, btime, clk_tck)
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
            Ok(stat) => match inspect_from_linux_stat(&stat, btime, clk_tck) {
                InspectResult::Unverifiable => {
                    complete = false;
                }
                InspectResult::Absent => {}
                InspectResult::Present {
                    instance,
                    execution,
                    ppid: Some(ppid),
                    pgid: Some(pgid),
                } => rows.push(CensusRow {
                    instance,
                    ppid,
                    pgid,
                    execution,
                }),
                InspectResult::Present { .. } => {
                    complete = false;
                }
            },
        }
    }
    finalize_census(rows, complete)
}

#[cfg(target_os = "macos")]
fn inspect_macos(pid: u32) -> InspectResult {
    match super::macos_proc::read_bsd_info(pid) {
        Ok(info) => super::macos_proc::inspect_from_macos_bsd_info(info),
        Err(_) => InspectResult::Unverifiable,
    }
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
        match inspect_macos(pid) {
            InspectResult::Present {
                instance,
                execution,
                ppid: Some(ppid),
                pgid: Some(pgid),
            } => {
                rows.push(CensusRow {
                    instance,
                    ppid,
                    pgid,
                    execution,
                });
            }
            InspectResult::Absent => {}
            InspectResult::Present { .. } | InspectResult::Unverifiable => complete = false,
        }
    }
    finalize_census(rows, complete)
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
            execution: ExecutionState::Running,
            ppid: Some(1),
            pgid: Some(7),
        }
    }

    #[test]
    fn case1_well_formed_linux_stat_is_present_running() {
        let stat = linux_stat(42, 'S', 1, 42, 1234);
        match inspect_from_linux_stat(&stat, 1_000, 100) {
            InspectResult::Present {
                instance,
                execution,
                ppid,
                pgid,
            } => {
                assert_eq!(instance.pid, 42);
                assert_eq!(instance.birth, ProcessBirth::linux(1234, 1_000, 100));
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
        match inspect_from_linux_stat(&stat, 1_000, 100) {
            InspectResult::Present { execution, .. } => {
                assert_eq!(execution, ExecutionState::Stopped);
            }
            other => panic!("expected Present, got {other:?}"),
        }
        let lowercase = linux_stat(9, 't', 1, 9, 50);
        match inspect_from_linux_stat(&lowercase, 1_000, 100) {
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
            inspect_from_linux_stat("no-close-paren 1 2 3", 1_000, 100),
            InspectResult::Unverifiable
        );
        assert_eq!(
            inspect_from_linux_stat("1 (comm) S 1 1", 1_000, 100),
            InspectResult::Unverifiable
        );
        assert!(parse_linux_stat("1 (comm").is_none());
    }

    #[test]
    fn case6_zombie_linux_stat_is_absent() {
        let stat = linux_stat(4, 'Z', 1, 4, 9);
        assert_eq!(
            inspect_from_linux_stat(&stat, 1_000, 100),
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
