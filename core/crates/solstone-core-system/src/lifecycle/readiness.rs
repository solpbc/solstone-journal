// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::LifecycleError;

/// Shared with Python readiness validation.
pub const START_TIME_TOLERANCE_SECONDS: f64 = 1.5;

pub(super) const WINDOWS_PROCESS_INSTANCE_FIELD: &str = "windows_process_instance";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadinessMarker {
    pub pid: u32,
    pub ready_at: f64,
    pub start_time: f64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn parse_marker(bytes: &[u8]) -> Option<ReadinessMarker> {
    serde_json::from_slice(bytes)
        .ok()
        .and_then(|marker: ReadinessMarker| {
            (marker.ready_at.is_finite() && marker.start_time.is_finite()).then_some(marker)
        })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn wait_ready_with(
    journal: &Path,
    timeout: Duration,
    now: impl Fn() -> Duration,
    poll: impl FnMut(),
) -> Option<ReadinessMarker> {
    wait_ready_with_start_time(
        journal,
        timeout,
        now,
        poll,
        super::state::process_start_time_epoch_seconds,
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn wait_ready_with_start_time(
    journal: &Path,
    timeout: Duration,
    now: impl Fn() -> Duration,
    mut poll: impl FnMut(),
    process_start_time: impl Fn(u32) -> Result<f64, LifecycleError>,
) -> Option<ReadinessMarker> {
    let start = now();
    loop {
        if readiness_is_valid_with_start_time(journal, &process_start_time) {
            return std::fs::read(journal.join("health/supervisor.ready"))
                .ok()
                .and_then(|bytes| parse_marker(&bytes));
        }
        if now().saturating_sub(start) >= timeout {
            return None;
        }
        poll();
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn wait_ready(
    journal: &Path,
    timeout: Duration,
    poll_interval: Duration,
) -> Option<ReadinessMarker> {
    let start = std::time::Instant::now();
    wait_ready_with(
        journal,
        timeout,
        || start.elapsed(),
        || std::thread::sleep(poll_interval),
    )
}

#[cfg(windows)]
pub fn wait_ready(
    journal: &Path,
    timeout: Duration,
    poll_interval: Duration,
) -> Option<ReadinessMarker> {
    let start = std::time::Instant::now();
    loop {
        if readiness_is_valid(journal) {
            return std::fs::read(journal.join("health/supervisor.ready"))
                .ok()
                .and_then(|bytes| parse_marker(&bytes));
        }
        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(poll_interval);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn readiness_is_valid(journal: impl AsRef<Path>) -> bool {
    readiness_is_valid_with_start_time(journal, super::state::process_start_time_epoch_seconds)
}

#[cfg(windows)]
pub fn readiness_is_valid(journal: impl AsRef<Path>) -> bool {
    let health = journal.as_ref().join("health");
    let Ok(marker_bytes) = std::fs::read(health.join("supervisor.ready")) else {
        return false;
    };
    let Some(marker) = parse_marker(&marker_bytes) else {
        return false;
    };
    let Ok(instance_bytes) = std::fs::read(health.join("supervisor.process_instance")) else {
        return false;
    };
    let Ok(instance) = serde_json::from_slice::<crate::process::ProcessInstance>(&instance_bytes)
    else {
        return false;
    };
    if !windows_marker_matches_instance(&marker, &instance) {
        return false;
    }
    matches!(
        crate::process::ProcessInstanceSource::observe(
            &crate::process::SystemProcessInstanceSource,
            &instance
        ),
        crate::process::InstanceVerdict::SameLive { .. }
    )
}

#[cfg(any(windows, test))]
fn windows_marker_matches_instance(
    marker: &ReadinessMarker,
    instance: &crate::process::ProcessInstance,
) -> bool {
    let Some(value) = marker.extra.get(WINDOWS_PROCESS_INSTANCE_FIELD) else {
        return false;
    };
    serde_json::from_value::<crate::process::ProcessInstance>(value.clone()).is_ok_and(|bound| {
        bound.pid == marker.pid && bound == *instance && bound.birth.windows_filetime().is_some()
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn readiness_is_valid_with_start_time(
    journal: impl AsRef<Path>,
    process_start_time: impl Fn(u32) -> Result<f64, LifecycleError>,
) -> bool {
    let health = journal.as_ref().join("health");
    let Ok(marker_bytes) = std::fs::read(health.join("supervisor.ready")) else {
        return false;
    };
    let Some(marker) = parse_marker(&marker_bytes) else {
        return false;
    };
    let Ok(pid) = std::fs::read_to_string(health.join("supervisor.pid")).and_then(|text| {
        text.trim()
            .parse::<u32>()
            .map_err(|_| std::io::Error::other("pid"))
    }) else {
        return false;
    };
    if marker.pid != pid {
        return false;
    }
    let Ok(recorded) =
        std::fs::read_to_string(health.join("supervisor.start_time")).and_then(|text| {
            text.trim()
                .parse::<f64>()
                .map_err(|_| std::io::Error::other("start"))
        })
    else {
        return false;
    };
    let Ok(actual) = process_start_time(pid) else {
        return false;
    };
    // Marker start_time is schema-only; pid-file identity is authoritative.
    (recorded - actual).abs() <= START_TIME_TOLERANCE_SECONDS
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use super::{ReadinessMarker, readiness_is_valid_with_start_time, wait_ready_with_start_time};

    #[test]
    fn ac3_ac8_injected_start_time_rejects_reused_pid() {
        let pid = std::process::id();
        let marker = ReadinessMarker {
            pid,
            ready_at: 1.0,
            start_time: 100.0,
            extra: serde_json::Map::new(),
        };
        let root = super::super::state::test_supervisor_journal(
            "readiness-probe",
            pid,
            100.0,
            Some(&marker),
        );
        assert!(readiness_is_valid_with_start_time(&root, |_| Ok(100.0)));
        assert!(!readiness_is_valid_with_start_time(&root, |_| Ok(101.6)));

        let ticks = Cell::new(0_u64);
        assert!(
            wait_ready_with_start_time(
                &root,
                Duration::from_secs(1),
                || Duration::from_secs(ticks.get()),
                || ticks.set(ticks.get() + 1),
                |_| Ok(101.6),
            )
            .is_none()
        );
        super::super::state::remove_test_supervisor_journal(root);
    }
}

#[cfg(test)]
mod windows_marker_identity_tests {
    use super::*;
    use crate::process::{ProcessBirth, ProcessInstance};

    #[test]
    fn exact_windows_birth_binds_readiness_even_when_pid_and_float_time_match() {
        let current = ProcessInstance {
            pid: 42,
            birth: ProcessBirth::windows(134_333_000_000_000_001),
        };
        let stale = ProcessInstance {
            pid: current.pid,
            birth: ProcessBirth::windows(134_333_000_000_000_000),
        };
        let mut marker = ReadinessMarker {
            pid: current.pid,
            ready_at: 2.0,
            start_time: 1.0,
            extra: Default::default(),
        };
        // Missing, malformed and unknown birth never become an unbound ready marker.
        assert!(!windows_marker_matches_instance(&marker, &current));
        marker.extra.insert(
            WINDOWS_PROCESS_INSTANCE_FIELD.into(),
            serde_json::json!("invalid"),
        );
        assert!(!windows_marker_matches_instance(&marker, &current));
        marker.extra.insert(
            WINDOWS_PROCESS_INSTANCE_FIELD.into(),
            serde_json::to_value(stale).unwrap(),
        );
        assert!(!windows_marker_matches_instance(&marker, &current));
        marker.extra.insert(
            WINDOWS_PROCESS_INSTANCE_FIELD.into(),
            serde_json::to_value(current).unwrap(),
        );
        assert!(windows_marker_matches_instance(&marker, &current));
        marker.pid += 1;
        assert!(!windows_marker_matches_instance(&marker, &current));
        marker.pid = current.pid;
        marker.extra.insert(
            WINDOWS_PROCESS_INSTANCE_FIELD.into(),
            serde_json::json!({"pid":current.pid,"birth":{"kind":"unknown"}}),
        );
        assert!(!windows_marker_matches_instance(&marker, &current));
    }
}
