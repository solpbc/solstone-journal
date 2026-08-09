// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! nvattest claim parsing and GPU appraisal.

pub mod appraise;
pub mod binary;
pub mod claims;

pub use appraise::{GpuAppraiser, NVATTEST_TIMEOUT, NvattestGpuAppraiser, appraise_gpu_leg};
pub use binary::{
    NvattestCommand, NvattestInstallation, build_nvattest_attest_command, locate_nvattest,
};
pub use claims::{
    GpuAppraisal, NvattestAcceptance, NvattestRejection, NvattestVerdict, build_gpu_appraisal,
    classify_nvattest_result, parse_nvattest_stdout,
};
