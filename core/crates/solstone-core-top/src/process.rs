// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

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
    Live { rss_bytes: u64, cpu_percent: f64 },
    Missing,
    AccessDenied,
    Zombie,
    Unavailable { reason: ProcessUnavailableReason },
}

/// Observe only the supplied PID. Implementations never enumerate command
/// arguments, environment, process trees, or perform destructive actions.
pub trait ProcessObserver {
    fn sample(&mut self, pid: u32, monotonic_seconds: f64) -> ProcessSample;
}

/// Construct the host backend; unsupported hosts return `Unavailable`.
#[must_use]
pub fn platform_observer() -> PlatformProcessObserver {
    PlatformProcessObserver::default()
}

/// Small platform facade that keeps cfg-specific implementation private.
#[derive(Default)]
pub struct PlatformProcessObserver {
    #[cfg(target_os = "linux")]
    baselines: BTreeMap<u32, (u64, f64)>,
}

impl ProcessObserver for PlatformProcessObserver {
    #[cfg(target_os = "linux")]
    fn sample(&mut self, pid: u32, monotonic_seconds: f64) -> ProcessSample {
        linux_sample(pid, monotonic_seconds, &mut self.baselines)
    }

    #[cfg(target_os = "macos")]
    fn sample(&mut self, pid: u32, _monotonic_seconds: f64) -> ProcessSample {
        macos_sample(pid)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn sample(&mut self, _pid: u32, _monotonic_seconds: f64) -> ProcessSample {
        ProcessSample::Unavailable {
            reason: ProcessUnavailableReason::UnsupportedPlatform,
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_sample(
    pid: u32,
    monotonic_seconds: f64,
    baselines: &mut BTreeMap<u32, (u64, f64)>,
) -> ProcessSample {
    linux_sample_at(Path::new("/proc"), pid, monotonic_seconds, baselines)
}

#[cfg(target_os = "linux")]
fn linux_sample_at(
    proc_root: &Path,
    pid: u32,
    monotonic_seconds: f64,
    baselines: &mut BTreeMap<u32, (u64, f64)>,
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
    let Some(ticks) = linux_cpu_ticks(&stat) else {
        return ProcessSample::Unavailable {
            reason: ProcessUnavailableReason::Parse,
        };
    };
    let percent = baselines
        .insert(pid, (ticks, monotonic_seconds))
        .and_then(|(old_ticks, old_at)| {
            let elapsed = monotonic_seconds - old_at;
            (elapsed > 0.0)
                .then(|| ((ticks.saturating_sub(old_ticks)) as f64 / 100.0) / elapsed * 100.0)
        })
        .unwrap_or(0.0);
    ProcessSample::Live {
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
fn linux_cpu_ticks(stat: &str) -> Option<u64> {
    let close = stat.rfind(')')?;
    let fields: Vec<_> = stat[close + 2..].split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

#[cfg(target_os = "macos")]
fn macos_sample(pid: u32) -> ProcessSample {
    let output = match std::process::Command::new("/bin/ps")
        .args(["-o", "state=,rss=,%cpu=", "-p", &pid.to_string()])
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
    macos_parse_ps(std::str::from_utf8(&output.stdout).unwrap_or(""))
}

#[cfg(target_os = "macos")]
fn macos_parse_ps(output: &str) -> ProcessSample {
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
        (Some(rss), Some(cpu_percent)) => ProcessSample::Live {
            rss_bytes: rss.saturating_mul(1024),
            cpu_percent,
        },
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
            "201 (x) S 0 0 0 0 0 0 0 0 0 0 100 20",
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
    #[test]
    fn platform_observer_never_panics_for_a_missing_pid() {
        let result = std::panic::catch_unwind(|| {
            let mut observer = platform_observer();
            observer.sample(u32::MAX, 0.0)
        });
        assert!(result.is_ok());
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
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_parser_is_deterministic() {
        assert_eq!(macos_parse_ps("Z 1 0.0\n"), ProcessSample::Zombie);
        assert_eq!(
            macos_parse_ps("S 6144 8.5\n"),
            ProcessSample::Live {
                rss_bytes: 6_291_456,
                cpu_percent: 8.5
            }
        );
        assert_eq!(macos_parse_ps(""), ProcessSample::Missing);
    }
}
