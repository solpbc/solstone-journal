// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::path::Path;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProcessBirth {
    LinuxStartTicks(u64),
    DarwinStart(String),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub birth: ProcessBirth,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CpuBaseline {
    ticks: u64,
    sampled_at: f64,
}

/// Reason an observation could not be obtained without treating the process as
/// absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessUnavailableReason {
    UnsupportedPlatform,
    Io,
    Parse,
}

/// A bounded, privacy-preserving process observation.
#[derive(Clone, Debug, PartialEq)]
pub enum ProcessSample {
    Live {
        identity: ProcessIdentity,
        rss_bytes: u64,
        cpu_percent: f64,
    },
    Missing,
    AccessDenied,
    Zombie,
    Unavailable {
        reason: ProcessUnavailableReason,
    },
}

/// Observe only the supplied PID. Implementations never enumerate command
/// arguments, environment, process trees, or perform destructive actions.
pub trait ProcessObserver {
    fn sample(&mut self, pid: u32, monotonic_seconds: f64) -> ProcessSample;
    fn forget(&mut self, _pid: u32) {}
}

/// Construct the host backend; unsupported hosts return `Unavailable`.
#[must_use]
pub fn platform_observer() -> PlatformProcessObserver {
    PlatformProcessObserver::default()
}

/// Small platform facade that keeps cfg-specific implementation private.
#[derive(Default)]
pub struct PlatformProcessObserver {
    baselines: BTreeMap<ProcessIdentity, CpuBaseline>,
}

impl ProcessObserver for PlatformProcessObserver {
    #[cfg(target_os = "linux")]
    fn sample(&mut self, pid: u32, monotonic_seconds: f64) -> ProcessSample {
        if !native_pid_is_representable(pid) {
            self.forget(pid);
            return ProcessSample::Unavailable {
                reason: ProcessUnavailableReason::Parse,
            };
        }
        let sample = linux_sample(pid, monotonic_seconds, &mut self.baselines);
        if !matches!(sample, ProcessSample::Live { .. }) {
            self.forget(pid);
        }
        sample
    }

    fn forget(&mut self, pid: u32) {
        self.baselines.retain(|identity, _| identity.pid != pid);
    }

    #[cfg(target_os = "macos")]
    fn sample(&mut self, pid: u32, monotonic_seconds: f64) -> ProcessSample {
        if !native_pid_is_representable(pid) {
            self.forget(pid);
            return ProcessSample::Unavailable {
                reason: ProcessUnavailableReason::Parse,
            };
        }
        let sample = macos_sample(pid, monotonic_seconds, &mut self.baselines);
        if !matches!(sample, ProcessSample::Live { .. }) {
            self.forget(pid);
        }
        sample
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn sample(&mut self, _pid: u32, _monotonic_seconds: f64) -> ProcessSample {
        ProcessSample::Unavailable {
            reason: ProcessUnavailableReason::UnsupportedPlatform,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn native_pid_is_representable(pid: u32) -> bool {
    i32::try_from(pid).is_ok()
}

#[cfg(target_os = "linux")]
fn linux_sample(
    pid: u32,
    monotonic_seconds: f64,
    baselines: &mut BTreeMap<ProcessIdentity, CpuBaseline>,
) -> ProcessSample {
    linux_sample_at(Path::new("/proc"), pid, monotonic_seconds, baselines)
}

#[cfg(target_os = "linux")]
fn linux_sample_at(
    proc_root: &Path,
    pid: u32,
    monotonic_seconds: f64,
    baselines: &mut BTreeMap<ProcessIdentity, CpuBaseline>,
) -> ProcessSample {
    let root = proc_root.join(pid.to_string());
    let status = match std::fs::read_to_string(root.join("status")) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProcessSample::Missing;
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return ProcessSample::AccessDenied;
        }
        Err(_) => {
            return ProcessSample::Unavailable {
                reason: ProcessUnavailableReason::Io,
            };
        }
    };
    if linux_state_is_zombie(&status) {
        return ProcessSample::Zombie;
    }
    let rss_kib = linux_rss_kib(&status);
    let stat = match std::fs::read_to_string(root.join("stat")) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProcessSample::Missing;
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return ProcessSample::AccessDenied;
        }
        Err(_) => {
            return ProcessSample::Unavailable {
                reason: ProcessUnavailableReason::Io,
            };
        }
    };
    let Some((ticks, start_ticks)) = linux_cpu_ticks(&stat) else {
        return ProcessSample::Unavailable {
            reason: ProcessUnavailableReason::Parse,
        };
    };
    let identity = ProcessIdentity {
        pid,
        birth: ProcessBirth::LinuxStartTicks(start_ticks),
    };
    baselines.retain(|known, _| known.pid != pid || known == &identity);
    let percent = baselines
        .insert(
            identity.clone(),
            CpuBaseline {
                ticks,
                sampled_at: monotonic_seconds,
            },
        )
        .and_then(|old| {
            let elapsed = monotonic_seconds - old.sampled_at;
            (elapsed > 0.0)
                .then(|| ((ticks.saturating_sub(old.ticks)) as f64 / 100.0) / elapsed * 100.0)
        })
        .unwrap_or(0.0);
    ProcessSample::Live {
        identity,
        rss_bytes: rss_kib.unwrap_or(0).saturating_mul(1024),
        cpu_percent: percent,
    }
}

#[cfg(target_os = "linux")]
fn linux_state_is_zombie(status: &str) -> bool {
    status.lines().any(|line| line.starts_with("State:\tZ"))
}
#[cfg(target_os = "linux")]
fn linux_rss_kib(status: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}
#[cfg(target_os = "linux")]
fn linux_cpu_ticks(stat: &str) -> Option<(u64, u64)> {
    let close = stat.rfind(')')?;
    let fields: Vec<_> = stat[close + 2..].split_whitespace().collect();
    Some((
        fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?,
        fields.get(19)?.parse::<u64>().ok()?,
    ))
}

#[cfg(target_os = "macos")]
fn macos_sample(
    pid: u32,
    monotonic_seconds: f64,
    baselines: &mut BTreeMap<ProcessIdentity, CpuBaseline>,
) -> ProcessSample {
    let output = match std::process::Command::new("/bin/ps")
        .env("LC_ALL", "C")
        .args(["-o", "state=,rss=,%cpu=,lstart=", "-p", &pid.to_string()])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return ProcessSample::AccessDenied;
        }
        Err(_) => {
            return ProcessSample::Unavailable {
                reason: ProcessUnavailableReason::Io,
            };
        }
    };
    if !output.status.success() {
        return ProcessSample::Missing;
    }
    macos_parse_ps_bytes(pid, &output.stdout, monotonic_seconds, baselines)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_parse_ps_bytes(
    pid: u32,
    output: &[u8],
    monotonic_seconds: f64,
    baselines: &mut BTreeMap<ProcessIdentity, CpuBaseline>,
) -> ProcessSample {
    let Ok(output) = std::str::from_utf8(output) else {
        baselines.retain(|identity, _| identity.pid != pid);
        return ProcessSample::Unavailable {
            reason: ProcessUnavailableReason::Parse,
        };
    };
    macos_parse_ps(pid, output, monotonic_seconds, baselines)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_parse_ps(
    pid: u32,
    output: &str,
    monotonic_seconds: f64,
    baselines: &mut BTreeMap<ProcessIdentity, CpuBaseline>,
) -> ProcessSample {
    let Some(line) = output.lines().next() else {
        return ProcessSample::Missing;
    };
    let mut fields = line.split_whitespace();
    let Some(state) = fields.next() else {
        return ProcessSample::Missing;
    };
    if state.starts_with('Z') {
        return ProcessSample::Zombie;
    }
    match (
        fields.next().and_then(|value| value.parse::<u64>().ok()),
        fields.next().and_then(|value| value.parse::<f64>().ok()),
    ) {
        (Some(rss), Some(cpu_percent)) => {
            let birth = fields.collect::<Vec<_>>().join(" ");
            if birth.is_empty() {
                return ProcessSample::Unavailable {
                    reason: ProcessUnavailableReason::Parse,
                };
            }
            let identity = ProcessIdentity {
                pid,
                birth: ProcessBirth::DarwinStart(birth),
            };
            baselines.retain(|known, _| known.pid != pid || known == &identity);
            baselines.insert(
                identity.clone(),
                CpuBaseline {
                    ticks: 0,
                    sampled_at: monotonic_seconds,
                },
            );
            ProcessSample::Live {
                identity,
                rss_bytes: rss.saturating_mul(1024),
                cpu_percent,
            }
        }
        _ => ProcessSample::Unavailable {
            reason: ProcessUnavailableReason::Parse,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReductionSample, TopState, cleanup_processes};
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_fixture_parser_handles_live_zombie_and_missing_without_panicking() {
        let root = std::env::temp_dir().join(format!("solstone-top-proc-{}", std::process::id()));
        let pid_root = root.join("201");
        std::fs::create_dir_all(&pid_root).unwrap();
        std::fs::write(
            pid_root.join("status"),
            "Name:\tx\nState:\tS (sleeping)\nVmRSS:\t6144 kB\n",
        )
        .unwrap();
        std::fs::write(
            pid_root.join("stat"),
            "201 (x with spaces) S 0 0 0 0 0 0 0 0 0 0 100 20 0 0 0 0 0 0 777",
        )
        .unwrap();
        let mut baselines = BTreeMap::new();
        assert!(matches!(
            linux_sample_at(&root, 201, 1.0, &mut baselines),
            ProcessSample::Live {
                rss_bytes: 6_291_456,
                ..
            }
        ));
        std::fs::write(pid_root.join("status"), "State:\tZ (zombie)\n").unwrap();
        assert_eq!(
            linux_sample_at(&root, 201, 2.0, &mut baselines),
            ProcessSample::Zombie
        );
        assert_eq!(
            linux_sample_at(&root, 999, 2.0, &mut baselines),
            ProcessSample::Missing
        );
        let _ = std::fs::remove_dir_all(root);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_stat_extracts_starttime() {
        assert_eq!(
            linux_cpu_ticks(
                "201 (command with spaces) S 0 0 0 0 0 0 0 0 0 0 100 20 0 0 0 0 0 0 777"
            ),
            Some((120, 777))
        );
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn platform_observer_rejects_unrepresentable_native_pid() {
        let mut observer = platform_observer();
        observer.baselines.insert(
            ProcessIdentity {
                pid: u32::MAX,
                birth: ProcessBirth::LinuxStartTicks(1),
            },
            CpuBaseline {
                ticks: 1,
                sampled_at: 1.0,
            },
        );
        assert_eq!(
            observer.sample(u32::MAX, 0.0),
            ProcessSample::Unavailable {
                reason: ProcessUnavailableReason::Parse,
            }
        );
        assert!(observer.baselines.is_empty());
        assert!(native_pid_is_representable(i32::MAX as u32));
        assert!(!native_pid_is_representable(i32::MAX as u32 + 1));
    }
    #[test]
    fn darwin_invalid_utf8_is_unavailable_not_missing() {
        let mut baselines = BTreeMap::new();
        baselines.insert(
            ProcessIdentity {
                pid: 9,
                birth: ProcessBirth::DarwinStart("old".to_owned()),
            },
            CpuBaseline {
                ticks: 1,
                sampled_at: 1.0,
            },
        );
        assert_eq!(
            macos_parse_ps_bytes(9, b"\xff", 2.0, &mut baselines),
            ProcessSample::Unavailable {
                reason: ProcessUnavailableReason::Parse,
            }
        );
        assert!(baselines.is_empty());
        assert_eq!(
            macos_parse_ps_bytes(9, b"", 2.0, &mut baselines),
            ProcessSample::Missing
        );
    }
    #[test]
    fn process_matrix_pins_missing_zombie_cleanup_and_five_second_ghost_boundary() {
        struct Matrix;
        impl ProcessObserver for Matrix {
            fn sample(&mut self, pid: u32, _: f64) -> ProcessSample {
                match pid {
                    202 => ProcessSample::Missing,
                    203 => ProcessSample::Zombie,
                    204 => ProcessSample::AccessDenied,
                    _ => ProcessSample::Live {
                        identity: ProcessIdentity {
                            pid,
                            birth: ProcessBirth::LinuxStartTicks(1),
                        },
                        rss_bytes: 0,
                        cpu_percent: 8.6,
                    },
                }
            }
        }
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/top_reference.json")).unwrap();
        let mut state =
            TopState::from_fixture_value(&fixture["process_matrix"]["after_cleanup"]).unwrap();
        state.running_tasks.insert(
            "missing".into(),
            serde_json::json!({"name":"missing","pid":202}),
        );
        state.running_tasks.insert(
            "zombie".into(),
            serde_json::json!({"name":"zombie","pid":203}),
        );
        let mut observer = Matrix;
        cleanup_processes(
            &mut state,
            &ReductionSample::fixture(1_800_000_000.0, "x"),
            &mut observer,
        );
        assert!(
            !state.running_tasks.contains_key("missing")
                && !state.running_tasks.contains_key("zombie")
        );
        assert!(state.running_tasks.contains_key("denied"));
        assert!(
            state.finished_tasks.contains_key("missing")
                && state.finished_tasks.contains_key("zombie")
        );
        cleanup_processes(
            &mut state,
            &ReductionSample::fixture(1_800_000_005.0, "x"),
            &mut observer,
        );
        assert!(state.finished_tasks.contains_key("missing"));
        cleanup_processes(
            &mut state,
            &ReductionSample::fixture(1_800_000_005.1, "x"),
            &mut observer,
        );
        assert!(!state.finished_tasks.contains_key("missing"));
    }
    #[test]
    fn darwin_ps_requires_start_identity() {
        let mut baselines = BTreeMap::new();
        assert_eq!(
            macos_parse_ps(9, "Z 1 0.0 Mon Jan 01 00:00:00 2024\n", 1.0, &mut baselines),
            ProcessSample::Zombie
        );
        assert_eq!(
            macos_parse_ps(
                9,
                "S 6144 8.5 Mon Jan 01 00:00:00 2024\n",
                1.0,
                &mut baselines
            ),
            ProcessSample::Live {
                identity: ProcessIdentity {
                    pid: 9,
                    birth: ProcessBirth::DarwinStart("Mon Jan 01 00:00:00 2024".to_owned()),
                },
                rss_bytes: 6_291_456,
                cpu_percent: 8.5
            }
        );
        assert_eq!(
            macos_parse_ps(9, "S 6144 8.5\n", 1.0, &mut baselines),
            ProcessSample::Unavailable {
                reason: ProcessUnavailableReason::Parse
            }
        );
        assert_eq!(
            macos_parse_ps(9, "", 1.0, &mut baselines),
            ProcessSample::Missing
        );
    }
}
