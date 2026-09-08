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
    #[cfg(feature = "test-fixture-pin")]
    let fixture_pin = {
        // Require the existing environment pin explicitly. A parent-only
        // OnceLock pin cannot be conveyed to another process, and a relative
        // path could resolve differently under the helper's working directory.
        let pin = std::env::var("SOLSTONE_JOURNAL_MINISIGN_PIN").map_err(|_| {
            OnnxHelperError::Admission(
                "test-signed ONNX launch requires an explicit UTF-8 fixture pin path".to_owned(),
            )
        })?;
        if pin.is_empty() || !Path::new(&pin).is_absolute() {
            return Err(OnnxHelperError::Admission(
                "test-signed ONNX launch requires an absolute fixture pin path".to_owned(),
            ));
        }
        pin
    };
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
    let environment = BTreeMap::from([(OsString::from("SystemRoot"), system_root)]);
    #[cfg(feature = "test-fixture-pin")]
    let environment = {
        let mut environment = environment;
        // Isolated fixture builds use the existing signed-manifest test pin in
        // each verifying process. The caller's OnceLock cannot cross a launch.
        environment.insert(
            OsString::from("SOLSTONE_JOURNAL_MINISIGN_PIN"),
            OsString::from(fixture_pin),
        );
        environment
    };
    Ok(run_bounded_helper(BoundedHelperRequest {
        resources,
        current_directory: package.package_root.join("bin"),
        package_root: package.package_root,
        executable,
        arguments: Vec::new(),
        environment,
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

#[cfg(all(test, feature = "test-fixture-pin"))]
#[path = "windows_onnx_native_tests.rs"]
mod native_tests;
