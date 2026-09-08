// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows CED transport; local retains model/digest and verdict authority.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

pub use solstone_core_system::process::BoundedHelperResources;

use serde_json::{Value, json};
use solstone_core_local::install::ced_readiness::verified_windows_ced_package;
use solstone_core_local::install::ced_runtime::{
    CED_ANALYZE_TIMEOUT, CED_PROBE_COMMAND, CedAnalyzeError, CedAnalyzeProgram,
};
use solstone_core_system::process::{
    BoundedHelperBudget, BoundedHelperError, BoundedHelperRequest, BoundedHelperResourceLimits,
    run_bounded_helper,
};

pub(crate) fn probe(library: &Path, model: &Path) -> Result<(), String> {
    let response = invoke(
        &CedAnalyzeProgram::SiblingHelper,
        &[CED_PROBE_COMMAND],
        &json!({"schema": "solstone-ced-probe-request-v1", "models": {
            "ced_library_path": library, "ced_model_path": model,
        }}),
        CED_ANALYZE_TIMEOUT,
        solstone_core_system::process::BoundedHelperResources::new(),
    )
    .map_err(|error| error.to_string())?;
    if response.get("schema").and_then(Value::as_str) != Some("solstone-ced-probe-response-v1")
        || response.get("ok") != Some(&Value::Bool(true))
    {
        return Err("CED helper returned an invalid probe response".to_owned());
    }
    Ok(())
}

pub fn invoke(
    program: &CedAnalyzeProgram,
    leading_args: &[&str],
    request: &Value,
    timeout: Duration,
    resources: BoundedHelperResources,
) -> Result<Value, CedAnalyzeError> {
    if !matches!(program, CedAnalyzeProgram::SiblingHelper)
        || (!leading_args.is_empty() && leading_args != [CED_PROBE_COMMAND])
    {
        return Err(CedAnalyzeError::Unresolved {
            detail: "Windows CED requires the signed package helper and its declared command"
                .to_owned(),
        });
    }
    let package =
        verified_windows_ced_package().map_err(|detail| CedAnalyzeError::Unresolved { detail })?;
    let requested_library = request
        .get("models")
        .and_then(|models| models.get("ced_library_path"))
        .and_then(Value::as_str);
    if !requested_library.is_some_and(|path| {
        solstone_core_local::install::windows_member_path::matches_declared_member_path(
            Path::new(path),
            &package.library,
        )
    }) {
        return Err(CedAnalyzeError::Unresolved {
            detail: "CED request library does not match the signed package".to_owned(),
        });
    }
    let system_root = std::env::var_os("SystemRoot").ok_or_else(|| CedAnalyzeError::Spawn {
        detail: "SystemRoot is not set".to_owned(),
    })?;
    let mut request = request.clone();
    request["models"]["ced_library_path"] = json!(package.library);
    let stdin = serde_json::to_vec(&request).map_err(|error| CedAnalyzeError::Io {
        detail: error.to_string(),
    })?;
    let output = run_bounded_helper(BoundedHelperRequest {
        resources,
        current_directory: package.root.join("bin"),
        package_root: package.root,
        executable: package.helper,
        arguments: leading_args.iter().map(|arg| (*arg).to_owned()).collect(),
        environment: BTreeMap::from([(OsString::from("SystemRoot"), system_root)]),
        stdin,
        budget: BoundedHelperBudget {
            timeout,
            stdin_limit_bytes: 8 * 1024 * 1024,
            stdout_limit_bytes: 64 * 1024 * 1024,
            stderr_limit_bytes: 64 * 1024,
        },
        resource_limits: Some(BoundedHelperResourceLimits {
            cpu_rate_per_10_000: 10_000,
            committed_memory_bytes: 2 * 1024 * 1024 * 1024,
        }),
    })
    .map_err(|error| match error.cause() {
        BoundedHelperError::DeadlineExceeded {
            quiescent: true, ..
        } if error.cleanup().is_none() => CedAnalyzeError::Timeout,
        _ => {
            log::warn!("CED helper launch failed: {error:?}");
            CedAnalyzeError::Spawn {
                detail: error.to_string(),
            }
        }
    })?;
    if output.exit_code != 0 {
        return Err(CedAnalyzeError::Exit {
            code: Some(output.exit_code),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|error| CedAnalyzeError::MalformedResponse {
        detail: error.to_string(),
    })
}
