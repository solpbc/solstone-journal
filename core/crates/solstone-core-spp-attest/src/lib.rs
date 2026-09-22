// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure parsers and verifiers for SPP attestation evidence.

pub mod binding;
pub mod error;
pub mod nvgpu;
pub mod pcr_appraisal;
pub mod pins;
pub mod snp;
pub mod tlv;
pub mod tpm_quote;

pub use error::{
    ApplicationExpectationsError, PcrFingerprintError, PcrPinMismatchError, QuotePcrsError,
};
pub use nvgpu::{
    GpuAppraiser, NVATTEST_TIMEOUT, NvattestCommand, NvattestGpuAppraiser, NvattestInstallation,
    appraise_gpu_leg, build_nvattest_attest_command, locate_nvattest,
};
pub use pcr_appraisal::{
    ApplicationExpectations, PcrAppraisalFailure, PcrAppraisalReason, appraise_application_pcrs,
    parse_quote_pcrs,
};
pub use pins::{PRODUCTION_PCR_SHA256_PINS, production_policy};
pub use snp::{
    CpuBundle, PcrMode, Policy, QuoteVerifier, TcbFloor, appraise_cpu_leg, check_pcr_fingerprint,
};

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;

    pub(crate) fn fixture_bytes(name: &str) -> Vec<u8> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("repository root")
            .join("tests/fixtures/spp_attest");
        std::fs::read(root.join(name)).expect("read SPP attestation fixture")
    }
}
