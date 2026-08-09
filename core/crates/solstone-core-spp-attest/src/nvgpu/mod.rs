// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure nvattest claim parsing and appraisal.

pub mod claims;

pub use claims::{
    GpuAppraisal, NvattestAcceptance, NvattestRejection, NvattestVerdict, build_gpu_appraisal,
    classify_nvattest_result, parse_nvattest_stdout,
};
