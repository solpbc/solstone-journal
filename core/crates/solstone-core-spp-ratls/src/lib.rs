// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Synchronous RA-TLS channel, cadence, and process-local state for SPP.

pub mod cadence;
pub mod error;
pub mod fresh;
pub mod installed;
pub mod nvattest;
mod nvattest_authority;
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
#[cfg(windows)]
pub use fresh::resolve_nvattest_dir;
pub use fresh::{FreshAttestedChannel, perform_fresh_reattest, resolve_ratls_target};
#[doc(hidden)]
pub use fresh::{
    FreshAttestedChannelWith, establish_fresh_production_channel, perform_fresh_reattest_with,
};
pub use installed::{
    NvattestPaths, NvattestRefusal, refusal_for_installed_payload, resolve_installed_nvattest,
    resolve_nvattest_from_current_exe,
};
pub use nvattest::{
    NvattestEnsureStatus, classify_channel_failure, classify_nvattest_prerequisite,
};
pub use nvattest_authority::confidential_verifier_on_this_platform;
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use ratls::channel::establish_local_test_channel_with_clock;
#[cfg(not(windows))]
pub use ratls::production_verifier::{
    establish_production_attested_channel_in_package,
    establish_production_attested_channel_in_package_with_clock,
};
pub use ratls::{
    channel::{
        AdmissionClock, AttestedChannel, AttestedHttpError, AttestedHttpResponse, AttestedIo,
        ChannelAdmission, ChannelStatus, OFFLINE_STATUS_ADMISSION_MARGIN,
        OFFLINE_STATUS_MIN_REMAINING, OFFLINE_STATUS_REQUEST_WINDOW, RatlsEndpoint,
        SystemAdmissionClock, Trailing, append_bearer_header, establish_attested_channel,
        establish_attested_channel_with_clock, send_admission_probe, send_json_request,
    },
    production_verifier::{
        PayloadSource, ProductionCompositeVerifier, check_nvattest_readiness,
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
