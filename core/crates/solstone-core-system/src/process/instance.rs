// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Coherent process-instance observation: one native sample per inspect/census.

#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(any(target_os = "macos", test))]
use chrono::{DateTime, Local, LocalResult, NaiveDateTime, TimeZone};
#[cfg(target_os = "linux")]
use nix::unistd::{SysconfVar, sysconf};

/// PID together with the native birth token observed for that PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessInstance {
    pub pid: u32,
    pub birth: ProcessBirth,
}

/// Opaque start-time identity. Equality is exact (tick-level on Linux,
/// second-level lstart on macOS). [`ProcessBirth::epoch_seconds`] is only for
/// supervisor pid-file identity, which applies `START_TIME_TOLERANCE_SECONDS`.
#[derive(Debug, Clone, Copy)]
pub struct ProcessBirth {
    inner: ProcessBirthInner,
}

#[derive(Debug, Clone, Copy)]
enum ProcessBirthInner {
    Linux {
        start_ticks: u64,
        btime: u64,
        clk_tck: u64,
    },
    #[allow(dead_code)]
    Macos { epoch_seconds: i64 },
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
                ProcessBirthInner::Macos {
                    epoch_seconds: left,
                },
                ProcessBirthInner::Macos {
                    epoch_seconds: right,
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
            ProcessBirthInner::Macos { epoch_seconds } => epoch_seconds as f64,
        }
    }

    pub(crate) fn linux(start_ticks: u64, btime: u64, clk_tck: u64) -> Self {
        Self {
            inner: ProcessBirthInner::Linux {
                start_ticks,
                btime,
                clk_tck,
            },
        }
    }

    #[allow(dead_code)]
    pub(crate) fn macos(epoch_seconds: i64) -> Self {
        Self {
            inner: ProcessBirthInner::Macos { epoch_seconds },
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "macos", test))]
fn parse_macos_lstart(lstart: &str) -> Option<i64> {
    let fields: Vec<_> = lstart.split_whitespace().collect();
    let [weekday, month, day, time, year] = fields.as_slice() else {
        return None;
    };
    let day: u8 = day.parse().ok()?;
    let normalized = format!("{weekday} {month} {day:02} {time} {year}");
    let naive = NaiveDateTime::parse_from_str(&normalized, "%a %b %d %H:%M:%S %Y").ok()?;
    epoch_seconds_from_local_result(Local.from_local_datetime(&naive))
}

/// Resolve DST fall-back ambiguity to the earliest instant so identity writes
/// and later identity checks make the same PID-reuse-safe comparison. A
/// nonexistent spring-forward local time cannot name a real process, so it
/// still fails closed.
#[cfg(any(target_os = "macos", test))]
fn epoch_seconds_from_local_result<Tz: TimeZone>(result: LocalResult<DateTime<Tz>>) -> Option<i64> {
    match result {
        LocalResult::Single(started) | LocalResult::Ambiguous(started, _) => {
            Some(started.timestamp())
        }
        LocalResult::None => None,
    }
}

#[cfg(any(target_os = "macos", test))]
fn execution_from_state_token(state: &str) -> Option<(ExecutionState, bool)> {
    let ch = state.chars().next()?;
    Some(match ch {
        'Z' => (ExecutionState::Running, true),
        'T' | 't' => (ExecutionState::Stopped, false),
        _ => (ExecutionState::Running, false),
    })
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_inspect_line(line: &str) -> Option<InspectResult> {
    let mut parts = line.split_whitespace();
    let pid: u32 = parts.next()?.parse().ok()?;
    let state = parts.next()?;
    let pgid: i32 = parts.next()?.parse().ok()?;
    let lstart = parts.collect::<Vec<_>>().join(" ");
    let epoch_seconds = parse_macos_lstart(&lstart)?;
    let (execution, zombie) = execution_from_state_token(state)?;
    if zombie {
        return Some(InspectResult::Absent);
    }
    Some(InspectResult::Present {
        instance: ProcessInstance {
            pid,
            birth: ProcessBirth::macos(epoch_seconds),
        },
        execution,
        ppid: None,
        pgid: Some(pgid),
    })
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_census_line(line: &str) -> Option<CensusRow> {
    let mut parts = line.split_whitespace();
    let pid: u32 = parts.next()?.parse().ok()?;
    let ppid: u32 = parts.next()?.parse().ok()?;
    let pgid: i32 = parts.next()?.parse().ok()?;
    let state = parts.next()?;
    let lstart = parts.collect::<Vec<_>>().join(" ");
    let epoch_seconds = parse_macos_lstart(&lstart)?;
    let (execution, zombie) = execution_from_state_token(state)?;
    if zombie {
        return None;
    }
    Some(CensusRow {
        instance: ProcessInstance {
            pid,
            birth: ProcessBirth::macos(epoch_seconds),
        },
        ppid,
        pgid,
        execution,
    })
}

#[cfg(target_os = "macos")]
fn inspect_macos(pid: u32) -> InspectResult {
    let output = match std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "pid=,state=,pgid=,lstart="])
        .env("LC_ALL", "C")
        .output()
    {
        Ok(output) => output,
        Err(_) => return InspectResult::Unverifiable,
    };
    if !output.status.success() {
        return InspectResult::Absent;
    }
    let line = String::from_utf8_lossy(&output.stdout);
    let line = line.trim();
    if line.is_empty() {
        return InspectResult::Absent;
    }
    parse_macos_inspect_line(line).unwrap_or(InspectResult::Unverifiable)
}

#[cfg(target_os = "macos")]
fn census_macos() -> InstanceCensus {
    let output = match std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pgid=,state=,lstart="])
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
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_macos_census_line(line) {
            Some(row) => rows.push(row),
            None => complete = false,
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
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_macos_sweep_row)
            .collect(),
    )
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
    use chrono::{Local, LocalResult, TimeZone, Utc};

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
    fn macos_lstart_parser_accepts_c_locale_timestamp() {
        let expected = Local
            .with_ymd_and_hms(2026, 8, 10, 12, 34, 56)
            .single()
            .expect("unambiguous summer time")
            .timestamp();
        assert_eq!(
            parse_macos_lstart("Mon Aug 10 12:34:56 2026").expect("parse lstart"),
            expected
        );
    }

    #[test]
    fn macos_lstart_parser_rejects_malformed_timestamp() {
        assert!(parse_macos_lstart("not an lstart timestamp").is_none());
    }

    #[test]
    fn macos_lstart_parser_resolves_ambiguous_local_time_deterministically() {
        let earliest = Utc
            .with_ymd_and_hms(2026, 11, 1, 7, 30, 0)
            .single()
            .expect("utc time");
        let latest = Utc
            .with_ymd_and_hms(2026, 11, 1, 8, 30, 0)
            .single()
            .expect("utc time");
        assert_eq!(
            epoch_seconds_from_local_result(LocalResult::Ambiguous(earliest, latest))
                .expect("resolve ambiguity"),
            earliest.timestamp()
        );
    }

    #[test]
    fn macos_lstart_parser_rejects_nonexistent_local_time() {
        let nonexistent: LocalResult<chrono::DateTime<Utc>> = LocalResult::None;
        assert!(epoch_seconds_from_local_result(nonexistent).is_none());
    }

    #[test]
    fn macos_inspect_line_puts_lstart_last() {
        let line = "99 T 99 Mon Aug 10 12:34:56 2026";
        match parse_macos_inspect_line(line) {
            Some(InspectResult::Present {
                instance,
                execution,
                pgid,
                ..
            }) => {
                assert_eq!(instance.pid, 99);
                assert_eq!(execution, ExecutionState::Stopped);
                assert_eq!(pgid, Some(99));
                assert_eq!(
                    instance.birth,
                    ProcessBirth::macos(parse_macos_lstart("Mon Aug 10 12:34:56 2026").unwrap())
                );
            }
            other => panic!("expected Present, got {other:?}"),
        }
        assert!(parse_macos_inspect_line("99 S 99 not-an-lstart").is_none());
        let census = parse_macos_census_line("99 1 99 T Mon Aug 10 12:34:56 2026").unwrap();
        assert_eq!(census.instance.pid, 99);
        assert_eq!(census.ppid, 1);
        assert_eq!(census.pgid, 99);
        assert_eq!(census.execution, ExecutionState::Stopped);
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
}
