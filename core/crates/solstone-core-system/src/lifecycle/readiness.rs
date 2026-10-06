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

/// Readiness is unsupported on Android and iOS.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn wait_ready(
    _journal: &Path,
    _timeout: Duration,
    _poll_interval: Duration,
) -> Option<ReadinessMarker> {
    log::debug!("supervisor readiness is unsupported on this platform");
    None
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

/// The verdict on the supervisor process this journal's resident recorded at
/// boot in `health/supervisor.process_instance`: is that exact process alive?
///
/// This is the liveness answer for a caller outside the resident. ⛔ On
/// Windows it is the only one: the resident serves Callosum on a named pipe and
/// never creates `health/callosum.sock`, so testing that path reports "nothing
/// running" on every healthy install. A missing or unreadable record means no
/// resident has booted since the journal was last cleanly stopped.
pub fn recorded_supervisor_verdict(journal: impl AsRef<Path>) -> crate::process::InstanceVerdict {
    use crate::process::{InstanceVerdict, ProcessInstanceSource, SystemProcessInstanceSource};
    let path = journal
        .as_ref()
        .join("health")
        .join("supervisor.process_instance");
    let Ok(bytes) = std::fs::read(path) else {
        return InstanceVerdict::NotSameOrExited;
    };
    let Ok(instance) = serde_json::from_slice::<crate::process::ProcessInstance>(&bytes) else {
        return InstanceVerdict::NotSameOrExited;
    };
    SystemProcessInstanceSource.observe(&instance)
}

/// This journal's resident, as its boot record names it and as its own sync
/// heartbeat describes it: the host it runs on, its pid, and when it started.
///
/// A caller outside the resident uses this to tell the resident's heartbeat
/// apart from every other one. It exists only while the recorded process is
/// verifiably the one alive now, so a reused pid never stands in for it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedResident {
    hostname: String,
    pid: u32,
    started_at: f64,
}

impl RecordedResident {
    /// The host name the resident writes into its heartbeat.
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The resident's start, in epoch seconds.
    pub fn started_at(&self) -> f64 {
        self.started_at
    }

    /// The one heartbeat among `peers` that this resident wrote, if exactly
    /// one qualifies.
    ///
    /// A heartbeat qualifies when it is an authoritative v2 record naming
    /// this host and this pid, stamped no earlier than the resident started.
    /// From that start onward only the resident holds its pid on this host,
    /// so no other process here can have written such a record. Another
    /// machine that shares the host name and happens to share the pid is the
    /// one case these fields cannot rule out; it would also match, and when
    /// more than one heartbeat qualifies none is claimed.
    pub fn own_heartbeat<'a>(
        &self,
        peers: &'a [super::sync::SyncPeerObservation],
    ) -> Option<&'a std::ffi::OsStr> {
        let mut matching = peers.iter().filter(|peer| self.wrote(peer));
        let own = matching.next()?;
        matching
            .next()
            .is_none()
            .then_some(own.source_filename.as_os_str())
    }

    fn wrote(&self, peer: &super::sync::SyncPeerObservation) -> bool {
        let super::sync::HeartbeatClassification::SchemaV2(heartbeat) = &peer.classification else {
            return false;
        };
        heartbeat.pid == self.pid
            && heartbeat.hostname == self.hostname
            && heartbeat.wall_time.parse::<f64>().is_ok_and(|written| {
                written.is_finite() && written + START_TIME_TOLERANCE_SECONDS >= self.started_at
            })
    }
}

/// The resident this journal recorded at boot, if that exact process is alive.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn recorded_live_resident(journal: impl AsRef<Path>) -> Option<RecordedResident> {
    let health = journal.as_ref().join("health");
    let pid = std::fs::read_to_string(health.join("supervisor.pid"))
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()?;
    let recorded = std::fs::read_to_string(health.join("supervisor.start_time"))
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())?;
    let actual = super::state::process_start_time_epoch_seconds(pid).ok()?;
    ((recorded - actual).abs() <= START_TIME_TOLERANCE_SECONDS).then(|| RecordedResident {
        hostname: super::hostname(),
        pid,
        started_at: actual,
    })
}

/// The resident this journal recorded at boot, if that exact process is alive.
#[cfg(windows)]
pub fn recorded_live_resident(journal: impl AsRef<Path>) -> Option<RecordedResident> {
    use crate::process::{InstanceVerdict, ProcessInstanceSource, SystemProcessInstanceSource};
    let bytes = std::fs::read(
        journal
            .as_ref()
            .join("health")
            .join(super::windows::SUPERVISOR_PROCESS_INSTANCE),
    )
    .ok()?;
    let instance = serde_json::from_slice::<crate::process::ProcessInstance>(&bytes).ok()?;
    let started_at = instance.birth.epoch_seconds()?;
    matches!(
        SystemProcessInstanceSource.observe(&instance),
        InstanceVerdict::SameLive { .. }
    )
    .then(|| RecordedResident {
        hostname: super::windows::hostname(),
        pid: instance.pid,
        started_at,
    })
}

/// No resident identity can be verified on this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn recorded_live_resident(_journal: impl AsRef<Path>) -> Option<RecordedResident> {
    None
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

#[cfg(test)]
mod resident_heartbeat_tests {
    use std::ffi::{OsStr, OsString};

    use super::RecordedResident;
    use crate::lifecycle::sync::{
        HEARTBEAT_SCHEMA_V1, Heartbeat, HeartbeatClassification, HeartbeatV2, RunId,
        SyncPeerObservation, WriterId,
    };

    const STARTED_AT: f64 = 1_000.0;

    fn resident() -> RecordedResident {
        RecordedResident {
            hostname: "this-host".to_owned(),
            pid: 42,
            started_at: STARTED_AT,
        }
    }

    fn v2(name: &str, run: &str, hostname: &str, pid: u32, wall: f64) -> SyncPeerObservation {
        SyncPeerObservation {
            source_filename: OsString::from(name),
            classification: HeartbeatClassification::SchemaV2(HeartbeatV2::new(
                WriterId::parse("0123456789abcdef0123456789abcdef").unwrap(),
                RunId::parse(run).unwrap(),
                hostname.to_owned(),
                pid,
                wall.to_string(),
                "test".to_owned(),
                15,
                "/journal".to_owned(),
            )),
            heartbeat: None,
            is_live: true,
        }
    }

    const RUN_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const RUN_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn the_residents_own_heartbeat_is_found_among_others() {
        let peers = [
            v2("other.check", RUN_B, "other-host", 42, STARTED_AT + 10.0),
            v2("own.check", RUN_A, "this-host", 42, STARTED_AT + 10.0),
        ];
        assert_eq!(
            resident().own_heartbeat(&peers),
            Some(OsStr::new("own.check"))
        );
    }

    #[test]
    fn nothing_that_could_be_another_writer_is_claimed() {
        let later = STARTED_AT + 10.0;
        // Another machine, another process here, and a record written before
        // the resident started (a previous holder of the same pid).
        for peer in [
            v2("other-host.check", RUN_A, "other-host", 42, later),
            v2("other-pid.check", RUN_A, "this-host", 43, later),
            v2("earlier.check", RUN_A, "this-host", 42, STARTED_AT - 60.0),
        ] {
            assert_eq!(
                resident().own_heartbeat(std::slice::from_ref(&peer)),
                None,
                "{:?}",
                peer.source_filename
            );
        }
        let legacy = SyncPeerObservation {
            source_filename: OsString::from("legacy.check"),
            classification: HeartbeatClassification::SchemaV1(Heartbeat {
                schema: HEARTBEAT_SCHEMA_V1,
                machine_id: "machine".to_owned(),
                hostname: "this-host".to_owned(),
                pid: 42,
                wall_time: later.to_string(),
                solstone_version: "test".to_owned(),
                interval_seconds: 15,
                journal_path: "/journal".to_owned(),
            }),
            heartbeat: None,
            is_live: true,
        };
        assert_eq!(resident().own_heartbeat(&[legacy]), None);
    }

    #[test]
    fn two_candidates_claim_neither() {
        let peers = [
            v2("one.check", RUN_A, "this-host", 42, STARTED_AT + 10.0),
            v2("two.check", RUN_B, "this-host", 42, STARTED_AT + 20.0),
        ];
        assert_eq!(resident().own_heartbeat(&peers), None);
    }
}
