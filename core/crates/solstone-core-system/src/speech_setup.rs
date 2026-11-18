// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::stt_backend_choice::SttRuntimeKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechSetupState {
    Waiting,
    Downloading,
    Verifying,
    Installing,
    Paused,
    Cooling,
    Attention,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechSetupTone {
    Neutral,
    Calm,
    Attention,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechSetupPresentation {
    pub applicable: bool,
    pub visible: bool,
    pub state: Option<SpeechSetupState>,
    pub tone: Option<SpeechSetupTone>,
    pub bytes_received: Option<u64>,
    pub bytes_total: Option<u64>,
    pub cause: Option<String>,
    pub recovery_representable: bool,
    pub command: Option<String>,
}

impl SpeechSetupPresentation {
    pub fn hidden(applicable: bool) -> Self {
        Self {
            applicable,
            visible: false,
            state: None,
            tone: None,
            bytes_received: None,
            bytes_total: None,
            cause: None,
            recovery_representable: false,
            command: None,
        }
    }

    pub fn unavailable() -> Self {
        Self {
            applicable: true,
            visible: true,
            state: Some(SpeechSetupState::Unavailable),
            tone: Some(SpeechSetupTone::Unavailable),
            bytes_received: None,
            bytes_total: None,
            cause: None,
            recovery_representable: false,
            command: None,
        }
    }
}

pub fn read_speech_setup(
    journal: &Path,
    os: &str,
    arch: &str,
    home: Option<&Path>,
    now: Timestamp,
) -> SpeechSetupPresentation {
    let policy = crate::stt_backend_choice::stt_host_policy(os, arch);
    let kind = match policy.runtime_kind {
        Some(k @ (SttRuntimeKind::LinuxCpp | SttRuntimeKind::MacosCoreml)) => k,
        _ => return SpeechSetupPresentation::hidden(false),
    };

    let platform_command = match kind {
        SttRuntimeKind::LinuxCpp => "solstone journal install-provider parakeet",
        SttRuntimeKind::MacosCoreml => "solstone journal install-models --force --variant coreml",
        _ => unreachable!(),
    };

    // 2 & 3. Linux checks
    let mut runtime_obs = crate::provider_runtime::RuntimeHealthObservation::Absent;
    if kind == SttRuntimeKind::LinuxCpp {
        if let Ok(artifact_key) =
            solstone_core_local::install::pins::parakeet_artifact_key(os, arch)
        {
            let (cpu_proof, vulkan_proof, model_proof) =
                crate::provider_runtime::inspect_parakeet_manifests(journal, &artifact_key);
            if proof_ready(&cpu_proof) && proof_ready(&vulkan_proof) && proof_ready(&model_proof) {
                return SpeechSetupPresentation::hidden(true);
            }
        }

        runtime_obs = crate::provider_runtime::observe_runtime_health(
            journal,
            crate::provider_runtime::ProviderName::Parakeet,
        );
        if let crate::provider_runtime::RuntimeHealthObservation::Present { phase, detail } =
            &runtime_obs
            && (*phase == crate::provider_runtime::RuntimePhase::NotDesired
                || detail.get("pin_follow").and_then(|v| v.as_str()) == Some("provider-not-needed"))
        {
            return SpeechSetupPresentation::hidden(true);
        }
    }

    // 4. macOS checks
    if kind == SttRuntimeKind::MacosCoreml {
        let home_dir = match home {
            Some(h) if h.is_absolute() => h,
            _ => return SpeechSetupPresentation::unavailable(),
        };
        let journal_config = match solstone_core_journal_config::read_journal_config(journal) {
            Ok(c) => c,
            Err(_) => return SpeechSetupPresentation::unavailable(),
        };
        let cache_dir = solstone_core_journal_config::parakeet_coreml::parakeet_coreml_cache_dir(
            &journal_config,
            home_dir,
        );
        if solstone_core_local::install::coreml_install::parakeet_coreml_installed(
            home_dir, &cache_dir,
        ) {
            return SpeechSetupPresentation::hidden(true);
        }
    }

    // 5. Required read unreadable or set aside -> unavailable
    if kind == SttRuntimeKind::LinuxCpp
        && matches!(
            runtime_obs,
            crate::provider_runtime::RuntimeHealthObservation::Unreadable
                | crate::provider_runtime::RuntimeHealthObservation::SetAside
        )
    {
        return SpeechSetupPresentation::unavailable();
    }

    let observed_status_res =
        solstone_core_local::install::status::read_observed_status(journal, "parakeet");
    let status_record = match observed_status_res {
        Ok(s) => s,
        Err(_) => return SpeechSetupPresentation::unavailable(),
    };

    let (runtime_phase, pin_follow) = match &runtime_obs {
        crate::provider_runtime::RuntimeHealthObservation::Present { phase, detail } => (
            Some(*phase),
            detail.get("pin_follow").and_then(|v| v.as_str()),
        ),
        _ => (None, None),
    };

    if matches!(
        pin_follow,
        Some("install-status-unreadable" | "install-lease-unreadable" | "install-status-in-future")
    ) {
        return SpeechSetupPresentation::unavailable();
    }

    let cause = status_record
        .error_code
        .clone()
        .or_else(|| status_record.install_error.clone());

    // 6. In-flight status
    if solstone_core_local::install::status::is_in_flight(&status_record.install_state) {
        let state = match status_record.install_state.as_str() {
            "downloading" => SpeechSetupState::Downloading,
            "verifying" => SpeechSetupState::Verifying,
            "installing" | "resolving" => SpeechSetupState::Installing,
            _ => SpeechSetupState::Installing,
        };
        return SpeechSetupPresentation {
            applicable: true,
            visible: true,
            state: Some(state),
            tone: Some(SpeechSetupTone::Neutral),
            bytes_received: status_record.progress_bytes_received,
            bytes_total: status_record.progress_bytes_total,
            cause: None,
            recovery_representable: true,
            command: None,
        };
    }

    // 7. Paused
    if status_record.error_code.as_deref() == Some("install_cancelled")
        || pin_follow == Some("owner-cancelled")
    {
        return SpeechSetupPresentation {
            applicable: true,
            visible: true,
            state: Some(SpeechSetupState::Paused),
            tone: Some(SpeechSetupTone::Calm),
            bytes_received: None,
            bytes_total: None,
            cause,
            recovery_representable: true,
            command: Some(platform_command.to_string()),
        };
    }

    // 8. Cooling
    let is_cooling = match kind {
        SttRuntimeKind::LinuxCpp => {
            pin_follow == Some("backoff") && status_record.install_state == "failed"
        }
        SttRuntimeKind::MacosCoreml if status_record.install_state == "failed" => status_record
            .last_transition_at
            .as_deref()
            .and_then(|stamp| solstone_core_time::parse_rfc3339(stamp).ok())
            .and_then(|at| std::time::Duration::try_from(now.duration_since(at.timestamp())).ok())
            .is_some_and(|since| {
                since < solstone_core_local::install::coreml_install::COREML_HEAL_COOLDOWN
            }),
        _ => false,
    };

    if is_cooling {
        return SpeechSetupPresentation {
            applicable: true,
            visible: true,
            state: Some(SpeechSetupState::Cooling),
            tone: Some(SpeechSetupTone::Attention),
            bytes_received: None,
            bytes_total: None,
            cause,
            recovery_representable: true,
            command: Some(platform_command.to_string()),
        };
    }

    // Host-blocked attention
    let is_host_blocked = runtime_phase == Some(crate::provider_runtime::RuntimePhase::HostBlocked)
        || pin_follow == Some("host-admission-blocked");

    if is_host_blocked {
        return SpeechSetupPresentation {
            applicable: true,
            visible: true,
            state: Some(SpeechSetupState::Attention),
            tone: Some(SpeechSetupTone::Attention),
            bytes_received: None,
            bytes_total: None,
            cause,
            recovery_representable: false,
            command: None,
        };
    }

    // 9. Waiting
    let is_waiting = (pin_follow == Some("backoff") && status_record.install_state != "failed")
        || pin_follow == Some("launch")
        || (pin_follow == Some("install-running")
            && !solstone_core_local::install::status::is_in_flight(&status_record.install_state))
        || ((pin_follow.is_none() || pin_follow == Some(""))
            && status_record.install_state == "idle");

    if is_waiting {
        return SpeechSetupPresentation {
            applicable: true,
            visible: true,
            state: Some(SpeechSetupState::Waiting),
            tone: Some(SpeechSetupTone::Neutral),
            bytes_received: None,
            bytes_total: None,
            cause,
            recovery_representable: true,
            command: Some(platform_command.to_string()),
        };
    }

    // 10. Attention
    let is_attention = status_record.error_code.as_deref() == Some("install_interrupted")
        || status_record.install_state == "failed"
        || status_record.install_state == "installed";

    if is_attention {
        return SpeechSetupPresentation {
            applicable: true,
            visible: true,
            state: Some(SpeechSetupState::Attention),
            tone: Some(SpeechSetupTone::Attention),
            bytes_received: None,
            bytes_total: None,
            cause,
            recovery_representable: true,
            command: Some(platform_command.to_string()),
        };
    }

    // 11. Fallback
    SpeechSetupPresentation::unavailable()
}

fn proof_ready(proof: &serde_json::Value) -> bool {
    proof.get("status").and_then(|s| s.as_str()) == Some("ready")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use solstone_core_journal_config::parakeet_coreml::{
        ParakeetCoremlSentinel, default_parakeet_coreml_cache_dir, parakeet_coreml_model_root,
        parakeet_coreml_sentinel_path,
    };
    use solstone_core_local::install::manifest::{
        artifact_manifest_path, build_manifest, write_manifest,
    };
    use solstone_core_local::install::pins;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_install_status(
        journal: &Path,
        state: &str,
        error_code: Option<&str>,
        install_error: Option<&str>,
        bytes: Option<(u64, u64)>,
        last_transition_at: Option<&str>,
    ) {
        let path = journal.join("health/providers/parakeet.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = json!({
            "schema_version": 1,
            "provider": "parakeet",
            "revision": 1,
            "install_state": state,
            "attempt_id": "attempt-1",
            "target_fingerprint_json": "fp",
            "target_fingerprint_sha256": "sha",
            "started_at": null,
            "last_transition_at": last_transition_at,
            "last_progress_at": null,
            "completed_at": null,
            "progress_bytes_received": bytes.map(|b| b.0),
            "progress_bytes_total": bytes.map(|b| b.1),
            "install_error": install_error,
            "error_code": error_code,
            "owner": null
        });
        fs::write(path, serde_json::to_vec(&payload).unwrap()).unwrap();
    }

    fn write_runtime_health(journal: &Path, phase: &str, pin_follow: Option<&str>) {
        let path = journal.join("health/providers/runtime/parakeet.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let detail = if let Some(pf) = pin_follow {
            json!({ "pin_follow": pf })
        } else {
            json!({})
        };
        let payload = json!({
            "schema_version": 1,
            "provider": "parakeet",
            "phase": phase,
            "detail": detail,
            "reason_code": null,
            "owner": null,
            "revision": 1,
            "generation": 1,
            "attempt": 1
        });
        fs::write(path, serde_json::to_vec(&payload).unwrap()).unwrap();
    }

    fn write_linux_proof(
        manifest_dir: &Path,
        unit: &str,
        pin_identity: serde_json::Value,
        filename: &str,
    ) {
        fs::create_dir_all(manifest_dir).unwrap();
        let file_path = manifest_dir.join(filename);
        let content = b"binary content";
        fs::write(&file_path, content).unwrap();

        let mut hasher = Sha256::new();
        hasher.update(content);
        let sha256_hex = format!("{:02x}", hasher.finalize());

        let inventory = vec![json!({
            "relative_path": filename,
            "role": "binary",
            "size": content.len() as u64,
            "sha256": sha256_hex,
        })];

        let manifest_val = build_manifest(
            "parakeet",
            unit,
            "fingerprint",
            json!({ "pin_identity": pin_identity }),
            inventory,
            None,
            None,
        )
        .unwrap();

        write_manifest(&artifact_manifest_path(manifest_dir), &manifest_val).unwrap();
    }

    fn setup_linux_proofs_ready(journal: &Path) {
        let key = "x86_64-unknown-linux-gnu";
        let paths = pins::parakeet_paths(journal, key);
        let cpu_path = PathBuf::from(paths["binary_path_cpu"].as_str().unwrap());
        let vulkan_path = PathBuf::from(paths["binary_path_vulkan"].as_str().unwrap());
        let model_path = PathBuf::from(paths["model_path"].as_str().unwrap());

        let (_, _, _, cpu_bin) = pins::parakeet_backend_pin(key, "cpu").unwrap();
        let (_, _, _, vulkan_bin) = pins::parakeet_backend_pin(key, "vulkan").unwrap();
        let (_, model_filename, ..) = pins::PARAKEET_MODEL;

        write_linux_proof(
            cpu_path.parent().unwrap(),
            "parakeet-server",
            pins::parakeet_backend_identity(key, "cpu").unwrap(),
            cpu_bin,
        );
        write_linux_proof(
            vulkan_path.parent().unwrap(),
            "parakeet-server",
            pins::parakeet_backend_identity(key, "vulkan").unwrap(),
            vulkan_bin,
        );
        write_linux_proof(
            model_path.parent().unwrap(),
            "parakeet-model",
            pins::parakeet_model_identity(),
            model_filename,
        );
    }

    fn setup_macos_sentinel_ready(home_dir: &Path) {
        let cache_dir = default_parakeet_coreml_cache_dir(home_dir);
        let model_root = parakeet_coreml_model_root(&cache_dir);
        fs::create_dir_all(&model_root).unwrap();
        fs::write(model_root.join("model_file.bin"), b"dummy").unwrap();

        let sentinel =
            ParakeetCoremlSentinel::new(cache_dir, "darwin", "arm64", "0.14.0", "1.0", vec![]);
        let sentinel_path = parakeet_coreml_sentinel_path(home_dir);
        fs::create_dir_all(sentinel_path.parent().unwrap()).unwrap();
        fs::write(sentinel_path, serde_json::to_vec(&sentinel).unwrap()).unwrap();
    }

    #[test]
    fn test_non_applicable_platforms() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        // Windows with garbage install file
        let bad_path = temp.path().join("health/providers/parakeet.json");
        fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
        fs::write(&bad_path, b"{ garbage json").unwrap();

        let pres = read_speech_setup(temp.path(), "windows", "x86_64", None, now);
        assert!(!pres.applicable);
        assert!(!pres.visible);
        assert_eq!(pres.state, None);
        assert_eq!(pres.tone, None);
        assert_eq!(pres.bytes_received, None);
        assert_eq!(pres.bytes_total, None);
        assert_eq!(pres.cause, None);
        assert!(!pres.recovery_representable);
        assert_eq!(pres.command, None);

        // Intel Mac
        let pres_intel = read_speech_setup(temp.path(), "darwin", "x86_64", None, now);
        assert!(!pres_intel.applicable);
        assert!(!pres_intel.visible);
    }

    #[test]
    fn test_linux_ready_proofs_hidden() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        setup_linux_proofs_ready(temp.path());

        for code in ["some_err", "install_cancelled"] {
            write_install_status(temp.path(), "failed", Some(code), None, None, None);
            for phase in &["starting", "warming", "stopped", "running"] {
                write_runtime_health(temp.path(), phase, None);
                let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
                assert!(pres.applicable);
                assert!(!pres.visible);
                assert_eq!(pres.state, None);
                assert_eq!(pres.command, None);
            }
        }
    }

    #[test]
    fn test_linux_not_desired_phases_hidden() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        write_runtime_health(temp.path(), "not-desired", None);
        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert!(pres.applicable);
        assert!(!pres.visible);

        write_runtime_health(temp.path(), "stopped", Some("provider-not-needed"));
        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert!(pres.applicable);
        assert!(!pres.visible);
    }

    #[test]
    fn test_macos_ready_sentinel_hidden() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("fake_home");
        fs::create_dir_all(&home).unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        setup_macos_sentinel_ready(&home);
        for code in ["fail_code", "install_cancelled"] {
            write_install_status(temp.path(), "failed", Some(code), None, None, None);
            let pres = read_speech_setup(temp.path(), "darwin", "arm64", Some(&home), now);
            assert!(pres.applicable);
            assert!(!pres.visible);
            assert_eq!(pres.command, None);
        }
    }

    #[test]
    fn test_macos_missing_or_relative_home() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        let pres_none = read_speech_setup(temp.path(), "darwin", "arm64", None, now);
        assert!(pres_none.applicable);
        assert!(pres_none.visible);
        assert_eq!(pres_none.state, Some(SpeechSetupState::Unavailable));
        assert_eq!(pres_none.tone, Some(SpeechSetupTone::Unavailable));
        assert!(!pres_none.recovery_representable);
        assert_eq!(pres_none.command, None);

        let pres_rel = read_speech_setup(
            temp.path(),
            "darwin",
            "arm64",
            Some(Path::new("rel/path")),
            now,
        );
        assert_eq!(pres_rel.state, Some(SpeechSetupState::Unavailable));
    }

    #[test]
    fn test_malformed_runtime_file_untouched() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        let path = temp.path().join("health/providers/runtime/parakeet.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let malformed = b"{\"not\": \"valid json";
        fs::write(&path, malformed).unwrap();

        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert!(pres.applicable);
        assert!(pres.visible);
        assert_eq!(pres.state, Some(SpeechSetupState::Unavailable));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Unavailable));
        assert_eq!(pres.command, None);

        // Check that the file was left untouched
        let after = fs::read(&path).unwrap();
        assert_eq!(after, malformed);

        let entries: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(!entries.iter().any(|name| name.contains(".wedged-")));
    }

    #[test]
    fn test_in_flight_downloading_with_lease_held() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        write_install_status(
            temp.path(),
            "downloading",
            None,
            None,
            Some((1024, 2048)),
            None,
        );

        // Hold lease
        let lease = solstone_core_local::install::lease::acquire(temp.path(), "parakeet").unwrap();
        assert!(lease.is_some());

        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert!(pres.applicable);
        assert!(pres.visible);
        assert_eq!(pres.state, Some(SpeechSetupState::Downloading));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Neutral));
        assert_eq!(pres.bytes_received, Some(1024));
        assert_eq!(pres.bytes_total, Some(2048));
        assert_eq!(pres.cause, None);
        assert!(pres.recovery_representable);
        assert_eq!(pres.command, None);

        drop(lease);

        // Without lease -> interrupted -> attention
        let pres_dropped = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_dropped.state, Some(SpeechSetupState::Attention));
        assert_eq!(pres_dropped.tone, Some(SpeechSetupTone::Attention));
        assert_eq!(pres_dropped.cause, Some("install_interrupted".to_string()));
        assert_eq!(pres_dropped.bytes_received, None);
        assert_eq!(pres_dropped.bytes_total, None);
        assert_eq!(
            pres_dropped.command,
            Some("solstone journal install-provider parakeet".to_string())
        );
    }

    #[test]
    fn test_paused_states() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        write_install_status(
            temp.path(),
            "failed",
            Some("install_cancelled"),
            None,
            None,
            None,
        );
        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres.state, Some(SpeechSetupState::Paused));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Calm));
        assert_eq!(pres.cause, Some("install_cancelled".to_string()));
        assert_eq!(
            pres.command,
            Some("solstone journal install-provider parakeet".to_string())
        );

        // pin_follow == owner-cancelled
        write_install_status(temp.path(), "failed", None, None, None, None);
        write_runtime_health(temp.path(), "stopped", Some("owner-cancelled"));
        let pres_pin = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_pin.state, Some(SpeechSetupState::Paused));
        assert_eq!(pres_pin.tone, Some(SpeechSetupTone::Calm));
    }

    #[test]
    fn test_cooling_and_waiting() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        // Linux backoff with failed -> cooling
        write_install_status(temp.path(), "failed", Some("err_1"), None, None, None);
        write_runtime_health(temp.path(), "stopped", Some("backoff"));
        let pres_cooling = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_cooling.state, Some(SpeechSetupState::Cooling));
        assert_eq!(pres_cooling.tone, Some(SpeechSetupTone::Attention));
        assert_eq!(pres_cooling.cause, Some("err_1".to_string()));
        assert_eq!(
            pres_cooling.command,
            Some("solstone journal install-provider parakeet".to_string())
        );

        // Linux backoff with non-failed -> waiting
        write_install_status(temp.path(), "idle", None, None, None, None);
        write_runtime_health(temp.path(), "stopped", Some("backoff"));
        let pres_waiting = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_waiting.state, Some(SpeechSetupState::Waiting));
        assert_eq!(pres_waiting.tone, Some(SpeechSetupTone::Neutral));

        // Linux idle with absent pin_follow -> waiting
        write_runtime_health(temp.path(), "stopped", None);
        let pres_idle = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_idle.state, Some(SpeechSetupState::Waiting));

        // macOS cooling within cooldown
        let home = temp.path().join("fake_home_cooling");
        fs::create_dir_all(&home).unwrap();
        let recent_transition =
            solstone_core_time::utc_rfc3339(now - jiff::SignedDuration::from_mins(5));
        write_install_status(
            temp.path(),
            "failed",
            Some("mac_err"),
            None,
            None,
            Some(&recent_transition),
        );
        let pres_mac_cooling = read_speech_setup(temp.path(), "darwin", "arm64", Some(&home), now);
        assert_eq!(pres_mac_cooling.state, Some(SpeechSetupState::Cooling));
        assert_eq!(pres_mac_cooling.tone, Some(SpeechSetupTone::Attention));
        assert_eq!(pres_mac_cooling.cause, Some("mac_err".to_string()));
        assert_eq!(
            pres_mac_cooling.command,
            Some("solstone journal install-models --force --variant coreml".to_string())
        );

        // macOS past cooldown -> attention
        let old_transition =
            solstone_core_time::utc_rfc3339(now - jiff::SignedDuration::from_mins(40));
        write_install_status(
            temp.path(),
            "failed",
            Some("mac_err"),
            None,
            None,
            Some(&old_transition),
        );
        let pres_mac_attention =
            read_speech_setup(temp.path(), "darwin", "arm64", Some(&home), now);
        assert_eq!(pres_mac_attention.state, Some(SpeechSetupState::Attention));
    }

    #[test]
    fn test_host_blocked_attention() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        write_runtime_health(temp.path(), "host-blocked", None);
        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres.state, Some(SpeechSetupState::Attention));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Attention));
        assert!(!pres.recovery_representable);
        assert_eq!(pres.command, None);

        write_runtime_health(temp.path(), "stopped", Some("host-admission-blocked"));
        let pres_pin = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_pin.state, Some(SpeechSetupState::Attention));
        assert_eq!(pres_pin.tone, Some(SpeechSetupTone::Attention));
        assert!(!pres_pin.recovery_representable);
        assert_eq!(pres_pin.command, None);
    }

    #[test]
    fn test_installed_record_without_proofs_is_attention() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        // Installed in parakeet.json, but proofs are NOT ready on disk
        write_install_status(temp.path(), "installed", None, None, None, None);
        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert!(pres.applicable);
        assert!(pres.visible);
        assert_eq!(pres.state, Some(SpeechSetupState::Attention));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Attention));
        assert!(pres.recovery_representable);
        assert_eq!(
            pres.command,
            Some("solstone journal install-provider parakeet".to_string())
        );

        // On macOS: installed record without sentinel is attention
        let home = temp.path().join("home_mac");
        fs::create_dir_all(&home).unwrap();
        let pres_mac = read_speech_setup(temp.path(), "darwin", "arm64", Some(&home), now);
        assert!(pres_mac.applicable);
        assert!(pres_mac.visible);
        assert_eq!(pres_mac.state, Some(SpeechSetupState::Attention));
        assert_eq!(
            pres_mac.command,
            Some("solstone journal install-models --force --variant coreml".to_string())
        );
    }

    #[test]
    fn test_unreadable_follow_pins_are_unavailable() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        write_install_status(temp.path(), "idle", None, None, None, None);

        for pin in &[
            "install-status-unreadable",
            "install-lease-unreadable",
            "install-status-in-future",
        ] {
            write_runtime_health(temp.path(), "stopped", Some(pin));
            let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
            assert!(pres.applicable);
            assert!(pres.visible);
            assert_eq!(pres.state, Some(SpeechSetupState::Unavailable));
            assert_eq!(pres.tone, Some(SpeechSetupTone::Unavailable));
            assert_eq!(pres.command, None);
            assert!(!pres.recovery_representable);
        }
    }

    #[test]
    fn test_in_flight_downloading_with_backoff_pin_is_downloading() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        write_install_status(
            temp.path(),
            "downloading",
            None,
            None,
            Some((1024, 2048)),
            None,
        );
        write_runtime_health(temp.path(), "stopped", Some("backoff"));

        let lease = solstone_core_local::install::lease::acquire(temp.path(), "parakeet").unwrap();
        assert!(lease.is_some());

        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert!(pres.applicable);
        assert!(pres.visible);
        assert_eq!(pres.state, Some(SpeechSetupState::Downloading));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Neutral));
        assert_eq!(pres.bytes_received, Some(1024));
        assert_eq!(pres.bytes_total, Some(2048));
        assert_eq!(pres.command, None);
        assert!(pres.recovery_representable);
    }

    #[test]
    fn test_in_flight_other_stages_with_lease_held() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        let lease = solstone_core_local::install::lease::acquire(temp.path(), "parakeet").unwrap();
        assert!(lease.is_some());

        write_install_status(temp.path(), "verifying", None, None, None, None);
        let pres_verifying = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_verifying.state, Some(SpeechSetupState::Verifying));
        assert_eq!(pres_verifying.tone, Some(SpeechSetupTone::Neutral));

        write_install_status(temp.path(), "resolving", None, None, None, None);
        let pres_resolving = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_resolving.state, Some(SpeechSetupState::Installing));
        assert_eq!(pres_resolving.tone, Some(SpeechSetupTone::Neutral));

        write_install_status(temp.path(), "installing", None, None, None, None);
        let pres_installing = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_installing.state, Some(SpeechSetupState::Installing));
        assert_eq!(pres_installing.tone, Some(SpeechSetupTone::Neutral));
    }

    #[test]
    fn test_no_runtime_and_no_install_file_is_waiting() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert!(pres.applicable);
        assert!(pres.visible);
        assert_eq!(pres.state, Some(SpeechSetupState::Waiting));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Neutral));
        assert_eq!(
            pres.command,
            Some("solstone journal install-provider parakeet".to_string())
        );
        assert!(pres.recovery_representable);
    }

    #[test]
    fn test_pin_follow_launch_and_install_running_with_idle_is_waiting() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        write_install_status(temp.path(), "idle", None, None, None, None);

        write_runtime_health(temp.path(), "stopped", Some("launch"));
        let pres_launch = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_launch.state, Some(SpeechSetupState::Waiting));
        assert_eq!(pres_launch.tone, Some(SpeechSetupTone::Neutral));

        write_runtime_health(temp.path(), "stopped", Some("install-running"));
        let pres_ir = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres_ir.state, Some(SpeechSetupState::Waiting));
        assert_eq!(pres_ir.tone, Some(SpeechSetupTone::Neutral));
    }

    #[test]
    fn test_malformed_install_status_file_is_unavailable_and_unchanged() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        let path = temp.path().join("health/providers/parakeet.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let malformed = b"{";
        fs::write(&path, malformed).unwrap();

        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert!(pres.applicable);
        assert!(pres.visible);
        assert_eq!(pres.state, Some(SpeechSetupState::Unavailable));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Unavailable));
        assert_eq!(pres.command, None);
        assert!(!pres.recovery_representable);

        let after = fs::read(&path).unwrap();
        assert_eq!(after, malformed);
    }

    #[test]
    fn test_macos_cancelled_within_cooldown_is_paused_not_cooling() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        let home = temp.path().join("fake_home_mac_cancelled");
        fs::create_dir_all(&home).unwrap();
        let recent_transition =
            solstone_core_time::utc_rfc3339(now - jiff::SignedDuration::from_mins(5));
        write_install_status(
            temp.path(),
            "failed",
            Some("install_cancelled"),
            None,
            None,
            Some(&recent_transition),
        );
        let pres = read_speech_setup(temp.path(), "darwin", "arm64", Some(&home), now);
        assert_eq!(pres.state, Some(SpeechSetupState::Paused));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Calm));
        assert_eq!(pres.cause, Some("install_cancelled".to_string()));
        assert_eq!(
            pres.command,
            Some("solstone journal install-models --force --variant coreml".to_string())
        );
    }

    #[test]
    fn test_macos_malformed_journal_config_is_unavailable() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();

        let config_path = temp.path().join("config/journal.json");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, b"{").unwrap();

        let home = temp.path().join("fake_home_mac_cfg");
        fs::create_dir_all(&home).unwrap();

        let pres = read_speech_setup(temp.path(), "darwin", "arm64", Some(&home), now);
        assert!(pres.applicable);
        assert!(pres.visible);
        assert_eq!(pres.state, Some(SpeechSetupState::Unavailable));
        assert_eq!(pres.tone, Some(SpeechSetupTone::Unavailable));
        assert_eq!(pres.command, None);
        assert!(!pres.recovery_representable);
    }

    #[test]
    fn unknown_follow_reason_does_not_prescribe_reinstall() {
        let temp = TempDir::new().unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();
        write_install_status(temp.path(), "idle", None, None, None, None);
        write_runtime_health(
            temp.path(),
            "artifact-not-ready",
            Some("future-follow-reason"),
        );
        let pres = read_speech_setup(temp.path(), "linux", "x86_64", None, now);
        assert_eq!(pres.state, Some(SpeechSetupState::Unavailable));
        assert_eq!(pres.command, None);
        assert!(!pres.recovery_representable);
    }

    #[test]
    fn macos_progress_uses_the_shared_active_install_observation() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("mac_home");
        fs::create_dir_all(&home).unwrap();
        let now: Timestamp = "2026-10-17T12:00:00Z".parse().unwrap();
        write_install_status(temp.path(), "downloading", None, None, Some((10, 20)), None);
        let lease = solstone_core_local::install::lease::acquire(temp.path(), "parakeet").unwrap();
        assert!(lease.is_some());
        let active = read_speech_setup(temp.path(), "darwin", "arm64", Some(&home), now);
        assert_eq!(active.state, Some(SpeechSetupState::Downloading));
        assert_eq!(active.bytes_received, Some(10));
        assert_eq!(active.bytes_total, Some(20));
        drop(lease);
        let abandoned = read_speech_setup(temp.path(), "darwin", "arm64", Some(&home), now);
        assert_eq!(abandoned.state, Some(SpeechSetupState::Attention));
        assert_eq!(abandoned.cause.as_deref(), Some("install_interrupted"));
    }
}
