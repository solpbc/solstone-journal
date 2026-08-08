// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::time::Duration;

/// Shared with Python readiness validation.
pub const START_TIME_TOLERANCE_SECONDS: f64 = 1.5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadinessMarker {
    pub pid: u32,
    pub ready_at: f64,
    pub start_time: f64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[cfg(target_os = "linux")]
pub fn parse_marker(bytes: &[u8]) -> Option<ReadinessMarker> {
    serde_json::from_slice(bytes)
        .ok()
        .and_then(|marker: ReadinessMarker| {
            (marker.ready_at.is_finite() && marker.start_time.is_finite()).then_some(marker)
        })
}

#[cfg(target_os = "linux")]
pub fn wait_ready_with(
    journal: &Path,
    timeout: Duration,
    now: impl Fn() -> Duration,
    mut poll: impl FnMut(),
) -> Option<ReadinessMarker> {
    let start = now();
    loop {
        if readiness_is_valid(journal) {
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
pub fn readiness_is_valid(journal: impl AsRef<std::path::Path>) -> bool {
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
    let Ok(actual) = super::state::process_start_time_epoch_seconds(pid) else {
        return false;
    };
    // Marker start_time is schema-only; pid-file identity is authoritative.
    (recorded - actual).abs() <= START_TIME_TOLERANCE_SECONDS
}
