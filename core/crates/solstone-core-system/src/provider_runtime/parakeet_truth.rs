// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Parakeet truth-observation shapes that do not depend on GPU or config
//! I/O: platform gating and the admission latch's own outcomes.
//!
//! Mirrors the early-exit shapes in `supervisor.py`'s
//! `_observe_parakeet_provider_truth`, in the same order it checks them:
//! remote mode, platform-can-host, then the admission latch's `blocked`/
//! `desired` verdict. The remaining branch -- resolving a GPU backend and
//! building a launch plan when the latch says Parakeet is desired and not
//! blocked -- is composed behind [`super::seams::TruthObservationSeam`] in
//! [`super::parakeet_truth_seam`], with an injectable (always-empty in
//! production) Vulkan device list standing in for the real device probing
//! that is still not part of this port.

use serde_json::{Value, json};

use super::admission::ParakeetAdmissionLatch;
use super::model::{ProviderName, ProviderTruthObservation, ReasonCode, RuntimePhase};

/// A host may run the installed Windows CPU package on x86_64, or one of the
/// existing pinned Linux artifacts. Package signature verification remains a
/// separate runtime capability check; this stays a pure platform decision.
pub fn parakeet_platform_can_host(platform: &str, machine: &str) -> bool {
    (platform == "windows" && machine == "x86_64")
        || (platform.starts_with("linux")
            && solstone_core_local::install::pins::parakeet_artifact_key("linux", machine).is_ok())
}

fn not_desired(reason_code: &'static str, detail: Value) -> ProviderTruthObservation {
    ProviderTruthObservation {
        provider: ProviderName::Parakeet,
        phase: RuntimePhase::NotDesired,
        reason_code: Some(ReasonCode::known(reason_code)),
        desired_fingerprint: None,
        has_plan: false,
        boot_required: false,
        detail: Some(detail),
    }
}

/// Mirrors `_not_desired_observation("parakeet", "provider-not-needed", detail={"remote_mode": True})`.
pub fn remote_mode_not_desired() -> ProviderTruthObservation {
    not_desired("provider-not-needed", json!({"remote_mode": true}))
}

/// Mirrors `_not_desired_observation("parakeet", "provider-not-needed", detail={"platform": sys.platform})`.
/// `platform` is the raw host platform string (e.g. `"linux"`, `"darwin"`,
/// `"win32"`), not the checked literal `parakeet_platform_can_host` matches
/// against -- Python records `sys.platform` verbatim in this detail.
pub fn platform_cannot_host_not_desired(platform: &str) -> ProviderTruthObservation {
    not_desired("provider-not-needed", json!({"platform": platform}))
}

/// Mirrors the admission latch's `blocked` branch: `phase="host-blocked"`,
/// `reason_code="host-admission-blocked"`, `boot_required=True`.
pub fn admission_blocked_observation(latch: &ParakeetAdmissionLatch) -> ProviderTruthObservation {
    debug_assert!(latch.blocked);
    ProviderTruthObservation {
        provider: ProviderName::Parakeet,
        phase: RuntimePhase::HostBlocked,
        reason_code: Some(ReasonCode::known("host-admission-blocked")),
        desired_fingerprint: None,
        has_plan: false,
        boot_required: true,
        detail: Some(json!({"stt_admission_latch": latch.to_json()})),
    }
}

/// Mirrors the admission latch's `not desired` branch: `_not_desired_observation`
/// with the latch's own `reason_code` (`confidential-backend-selected` or
/// `provider-not-needed`) and the latch itself as detail.
pub fn admission_not_desired_observation(
    latch: &ParakeetAdmissionLatch,
) -> ProviderTruthObservation {
    debug_assert!(!latch.desired && !latch.blocked);
    not_desired(
        latch.reason_code,
        json!({"stt_admission_latch": latch.to_json()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_x86_64_can_host() {
        assert!(parakeet_platform_can_host("linux", "x86_64"));
    }

    #[test]
    fn linux_aarch64_can_host() {
        assert!(parakeet_platform_can_host("linux", "aarch64"));
    }

    #[test]
    fn linux_unknown_machine_cannot_host() {
        assert!(!parakeet_platform_can_host("linux", "riscv64"));
    }

    #[test]
    fn darwin_cannot_host_even_on_a_supported_arch() {
        assert!(!parakeet_platform_can_host("darwin", "x86_64"));
    }

    #[test]
    fn windows_x86_64_can_host_when_the_signed_package_is_present() {
        assert!(parakeet_platform_can_host("windows", "x86_64"));
        assert!(!parakeet_platform_can_host("windows", "aarch64"));
    }

    #[test]
    fn remote_mode_shape_matches_python() {
        let observation = remote_mode_not_desired();
        assert_eq!(observation.provider, ProviderName::Parakeet);
        assert_eq!(observation.phase, RuntimePhase::NotDesired);
        assert_eq!(
            observation.reason_code.as_ref().map(ReasonCode::as_str),
            Some("provider-not-needed")
        );
        assert!(!observation.boot_required);
        assert!(!observation.has_plan);
        assert_eq!(observation.detail, Some(json!({"remote_mode": true})));
    }

    #[test]
    fn platform_cannot_host_shape_carries_the_raw_platform_string() {
        let observation = platform_cannot_host_not_desired("darwin");
        assert_eq!(observation.phase, RuntimePhase::NotDesired);
        assert_eq!(
            observation.reason_code.as_ref().map(ReasonCode::as_str),
            Some("provider-not-needed")
        );
        assert_eq!(observation.detail, Some(json!({"platform": "darwin"})));
    }

    fn desired_latch() -> ParakeetAdmissionLatch {
        ParakeetAdmissionLatch {
            input_json: "{}".to_owned(),
            input_sha256: "deadbeef".to_owned(),
            retry_epoch: 0,
            choice: "parakeet".to_owned(),
            desired: true,
            blocked: false,
            reason_code: "provider-not-needed",
        }
    }

    #[test]
    fn admission_blocked_shape_is_host_blocked_and_boot_required() {
        let latch = ParakeetAdmissionLatch {
            choice: "surface".to_owned(),
            desired: false,
            blocked: true,
            reason_code: "host-admission-blocked",
            ..desired_latch()
        };
        let observation = admission_blocked_observation(&latch);
        assert_eq!(observation.phase, RuntimePhase::HostBlocked);
        assert_eq!(
            observation.reason_code.as_ref().map(ReasonCode::as_str),
            Some("host-admission-blocked")
        );
        assert!(observation.boot_required);
        assert_eq!(
            observation.detail,
            Some(json!({"stt_admission_latch": latch.to_json()}))
        );
    }

    #[test]
    fn admission_not_desired_shape_carries_the_latchs_own_reason_code() {
        let latch = ParakeetAdmissionLatch {
            choice: "confidential".to_owned(),
            desired: false,
            blocked: false,
            reason_code: "confidential-backend-selected",
            ..desired_latch()
        };
        let observation = admission_not_desired_observation(&latch);
        assert_eq!(observation.phase, RuntimePhase::NotDesired);
        assert_eq!(
            observation.reason_code.as_ref().map(ReasonCode::as_str),
            Some("confidential-backend-selected")
        );
        assert!(!observation.boot_required);
        assert_eq!(
            observation.detail,
            Some(json!({"stt_admission_latch": latch.to_json()}))
        );
    }
}
