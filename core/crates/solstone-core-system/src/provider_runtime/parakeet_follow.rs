// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keeps the Parakeet provider installed and on the release's pins.
//!
//! On linux, Parakeet is the default speech recognizer, and `journal setup`
//! installs it into the journal's provider cache. A journal can still lack it:
//! setup's model step failed or was skipped, or the journal was restored or
//! copied, which leaves `cache/` behind. Transcription then defers every audio
//! file with `no_port` for a server that is never started. So when admission
//! says Parakeet is wanted and its artifacts are missing or behind their pins,
//! this starts the same installer `journal install-provider parakeet` runs,
//! whether or not a model was installed before. Owner cancellation, persisted
//! cooldown and launch spacing still apply.

use std::path::Path;
#[cfg(any(not(windows), test))]
use std::sync::Mutex;
#[cfg(any(not(windows), test))]
use std::time::Instant;

#[cfg(any(not(windows), test))]
use chrono::{DateTime, Utc};
use serde_json::Value;
#[cfg(any(not(windows), test))]
use solstone_core_local::install::status::InstallStatus;
#[cfg(not(windows))]
use solstone_core_local::install::{lease, manifest, pins, status};

use super::local_follow::LocalInstallerLauncher;
#[cfg(any(not(windows), test))]
use super::local_follow::{FollowDecision, FollowMemory, install_follow_tail};

// Windows takes Parakeet from its installed package, so the pin follow below
// runs only off Windows (and in this file's tests).
#[cfg(any(not(windows), test))]
const PIN_MOVE_REASONS: [&str; 2] = ["manifest_missing", "manifest_pin_mismatch"];

#[cfg(any(not(windows), test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParakeetFollowDecision {
    Launch,
    Hold(String),
}

#[cfg(any(not(windows), test))]
pub(crate) struct ParakeetFollowInput<'a> {
    pub cpu_proof: &'a Value,
    pub vulkan_proof: &'a Value,
    pub model_proof: &'a Value,
    pub desired: bool,
    pub blocked: bool,
    pub model_installed_before: bool,
    pub record: Option<&'a InstallStatus>,
    pub now: DateTime<Utc>,
}

pub(crate) struct ParakeetFollow {
    #[cfg(any(not(windows), test))]
    launcher: LocalInstallerLauncher,
    #[cfg(any(not(windows), test))]
    memory: Mutex<FollowMemory>,
}

impl ParakeetFollow {
    pub(crate) fn new(launcher: LocalInstallerLauncher) -> Self {
        // The Windows package carries Parakeet; this installer is never launched there.
        #[cfg(all(windows, not(test)))]
        let _ = launcher;
        Self {
            #[cfg(any(not(windows), test))]
            launcher,
            #[cfg(any(not(windows), test))]
            memory: Mutex::new(FollowMemory::default()),
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn observe(&self, journal: &Path, artifact_key: &str) -> ParakeetFollowDecision {
        let model_installed_before = parakeet_model_installed_before(journal);
        let (cpu_proof, vulkan_proof, model_proof) =
            inspect_parakeet_manifests(journal, artifact_key);
        let record = status::read_status(journal, "parakeet").ok();
        let input = ParakeetFollowInput {
            cpu_proof: &cpu_proof,
            vulkan_proof: &vulkan_proof,
            model_proof: &model_proof,
            desired: true,
            blocked: false,
            model_installed_before,
            record: record.as_ref(),
            now: Utc::now(),
        };
        self.observe_at(
            journal,
            &input,
            || lease::is_held(journal, "parakeet"),
            Instant::now(),
        )
    }

    #[cfg(any(not(windows), test))]
    pub(crate) fn observe_at(
        &self,
        journal: &Path,
        input: &ParakeetFollowInput<'_>,
        lease_held: impl FnOnce() -> std::io::Result<bool>,
        now_instant: Instant,
    ) -> ParakeetFollowDecision {
        let Ok(mut memory) = self.memory.lock() else {
            return ParakeetFollowDecision::Hold("follow-state-unavailable".to_owned());
        };

        if proof_is_ready(input.cpu_proof)
            && proof_is_ready(input.vulkan_proof)
            && proof_is_ready(input.model_proof)
        {
            memory.reset();
            return ParakeetFollowDecision::Hold("ready".to_owned());
        }

        let decision = decide(input, memory.backoff_elapsed(now_instant), lease_held);

        if decision == ParakeetFollowDecision::Launch {
            memory.record_launch(now_instant);
            let why = if input.model_installed_before {
                "parakeet provider artifacts are behind this release's pins"
            } else {
                "parakeet speech recognition is not installed in this journal"
            };
            match (self.launcher)(journal) {
                Ok(()) => log::warn!("{why}; started the installer"),
                Err(error) => log::warn!("{why}; installer did not start: {error}"),
            }
        }

        decision
    }
}

#[cfg(any(not(windows), test))]
fn proof_is_ready(proof: &Value) -> bool {
    proof.get("status").and_then(Value::as_str) == Some("ready")
}

#[cfg(any(not(windows), test))]
fn decide(
    input: &ParakeetFollowInput<'_>,
    launch_backoff_elapsed: bool,
    lease_held: impl FnOnce() -> std::io::Result<bool>,
) -> ParakeetFollowDecision {
    if input.blocked {
        return ParakeetFollowDecision::Hold("host-admission-blocked".to_owned());
    }
    if !input.desired {
        return ParakeetFollowDecision::Hold("provider-not-needed".to_owned());
    }
    let mut moved = false;
    for proof in [input.cpu_proof, input.vulkan_proof, input.model_proof] {
        match proof.get("status").and_then(Value::as_str) {
            Some("ready") => {}
            Some("missing-or-mismatched") => {
                let reason = proof.get("reason_code").and_then(Value::as_str);
                if reason.is_some_and(|r| PIN_MOVE_REASONS.contains(&r)) {
                    moved = true;
                } else {
                    let code = reason.unwrap_or("manifest_malformed");
                    return ParakeetFollowDecision::Hold(code.to_owned());
                }
            }
            _ => {
                let code = proof
                    .get("reason_code")
                    .and_then(Value::as_str)
                    .unwrap_or("manifest_malformed");
                return ParakeetFollowDecision::Hold(code.to_owned());
            }
        }
    }

    if !moved {
        return ParakeetFollowDecision::Hold("ready".to_owned());
    }

    match install_follow_tail(input.record, input.now, launch_backoff_elapsed, lease_held) {
        FollowDecision::Launch => ParakeetFollowDecision::Launch,
        FollowDecision::Hold(reason) => ParakeetFollowDecision::Hold(reason.to_owned()),
    }
}

#[cfg(not(windows))]
fn parakeet_model_installed_before(journal: &Path) -> bool {
    let models_root = pins::parakeet_cache_root(journal).join("models");
    let Ok(repo_entries) = std::fs::read_dir(models_root) else {
        return false;
    };
    for repo_entry in repo_entries.filter_map(Result::ok) {
        let repo_name = repo_entry.file_name();
        if repo_name.to_string_lossy().starts_with('.') || !repo_entry.path().is_dir() {
            continue;
        }
        let Ok(rev_entries) = std::fs::read_dir(repo_entry.path()) else {
            continue;
        };
        for rev_entry in rev_entries.filter_map(Result::ok) {
            let rev_name = rev_entry.file_name();
            if rev_name.to_string_lossy().starts_with('.') || !rev_entry.path().is_dir() {
                continue;
            }
            if manifest::artifact_manifest_path(&rev_entry.path()).is_file() {
                return true;
            }
        }
    }
    false
}

pub(crate) fn inspect_parakeet_manifests(
    journal: &Path,
    artifact_key: &str,
) -> (Value, Value, Value) {
    solstone_core_local::install::inspect_parakeet_manifests(journal, artifact_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    #[cfg(feature = "full-tests")]
    use std::sync::mpsc::channel;
    use std::time::Duration;

    fn proof_ready() -> Value {
        json!({"status": "ready", "reason_code": "ready"})
    }

    fn proof_missing() -> Value {
        json!({"status": "missing-or-mismatched", "reason_code": "manifest_missing"})
    }

    fn proof_mismatch() -> Value {
        json!({"status": "missing-or-mismatched", "reason_code": "manifest_pin_mismatch"})
    }

    fn proof_with_reason(reason: &str) -> Value {
        json!({"status": "missing-or-mismatched", "reason_code": reason})
    }

    fn record_installed(at: &str) -> InstallStatus {
        InstallStatus {
            schema_version: 1,
            provider: "parakeet".into(),
            revision: 1,
            install_state: "installed".into(),
            attempt_id: None,
            target_fingerprint_json: None,
            target_fingerprint_sha256: None,
            started_at: Some(at.into()),
            last_transition_at: Some(at.into()),
            last_progress_at: None,
            completed_at: Some(at.into()),
            progress_bytes_received: None,
            progress_bytes_total: None,
            install_error: None,
            error_code: None,
            owner: None,
            admission_warning: None,
        }
    }

    fn idle_record() -> InstallStatus {
        // What `status::read_status` returns when no installer ever ran.
        solstone_core_local::install::status::idle_status("parakeet")
    }

    fn record_failed_cancelled(at: &str) -> InstallStatus {
        let mut r = record_installed(at);
        r.install_state = "failed".into();
        r.error_code = Some("install_cancelled".into());
        r
    }

    fn dummy_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-08T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn admitted_and_prior_install_with_moved_pin_launches() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();
        let follow = ParakeetFollow::new(Arc::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let cpu = proof_missing();
        let vulkan = proof_ready();
        let model = proof_ready();
        let record = record_installed("2026-09-01T12:00:00Z");
        let input = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };

        let decision = follow.observe_at(Path::new("/"), &input, || Ok(false), Instant::now());

        assert_eq!(decision, ParakeetFollowDecision::Launch);
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn all_three_ready_holds_ready_and_resets_spacing() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();
        let follow = ParakeetFollow::new(Arc::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let cpu = proof_missing();
        let vulkan = proof_ready();
        let model = proof_ready();
        let record = record_installed("2026-09-01T12:00:00Z");
        let moved_input = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };

        let now = Instant::now();
        // First launch
        let d1 = follow.observe_at(Path::new("/"), &moved_input, || Ok(false), now);
        assert_eq!(d1, ParakeetFollowDecision::Launch);
        assert_eq!(called.load(Ordering::SeqCst), 1);

        // Immediate next call would be backoff
        let d2 = follow.observe_at(Path::new("/"), &moved_input, || Ok(false), now);
        assert_eq!(d2, ParakeetFollowDecision::Hold("backoff".into()));
        assert_eq!(called.load(Ordering::SeqCst), 1);

        // Now observe all-ready
        let ready = proof_ready();
        let ready_input = ParakeetFollowInput {
            cpu_proof: &ready,
            vulkan_proof: &ready,
            model_proof: &ready,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };
        let d3 = follow.observe_at(Path::new("/"), &ready_input, || Ok(false), now);
        assert_eq!(d3, ParakeetFollowDecision::Hold("ready".into()));
        assert_eq!(called.load(Ordering::SeqCst), 1);

        // Immediate next moved call at same Instant now launches because memory was reset
        let d4 = follow.observe_at(Path::new("/"), &moved_input, || Ok(false), now);
        assert_eq!(d4, ParakeetFollowDecision::Launch);
        assert_eq!(called.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn never_installed_journal_that_wants_parakeet_starts_the_installer() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();
        let follow = ParakeetFollow::new(Arc::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let cpu = proof_missing();
        let vulkan = proof_missing();
        let model = proof_missing();
        let record = idle_record();
        let input = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: false,
            record: Some(&record),
            now: dummy_now(),
        };

        let now = Instant::now();
        let decision = follow.observe_at(Path::new("/"), &input, || Ok(false), now);
        assert_eq!(decision, ParakeetFollowDecision::Launch);
        assert_eq!(called.load(Ordering::SeqCst), 1);

        // The same spacing as a pin follow: no second launch right away.
        let again = follow.observe_at(Path::new("/"), &input, || Ok(false), now);
        assert_eq!(again, ParakeetFollowDecision::Hold("backoff".into()));
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn never_installed_journal_holds_for_owner_cancel_running_install_and_admission() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();
        let follow = ParakeetFollow::new(Arc::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        let missing = proof_missing();
        let cancelled = record_failed_cancelled("2026-09-01T12:00:00Z");
        let idle = idle_record();
        let mut input = ParakeetFollowInput {
            cpu_proof: &missing,
            vulkan_proof: &missing,
            model_proof: &missing,
            desired: true,
            blocked: false,
            model_installed_before: false,
            record: Some(&cancelled),
            now: dummy_now(),
        };
        assert_eq!(
            follow.observe_at(Path::new("/"), &input, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("owner-cancelled".into())
        );
        input.record = Some(&idle);
        assert_eq!(
            follow.observe_at(Path::new("/"), &input, || Ok(true), Instant::now()),
            ParakeetFollowDecision::Hold("install-running".into())
        );
        input.desired = false;
        assert_eq!(
            follow.observe_at(Path::new("/"), &input, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("provider-not-needed".into())
        );
        input.desired = true;
        input.blocked = true;
        assert_eq!(
            follow.observe_at(Path::new("/"), &input, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("host-admission-blocked".into())
        );
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn blocked_and_not_desired_hold_admission_reasons() {
        let follow = ParakeetFollow::new(Arc::new(|_| Ok(())));
        let cpu = proof_missing();
        let vulkan = proof_ready();
        let model = proof_ready();
        let record = record_installed("2026-09-01T12:00:00Z");

        let mut input = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: true,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };

        assert_eq!(
            follow.observe_at(Path::new("/"), &input, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("host-admission-blocked".into())
        );

        input.blocked = false;
        input.desired = false;
        assert_eq!(
            follow.observe_at(Path::new("/"), &input, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("provider-not-needed".into())
        );
    }

    #[test]
    fn non_followable_proof_reasons_hold_with_proof_reason_code() {
        let follow = ParakeetFollow::new(Arc::new(|_| Ok(())));
        let vulkan = proof_missing();
        let model = proof_mismatch();
        let record = record_installed("2026-09-01T12:00:00Z");

        let reasons = [
            "manifest_io_error",
            "manifest_malformed",
            "inventory_malformed",
            "inventory_member_missing",
            "inventory_size_mismatch",
            "expected_hash_unavailable",
        ];

        for reason in reasons {
            let cpu = proof_with_reason(reason);
            let input = ParakeetFollowInput {
                cpu_proof: &cpu,
                vulkan_proof: &vulkan,
                model_proof: &model,
                desired: true,
                blocked: false,
                model_installed_before: true,
                record: Some(&record),
                now: dummy_now(),
            };
            assert_eq!(
                follow.observe_at(Path::new("/"), &input, || Ok(false), Instant::now()),
                ParakeetFollowDecision::Hold(reason.into())
            );
        }
    }

    #[test]
    fn status_record_and_lease_preconditions() {
        let follow = ParakeetFollow::new(Arc::new(|_| Ok(())));
        let cpu = proof_missing();
        let vulkan = proof_ready();
        let model = proof_ready();
        let now = dummy_now();

        // Owner cancelled
        let rec_cancelled = record_failed_cancelled("2026-09-01T12:00:00Z");
        let input_cancelled = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&rec_cancelled),
            now,
        };
        assert_eq!(
            follow.observe_at(
                Path::new("/"),
                &input_cancelled,
                || Ok(false),
                Instant::now()
            ),
            ParakeetFollowDecision::Hold("owner-cancelled".into())
        );

        // Activity 15 min ago -> backoff
        let rec_recent = record_installed("2026-10-08T11:45:00Z");
        let input_recent = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&rec_recent),
            now,
        };
        assert_eq!(
            follow.observe_at(Path::new("/"), &input_recent, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("backoff".into())
        );

        // Future stamp -> install-status-in-future
        let rec_future = record_installed("2026-10-08T13:00:00Z");
        let input_future = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&rec_future),
            now,
        };
        assert_eq!(
            follow.observe_at(Path::new("/"), &input_future, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("install-status-in-future".into())
        );

        // Unparseable stamp -> install-status-unreadable
        let rec_unparseable = record_installed("not-a-timestamp");
        let input_unparseable = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&rec_unparseable),
            now,
        };
        assert_eq!(
            follow.observe_at(
                Path::new("/"),
                &input_unparseable,
                || Ok(false),
                Instant::now()
            ),
            ParakeetFollowDecision::Hold("install-status-unreadable".into())
        );

        // Record None -> install-status-unreadable
        let input_no_rec = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: None,
            now,
        };
        assert_eq!(
            follow.observe_at(Path::new("/"), &input_no_rec, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("install-status-unreadable".into())
        );

        // Lease Ok(true) -> install-running
        let rec_old = record_installed("2026-09-01T12:00:00Z");
        let input_ok = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&rec_old),
            now,
        };
        assert_eq!(
            follow.observe_at(Path::new("/"), &input_ok, || Ok(true), Instant::now()),
            ParakeetFollowDecision::Hold("install-running".into())
        );

        // Lease Err -> install-lease-unreadable
        assert_eq!(
            follow.observe_at(
                Path::new("/"),
                &input_ok,
                || Err(std::io::Error::other("err")),
                Instant::now()
            ),
            ParakeetFollowDecision::Hold("install-lease-unreadable".into())
        );
    }

    #[test]
    fn launcher_error_still_returns_launch_and_advances_backoff() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();
        let follow = ParakeetFollow::new(Arc::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Err("launcher failed".to_owned())
        }));

        let cpu = proof_missing();
        let vulkan = proof_ready();
        let model = proof_ready();
        let record = record_installed("2026-09-01T12:00:00Z");
        let input = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };

        let now = Instant::now();
        let decision1 = follow.observe_at(Path::new("/"), &input, || Ok(false), now);
        assert_eq!(decision1, ParakeetFollowDecision::Launch);
        assert_eq!(called.load(Ordering::SeqCst), 1);

        let decision2 = follow.observe_at(Path::new("/"), &input, || Ok(false), now);
        assert_eq!(decision2, ParakeetFollowDecision::Hold("backoff".into()));
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn spacing_steps_ladder_and_all_ready_reset() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();
        let follow = ParakeetFollow::new(Arc::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let cpu = proof_missing();
        let vulkan = proof_ready();
        let model = proof_ready();
        let record = record_installed("2026-09-01T12:00:00Z");
        let input = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };

        let base = Instant::now();
        // Launch 1 (at base)
        assert_eq!(
            follow.observe_at(Path::new("/"), &input, || Ok(false), base),
            ParakeetFollowDecision::Launch
        );
        assert_eq!(called.load(Ordering::SeqCst), 1);

        // Steps: 30m, 2h, 8h, 24h, 24h
        let steps = [
            Duration::from_secs(30 * 60),
            Duration::from_secs(2 * 60 * 60),
            Duration::from_secs(8 * 60 * 60),
            Duration::from_secs(24 * 60 * 60),
            Duration::from_secs(24 * 60 * 60),
        ];

        let mut current = base;
        for (i, step) in steps.iter().enumerate() {
            // step - 1s holds
            let just_before = current + *step - Duration::from_secs(1);
            assert_eq!(
                follow.observe_at(Path::new("/"), &input, || Ok(false), just_before),
                ParakeetFollowDecision::Hold("backoff".into())
            );
            assert_eq!(called.load(Ordering::SeqCst), (i + 1) as u32);

            // step launches
            let at_step = current + *step;
            assert_eq!(
                follow.observe_at(Path::new("/"), &input, || Ok(false), at_step),
                ParakeetFollowDecision::Launch
            );
            assert_eq!(called.load(Ordering::SeqCst), (i + 2) as u32);
            current = at_step;
        }

        // All-ready reset
        let ready = proof_ready();
        let ready_input = ParakeetFollowInput {
            cpu_proof: &ready,
            vulkan_proof: &ready,
            model_proof: &ready,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };
        assert_eq!(
            follow.observe_at(Path::new("/"), &ready_input, || Ok(false), current),
            ParakeetFollowDecision::Hold("ready".into())
        );

        // Next moved at current launches immediately
        assert_eq!(
            follow.observe_at(Path::new("/"), &input, || Ok(false), current),
            ParakeetFollowDecision::Launch
        );
    }

    #[test]
    fn independent_memory_instances_do_not_share_spacing() {
        let follow1 = ParakeetFollow::new(Arc::new(|_| Ok(())));
        let follow2 = ParakeetFollow::new(Arc::new(|_| Ok(())));

        let cpu = proof_missing();
        let vulkan = proof_ready();
        let model = proof_ready();
        let record = record_installed("2026-09-01T12:00:00Z");
        let input = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };

        let now = Instant::now();
        assert_eq!(
            follow1.observe_at(Path::new("/"), &input, || Ok(false), now),
            ParakeetFollowDecision::Launch
        );
        // follow2 is independent, launches at same now
        assert_eq!(
            follow2.observe_at(Path::new("/"), &input, || Ok(false), now),
            ParakeetFollowDecision::Launch
        );

        // Standalone FollowMemory record does not backoff a fresh follow
        let mut standalone_mem = FollowMemory::default();
        standalone_mem.record_launch(now);
        let follow3 = ParakeetFollow::new(Arc::new(|_| Ok(())));
        assert_eq!(
            follow3.observe_at(Path::new("/"), &input, || Ok(false), now),
            ParakeetFollowDecision::Launch
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn concurrent_observe_race_free_mutex_hold() {
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let release_rx = Mutex::new(release_rx);
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let follow = Arc::new(ParakeetFollow::new(Arc::new(move |_| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            Ok(())
        })));

        let follow_a = follow.clone();
        let follow_b = follow.clone();

        let cpu = proof_missing();
        let vulkan = proof_ready();
        let model = proof_ready();
        let record = record_installed("2026-09-01T12:00:00Z");
        let now_instant = Instant::now();

        let handle_a = std::thread::spawn(move || {
            let input = ParakeetFollowInput {
                cpu_proof: &cpu,
                vulkan_proof: &vulkan,
                model_proof: &model,
                desired: true,
                blocked: false,
                model_installed_before: true,
                record: Some(&record),
                now: dummy_now(),
            };
            follow_a.observe_at(Path::new("/"), &input, || Ok(false), now_instant)
        });

        // Wait until thread A has entered callback (lock held, launch recorded)
        entered_rx.recv().unwrap();

        // Now start thread B on same instant
        let cpu_b = proof_missing();
        let vulkan_b = proof_ready();
        let model_b = proof_ready();
        let record_b = record_installed("2026-09-01T12:00:00Z");
        let handle_b = std::thread::spawn(move || {
            let input = ParakeetFollowInput {
                cpu_proof: &cpu_b,
                vulkan_proof: &vulkan_b,
                model_proof: &model_b,
                desired: true,
                blocked: false,
                model_installed_before: true,
                record: Some(&record_b),
                now: dummy_now(),
            };
            follow_b.observe_at(Path::new("/"), &input, || Ok(false), now_instant)
        });

        // Release thread A
        release_tx.send(()).unwrap();

        let res_a = handle_a.join().unwrap();
        let res_b = handle_b.join().unwrap();

        assert_eq!(res_a, ParakeetFollowDecision::Launch);
        assert_eq!(res_b, ParakeetFollowDecision::Hold("backoff".into()));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn boot_version_record_does_not_affect_parakeet_follow_decisions() {
        let journal = tempfile::tempdir().unwrap();
        let health = journal.path().join("health");
        std::fs::create_dir_all(&health).unwrap();
        let boot_path = health.join("boot-version.json");

        let launches = Arc::new(AtomicU32::new(0));
        let launches_clone = launches.clone();
        let launcher: LocalInstallerLauncher = Arc::new(move |_| {
            launches_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        let cpu = proof_missing();
        let vulkan = proof_ready();
        let model = proof_ready();
        let record = record_installed("2026-09-01T12:00:00Z");
        let moved_input = ParakeetFollowInput {
            cpu_proof: &cpu,
            vulkan_proof: &vulkan,
            model_proof: &model,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };

        std::fs::write(
            &boot_path,
            serde_json::json!({ "schema": 1, "product_version": env!("CARGO_PKG_VERSION") })
                .to_string(),
        )
        .unwrap();
        let follow1 = ParakeetFollow::new(Arc::clone(&launcher));
        assert_eq!(
            follow1.observe_at(journal.path(), &moved_input, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Launch
        );
        assert_eq!(launches.load(Ordering::SeqCst), 1);

        std::fs::write(
            &boot_path,
            serde_json::json!({ "schema": 1, "product_version": "0.0.0-older" }).to_string(),
        )
        .unwrap();
        let follow2 = ParakeetFollow::new(Arc::clone(&launcher));
        assert_eq!(
            follow2.observe_at(journal.path(), &moved_input, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Launch
        );
        assert_eq!(launches.load(Ordering::SeqCst), 2);

        let ready = proof_ready();
        let ready_input = ParakeetFollowInput {
            cpu_proof: &ready,
            vulkan_proof: &ready,
            model_proof: &ready,
            desired: true,
            blocked: false,
            model_installed_before: true,
            record: Some(&record),
            now: dummy_now(),
        };
        let follow3 = ParakeetFollow::new(Arc::clone(&launcher));
        assert_eq!(
            follow3.observe_at(journal.path(), &ready_input, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("ready".to_owned())
        );
        let follow4 = ParakeetFollow::new(Arc::clone(&launcher));
        assert_eq!(
            follow4.observe_at(journal.path(), &ready_input, || Ok(false), Instant::now()),
            ParakeetFollowDecision::Hold("ready".to_owned())
        );
        assert_eq!(launches.load(Ordering::SeqCst), 2);
    }
}
