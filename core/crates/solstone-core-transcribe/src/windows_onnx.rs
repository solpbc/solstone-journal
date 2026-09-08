// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed-package routing into the existing bounded Windows helper owner.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use solstone_core_local::install::onnx_readiness::verified_windows_onnx_package;
use solstone_core_system::process::{
    BoundedHelperBudget, BoundedHelperError, BoundedHelperFailure, BoundedHelperOutput,
    BoundedHelperRequest, BoundedHelperResourceLimits, BoundedHelperResources, run_bounded_helper,
};

pub(crate) const ONNX_STDIN_LIMIT: usize = 8 * 1024 * 1024;
pub(crate) const ONNX_STDOUT_LIMIT: usize = 1024 * 1024;
pub(crate) const ONNX_STDERR_LIMIT: usize = 64 * 1024;

pub(crate) enum OnnxHelper {
    Speakers,
    Vad,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OnnxHelperError {
    #[error("{0}")]
    Admission(String),
    #[error(transparent)]
    Validation(#[from] BoundedHelperError),
    #[error(transparent)]
    Launch(#[from] BoundedHelperFailure),
}

pub(crate) fn run_onnx_helper(
    helper: OnnxHelper,
    expected_binary: &Path,
    stdin: &[u8],
    budget: BoundedHelperBudget,
    resources: BoundedHelperResources,
) -> Result<BoundedHelperOutput, OnnxHelperError> {
    if stdin.len() > budget.stdin_limit_bytes {
        return Err(BoundedHelperError::InputLimitExceeded.into());
    }
    let package = verified_windows_onnx_package().map_err(OnnxHelperError::Admission)?;
    let executable = match helper {
        OnnxHelper::Speakers => package.speakers_worker,
        OnnxHelper::Vad => package.vad_worker,
    };
    if !solstone_core_local::install::windows_member_path::matches_declared_member_path(
        expected_binary,
        &executable,
    ) {
        return Err(OnnxHelperError::Admission(
            "ONNX helper is not the declared member of the signed app payload".to_owned(),
        ));
    }
    let system_root =
        std::env::var_os("SystemRoot").ok_or(BoundedHelperError::MissingSystemRoot)?;
    Ok(run_bounded_helper(BoundedHelperRequest {
        resources,
        current_directory: package.package_root.join("bin"),
        package_root: package.package_root,
        executable,
        arguments: Vec::new(),
        environment: BTreeMap::from([(OsString::from("SystemRoot"), system_root)]),
        stdin: stdin.to_vec(),
        budget,
        resource_limits: Some(BoundedHelperResourceLimits {
            cpu_rate_per_10_000: 10_000,
            committed_memory_bytes: 2 * 1024 * 1024 * 1024,
        }),
    })?)
}

/// Retain the caller's already validated generation; never forward it to the
/// bounded helper or reconstruct it from environment metadata.
pub(crate) fn generation_resources(
    context: &solstone_core_system::process::ChildLaunchContext,
) -> BoundedHelperResources {
    let mut resources = BoundedHelperResources::new();
    resources.retain(std::sync::Arc::new(context.read_file_grants.clone()));
    resources
}
