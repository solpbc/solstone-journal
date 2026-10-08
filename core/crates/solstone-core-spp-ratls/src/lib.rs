// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Synchronous RA-TLS channel, cadence, and process-local state for SPP.

pub mod cadence;
pub mod error;
pub mod fresh;
pub mod nvattest;
mod nvattest_authority;
mod nvattest_install;
pub mod qualification;
pub mod ratls;
pub mod state;

#[cfg(test)]
mod test_support;

pub use cadence::{
    AttestationSession, CompositeVerdict, GPU_REATTEST_INTERVAL, SESSION_CAP,
    TPM_HEARTBEAT_INTERVAL,
};
pub use error::{
    CompositeVerificationError, RatlsChannelError, RatlsContractError, RatlsVerificationError,
};
pub use fresh::{
    FreshAttestedChannel, perform_fresh_reattest, resolve_nvattest_dir, resolve_ratls_target,
};
#[doc(hidden)]
pub use fresh::{
    FreshAttestedChannelWith, establish_fresh_production_channel, perform_fresh_reattest_with,
};
pub use nvattest::{
    NvattestEnsureStatus, classify_channel_failure, classify_nvattest_prerequisite,
};
pub use nvattest_authority::confidential_verifier_on_this_platform;
pub use nvattest_install::ensure_nvattest_installed;
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use nvattest_install::ensure_nvattest_installed_on_for_tests as ensure_nvattest_installed_on;
#[cfg(all(unix, feature = "test-hooks"))]
#[doc(hidden)]
pub use nvattest_install::ensure_nvattest_installed_with_for_tests as ensure_nvattest_installed_with;
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use ratls::channel::establish_local_test_channel_with_clock;
pub use ratls::{
    channel::{
        AdmissionClock, AttestedChannel, AttestedHttpError, AttestedHttpResponse, AttestedIo,
        ChannelAdmission, ChannelStatus, OFFLINE_STATUS_ADMISSION_MARGIN,
        OFFLINE_STATUS_MIN_REMAINING, OFFLINE_STATUS_REQUEST_WINDOW, RatlsEndpoint,
        SystemAdmissionClock, Trailing, append_bearer_header, establish_attested_channel,
        establish_attested_channel_with_clock, send_admission_probe, send_json_request,
    },
    production_verifier::{
        ProductionCompositeVerifier, check_nvattest_readiness,
        establish_production_attested_channel, establish_production_attested_channel_with_clock,
        verify_composite_with_gpu_appraiser, verify_gpu_after_cpu,
    },
    verify::{
        CompositeVerificationInput, CompositeVerifier, VerifiedCertificateEvidence,
        verify_certificate_evidence, verify_exporter_proof,
    },
};
pub use state::{
    AttestationFailure, AttestationFailureKind, AttestationState, AttestationStateStore,
};
