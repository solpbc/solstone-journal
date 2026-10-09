// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::state::{AttestationFailure, AttestationFailureKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvattestEnsureStatus {
    AlreadyInstalled,
    Installed,
    PlatformUnsupported,
    Unavailable,
    IntegrityFailed,
}

pub fn classify_nvattest_prerequisite(status: NvattestEnsureStatus) -> Option<AttestationFailure> {
    match status {
        NvattestEnsureStatus::AlreadyInstalled | NvattestEnsureStatus::Installed => None,
        NvattestEnsureStatus::PlatformUnsupported => Some(AttestationFailure {
            kind: AttestationFailureKind::Failed,
            reason_code: "nvattest_platform_unsupported",
        }),
        NvattestEnsureStatus::Unavailable => Some(AttestationFailure {
            kind: AttestationFailureKind::Unreachable,
            reason_code: "nvattest_unavailable",
        }),
        NvattestEnsureStatus::IntegrityFailed => Some(AttestationFailure {
            kind: AttestationFailureKind::Failed,
            reason_code: "nvattest_integrity_failed",
        }),
    }
}

pub fn classify_channel_failure(reason_code: &str) -> AttestationFailureKind {
    if reason_code == "gateway_unreachable" || reason_code == "online_check_unreachable" {
        AttestationFailureKind::Unreachable
    } else {
        AttestationFailureKind::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_unreachable_gateway_or_online_check_is_unreachable() {
        assert_eq!(
            classify_channel_failure("gateway_unreachable"),
            AttestationFailureKind::Unreachable
        );
        assert_eq!(
            classify_channel_failure("online_check_unreachable"),
            AttestationFailureKind::Unreachable
        );
        assert_eq!(
            classify_channel_failure("certificate_evidence_invalid"),
            AttestationFailureKind::Failed
        );
    }

    #[test]
    fn composite_failures_are_all_failed_not_unreachable() {
        for reason in [
            "pcr_pin_mismatch",
            "id_key_pin_mismatch",
            "cpu_verification_failed",
            "nvattest_unavailable",
            "nvattest_integrity_failed",
            "gpu_nonce_mismatch",
            "gpu_appraisal_failed",
        ] {
            assert_eq!(
                classify_channel_failure(reason),
                AttestationFailureKind::Failed,
                "{reason} must fail closed"
            );
        }
    }
}
