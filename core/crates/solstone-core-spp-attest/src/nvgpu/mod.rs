// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! nvattest claim parsing and GPU appraisal.

pub mod appraise;
pub mod binary;
pub mod claims;
mod rims;
mod staging;
#[cfg(any(windows, test))]
mod windows;

pub use appraise::{
    GpuAppraiser, GpuStatusInput, NVATTEST_TIMEOUT, NvattestGpuAppraiser,
    ProductionAppraisalStaging, appraise_gpu_leg, write_production_appraisal_staging,
};
pub use binary::{
    NvattestCommand, NvattestInstallation, android_verifier_child_environment,
    authorize_android_nvattest_installation, authorize_nvattest_spawn,
    build_nvattest_attest_command, build_nvattest_attest_command_with_installation,
    build_nvattest_offline_attest_command, build_nvattest_offline_attest_command_with_installation,
    locate_nvattest,
};
pub use claims::{
    GpuAppraisal, GpuStatusAuthorization, NvattestAcceptance, NvattestRejection, NvattestVerdict,
    StatusExpectation, build_gpu_appraisal, classify_nvattest_result, parse_nvattest_stdout,
};
pub use rims::{
    GpuProfile, GpuProfiles, ManifestSet, PackagedRim, StatusMode, is_production_manifest_set,
    production_rims,
};

/// Trusts a fixture signing key for a privately signed Windows payload, so a
/// test harness can run the real helper through the production locator.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use solstone_core_distribution::manifest_verify::install_test_fixture_pin;
