// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keeps an installed local provider on the release's pins.
//!
//! The bundled local provider is installed when the owner asks for it, and a
//! release can move the pinned llama.cpp runtime or the pinned model. After such
//! an update the installed copy no longer matches its pin, so the provider stays
//! `artifact-not-ready` and nothing on this computer thinks until the owner
//! installs again. When the owner has installed a local model before, this
//! starts the same installer the Thinking app starts, so the update brings the
//! newly pinned artifacts with it.
//!
//! Windows truth follows a moved model pin through convey; the Windows binary
//! proof is the signed package. Linux and macOS still follow runtime and model pin moves.
//!
//! The decision is made on every local truth observation and reads only
//! durable state: the journal config, the presence-mode readiness the
//! observation already computed, the install status record and the install
//! lease. It never hashes artifact bytes; the launched installer does that.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use solstone_core_local::install::status::InstallStatus;
use solstone_core_local::install::{lease, manifest, pins, status};

/// Starts `install-provider local` for a journal. Returns once the installer
/// process has been launched; it reports through the install status record.
pub type LocalInstallerLauncher = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// No attempt starts within this long of the install record's latest activity,
/// whichever process wrote it. This is what holds across a supervisor restart.
pub(crate) const PERSISTED_BACKOFF: Duration = Duration::from_secs(30 * 60);

/// Spacing between attempts launched by one supervisor. A deterministic failure
/// settles at one attempt a day.
pub(crate) const LAUNCH_BACKOFF_STEPS: [Duration; 4] = [
    Duration::from_secs(30 * 60),
    Duration::from_secs(2 * 60 * 60),
    Duration::from_secs(8 * 60 * 60),
    Duration::from_secs(24 * 60 * 60),
];

/// Artifact proof reasons that mean "not at the current pin", as opposed to a
/// damaged or unreadable install, which stays visible instead of being refetched.
const PIN_MOVE_REASONS: [&str; 2] = ["manifest_missing", "manifest_pin_mismatch"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FollowDecision {
    Launch,
    Hold(&'static str),
}

/// What one supervisor remembers about the attempts it launched.
#[derive(Debug, Default)]
pub(crate) struct FollowMemory {
    launches: u32,
    last_launch: Option<Instant>,
}

impl FollowMemory {
    fn backoff_elapsed(&self, now: Instant) -> bool {
        let Some(last) = self.last_launch else {
            return true;
        };
        let step = self.launches.saturating_sub(1) as usize;
        let wait = LAUNCH_BACKOFF_STEPS[step.min(LAUNCH_BACKOFF_STEPS.len() - 1)];
        now.saturating_duration_since(last) >= wait
    }

    fn record_launch(&mut self, now: Instant) {
        self.launches = self.launches.saturating_add(1);
        self.last_launch = Some(now);
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

pub(crate) struct LocalFollow {
    launcher: LocalInstallerLauncher,
    memory: Mutex<FollowMemory>,
}

impl LocalFollow {
    pub(crate) fn new(launcher: LocalInstallerLauncher) -> Self {
        Self {
            launcher,
            memory: Mutex::new(FollowMemory::default()),
        }
    }

    /// Called with each local readiness observation, including Windows after
    /// the signed package and model proofs are known.
    pub(crate) fn observe(
        &self,
        journal: &Path,
        config: Option<&Map<String, Value>>,
        readiness: &Value,
    ) -> FollowDecision {
        let Ok(mut memory) = self.memory.lock() else {
            return FollowDecision::Hold("follow-state-unavailable");
        };
        if readiness.get("ready").and_then(Value::as_bool) == Some(true) {
            memory.reset();
            return FollowDecision::Hold("ready");
        }
        let now = Instant::now();
        let record = status::read_status(journal, "local").ok();
        let decision = decide(
            &FollowInput {
                local_active: config.is_some_and(local_is_active),
                model_installed_before: model_installed_before(journal),
                readiness,
                record: record.as_ref(),
                now: Utc::now(),
                launch_backoff_elapsed: memory.backoff_elapsed(now),
            },
            || lease::is_held(journal, "local"),
        );
        if decision == FollowDecision::Launch {
            memory.record_launch(now);
            match (self.launcher)(journal) {
                Ok(()) => log::info!(
                    "local provider artifacts are behind this release's pins; started the installer"
                ),
                Err(error) => log::warn!(
                    "local provider artifacts are behind this release's pins; installer did not start: {error}"
                ),
            }
        }
        decision
    }
}

pub(crate) struct FollowInput<'a> {
    pub local_active: bool,
    pub model_installed_before: bool,
    pub readiness: &'a Value,
    /// `None` when the install status record could not be read.
    pub record: Option<&'a InstallStatus>,
    pub now: DateTime<Utc>,
    pub launch_backoff_elapsed: bool,
}

pub(crate) fn decide(
    input: &FollowInput<'_>,
    lease_held: impl FnOnce() -> std::io::Result<bool>,
) -> FollowDecision {
    if !input.local_active {
        return FollowDecision::Hold("local-not-active");
    }
    if !input.model_installed_before {
        return FollowDecision::Hold("never-installed");
    }
    if let Some(reason) = pins_moved(input.readiness) {
        return FollowDecision::Hold(reason);
    }
    let Some(record) = input.record else {
        return FollowDecision::Hold("install-status-unreadable");
    };
    if record.install_state == "failed" && record.error_code.as_deref() == Some("install_cancelled")
    {
        return FollowDecision::Hold("owner-cancelled");
    }
    match latest_activity(record) {
        Err(()) => return FollowDecision::Hold("install-status-unreadable"),
        Ok(Some(latest)) => {
            let Ok(since) = (input.now - latest).to_std() else {
                return FollowDecision::Hold("install-status-in-future");
            };
            if since < PERSISTED_BACKOFF {
                return FollowDecision::Hold("backoff");
            }
        }
        Ok(None) => {}
    }
    if !input.launch_backoff_elapsed {
        return FollowDecision::Hold("backoff");
    }
    match lease_held() {
        Ok(false) => FollowDecision::Launch,
        Ok(true) => FollowDecision::Hold("install-running"),
        Err(_) => FollowDecision::Hold("install-lease-unreadable"),
    }
}

/// `None` when every artifact is either ready or simply not at the current
/// pin and at least one is not; otherwise the reason to hold.
fn pins_moved(readiness: &Value) -> Option<&'static str> {
    if readiness.get("ready").and_then(Value::as_bool) == Some(true) {
        return Some("ready");
    }
    if readiness.get("status").and_then(Value::as_str) != Some("missing-or-mismatched") {
        return Some("readiness-not-followable");
    }
    if readiness
        .pointer("/host/platform_supported")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Some("platform-unsupported");
    }
    let Some(proofs) = readiness.get("proof").and_then(Value::as_object) else {
        return Some("readiness-not-followable");
    };
    let mut moved = false;
    // Proofs carry a status; a launch probe entry does not and is skipped.
    for proof in proofs
        .values()
        .filter(|proof| proof.get("status").is_some())
    {
        match proof.get("status").and_then(Value::as_str) {
            Some("ready") => {}
            Some("missing-or-mismatched")
                if proof
                    .get("reason_code")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| PIN_MOVE_REASONS.contains(&reason)) =>
            {
                moved = true;
            }
            _ => return Some("artifact-damaged"),
        }
    }
    if moved {
        None
    } else {
        Some("readiness-not-followable")
    }
}

fn local_is_active(config: &Map<String, Value>) -> bool {
    config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("active"))
        .and_then(Value::as_object)
        .and_then(|active| active.get("provider"))
        .and_then(Value::as_str)
        == Some("local")
}

/// The owner installed local thinking at some pin: a model manifest exists.
/// The installer rewrites a model's manifest only after its files are fetched,
/// so an interrupted attempt leaves this true.
fn model_installed_before(journal: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(pins::cache_root(journal).join("models")) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name();
        !name.to_string_lossy().starts_with('.')
            && manifest::artifact_manifest_path(&entry.path()).is_file()
    })
}

fn latest_activity(record: &InstallStatus) -> Result<Option<DateTime<Utc>>, ()> {
    let mut latest = None;
    for stamp in [
        &record.completed_at,
        &record.last_transition_at,
        &record.last_progress_at,
    ]
    .into_iter()
    .flatten()
    {
        let parsed = DateTime::parse_from_rfc3339(stamp)
            .map_err(|_| ())?
            .with_timezone(&Utc);
        latest = latest.max(Some(parsed));
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn moved_runtime() -> Value {
        json!({
            "ready": false,
            "status": "missing-or-mismatched",
            "reason_code": "manifest_missing",
            "host": {"platform_supported": true, "backend": "vulkan"},
            "proof": {
                "binary": {"status": "missing-or-mismatched", "reason_code": "manifest_missing"},
                "model": {"status": "ready", "reason_code": "ready"},
            },
        })
    }

    fn moved_metal_model() -> Value {
        json!({
            "ready": false,
            "status": "missing-or-mismatched",
            "reason_code": "manifest_pin_mismatch",
            "host": {"platform_supported": true, "backend": "metal"},
            "proof": {
                "server_binary": {"status": "ready", "reason_code": "ready"},
                "model_gguf": {"status": "missing-or-mismatched", "reason_code": "manifest_pin_mismatch"},
                "projector": {"status": "missing-or-mismatched", "reason_code": "manifest_pin_mismatch"},
                "binary_probe": {"runnable": true, "verification": "deferred_until_launch"},
            },
        })
    }

    fn record(state: &str, error_code: Option<&str>, at: Option<&str>) -> InstallStatus {
        InstallStatus {
            schema_version: 1,
            provider: "local".into(),
            revision: 3,
            install_state: state.into(),
            attempt_id: None,
            target_fingerprint_json: None,
            target_fingerprint_sha256: None,
            started_at: at.map(Into::into),
            last_transition_at: at.map(Into::into),
            last_progress_at: None,
            completed_at: (state == "installed" || state == "failed")
                .then(|| at.map(Into::into))
                .flatten(),
            progress_bytes_received: None,
            progress_bytes_total: None,
            install_error: error_code.map(Into::into),
            error_code: error_code.map(Into::into),
            owner: None,
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-07T18:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn installed_long_ago() -> InstallStatus {
        record("installed", None, Some("2026-09-01T12:00:00Z"))
    }

    fn decide_with(
        readiness: &Value,
        record: Option<&InstallStatus>,
        adjust: impl FnOnce(&mut FollowInput<'_>),
        lease: std::io::Result<bool>,
    ) -> FollowDecision {
        let mut input = FollowInput {
            local_active: true,
            model_installed_before: true,
            readiness,
            record,
            now: now(),
            launch_backoff_elapsed: true,
        };
        adjust(&mut input);
        decide(&input, || lease)
    }

    #[test]
    fn a_moved_runtime_pin_launches_the_installer() {
        let installed = installed_long_ago();
        assert_eq!(
            decide_with(&moved_runtime(), Some(&installed), |_| {}, Ok(false)),
            FollowDecision::Launch
        );
    }

    #[test]
    fn a_moved_model_pin_launches_the_installer_on_metal() {
        let installed = installed_long_ago();
        assert_eq!(
            decide_with(&moved_metal_model(), Some(&installed), |_| {}, Ok(false)),
            FollowDecision::Launch
        );
    }

    #[test]
    fn each_missing_precondition_holds() {
        let installed = installed_long_ago();
        let readiness = moved_runtime();
        type Adjust = Box<dyn FnOnce(&mut FollowInput<'_>)>;
        let cases: Vec<(&str, Adjust)> = vec![
            (
                "local-not-active",
                Box::new(|input| input.local_active = false),
            ),
            (
                "never-installed",
                Box::new(|input| input.model_installed_before = false),
            ),
            (
                "backoff",
                Box::new(|input| input.launch_backoff_elapsed = false),
            ),
            (
                "install-status-unreadable",
                Box::new(|input| input.record = None),
            ),
        ];
        for (expected, adjust) in cases {
            assert_eq!(
                decide_with(&readiness, Some(&installed), adjust, Ok(false)),
                FollowDecision::Hold(expected)
            );
        }
    }

    #[test]
    fn damaged_or_unproven_artifacts_are_never_refetched() {
        let installed = installed_long_ago();
        for (pointer, value) in [
            ("/proof/binary/reason_code", json!("sha256_mismatch")),
            (
                "/proof/binary/reason_code",
                json!("inventory_size_mismatch"),
            ),
            ("/proof/binary/reason_code", json!("manifest_malformed")),
            ("/proof/binary/reason_code", json!("cuda_runtime_integrity")),
            ("/proof/model/status", json!("proof-unavailable")),
        ] {
            let mut readiness = moved_runtime();
            *readiness.pointer_mut(pointer).unwrap() = value;
            assert_eq!(
                decide_with(&readiness, Some(&installed), |_| {}, Ok(false)),
                FollowDecision::Hold("artifact-damaged"),
                "{pointer}"
            );
        }
        let mut unproven = moved_runtime();
        unproven["status"] = json!("proof-unavailable");
        assert_eq!(
            decide_with(&unproven, Some(&installed), |_| {}, Ok(false)),
            FollowDecision::Hold("readiness-not-followable")
        );
        let mut unsupported = moved_runtime();
        unsupported["host"]["platform_supported"] = json!(false);
        assert_eq!(
            decide_with(&unsupported, Some(&installed), |_| {}, Ok(false)),
            FollowDecision::Hold("platform-unsupported")
        );
    }

    #[test]
    fn owner_cancel_holds_until_the_owner_installs_again() {
        let cancelled = record(
            "failed",
            Some("install_cancelled"),
            Some("2026-09-01T12:00:00Z"),
        );
        assert_eq!(
            decide_with(&moved_runtime(), Some(&cancelled), |_| {}, Ok(false)),
            FollowDecision::Hold("owner-cancelled")
        );
    }

    #[test]
    fn failed_and_abandoned_attempts_back_off_from_their_own_timestamps() {
        for (state, code) in [
            ("failed", Some("install_failed")),
            ("failed", Some("install_interrupted")),
            ("downloading", None),
            ("installed", None),
        ] {
            let recent = record(state, code, Some("2026-10-07T17:45:00Z"));
            assert_eq!(
                decide_with(&moved_runtime(), Some(&recent), |_| {}, Ok(false)),
                FollowDecision::Hold("backoff"),
                "{state}"
            );
            let elapsed = record(state, code, Some("2026-10-07T17:15:00Z"));
            assert_eq!(
                decide_with(&moved_runtime(), Some(&elapsed), |_| {}, Ok(false)),
                FollowDecision::Launch,
                "{state}"
            );
        }
        let future = record(
            "failed",
            Some("install_failed"),
            Some("2026-10-08T00:00:00Z"),
        );
        assert_eq!(
            decide_with(&moved_runtime(), Some(&future), |_| {}, Ok(false)),
            FollowDecision::Hold("install-status-in-future")
        );
        let garbled = record("failed", Some("install_failed"), Some("yesterday"));
        assert_eq!(
            decide_with(&moved_runtime(), Some(&garbled), |_| {}, Ok(false)),
            FollowDecision::Hold("install-status-unreadable")
        );
    }

    #[test]
    fn a_running_or_unreadable_lease_holds() {
        let installed = installed_long_ago();
        assert_eq!(
            decide_with(&moved_runtime(), Some(&installed), |_| {}, Ok(true)),
            FollowDecision::Hold("install-running")
        );
        assert_eq!(
            decide_with(
                &moved_runtime(),
                Some(&installed),
                |_| {},
                Err(std::io::Error::other("lease"))
            ),
            FollowDecision::Hold("install-lease-unreadable")
        );
    }

    #[test]
    fn launch_backoff_escalates_to_daily() {
        let start = Instant::now();
        let mut memory = FollowMemory::default();
        assert!(memory.backoff_elapsed(start));
        let mut at = start;
        for step in LAUNCH_BACKOFF_STEPS
            .iter()
            .chain(std::iter::repeat_n(&LAUNCH_BACKOFF_STEPS[3], 2))
        {
            memory.record_launch(at);
            assert!(!memory.backoff_elapsed(at + *step - Duration::from_secs(1)));
            assert!(memory.backoff_elapsed(at + *step));
            at += *step;
        }
        memory.reset();
        assert!(memory.backoff_elapsed(at));
    }
}
