// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Synchronous RA-TLS channel, cadence, and process-local state for SPP.

pub mod cadence;
pub mod error;
pub mod fresh;
pub mod nvattest;
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
pub use fresh::{FreshAttestedChannel, perform_fresh_reattest};
pub use nvattest::{
    NvattestEnsureStatus, classify_channel_failure, classify_nvattest_prerequisite,
};
pub use ratls::{
    channel::{
        AttestedChannel, AttestedHttpError, AttestedHttpResponse, AttestedIo, RatlsEndpoint,
        establish_attested_channel, send_json_request,
    },
    production_verifier::{
        ProductionCompositeVerifier, check_nvattest_readiness,
        establish_production_attested_channel, verify_composite_with_gpu_appraiser,
    },
    verify::{
        CompositeVerificationInput, CompositeVerifier, VerifiedCertificateEvidence,
        verify_certificate_evidence, verify_exporter_proof,
    },
};
pub use state::{
    AttestationFailure, AttestationFailureKind, AttestationState, AttestationStateStore,
};
