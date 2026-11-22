// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows CED transport; local retains model/digest and verdict authority.

use solstone_core_cli_copy::MessageDetail;
use solstone_core_local::install::capability_status::CapabilityDetail;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

pub use solstone_core_system::process::BoundedHelperResources;

use serde_json::{Value, json};
use solstone_core_local::install::ced_readiness::verified_windows_ced_package_detail;
use solstone_core_local::install::ced_runtime::{
    CED_ANALYZE_TIMEOUT, CED_PROBE_COMMAND, CedAnalyzeError, CedAnalyzeProgram,
};
use solstone_core_system::process::{
    BoundedHelperBudget, BoundedHelperError, BoundedHelperRequest, BoundedHelperResourceLimits,
    run_bounded_helper,
};

pub(crate) fn probe(locale: &str, library: &Path, model: &Path) -> Result<(), CapabilityDetail> {
    let response = invoke_in(
        locale,
        &CedAnalyzeProgram::SiblingHelper,
        &[CED_PROBE_COMMAND],
        &json!({"schema": "solstone-ced-probe-request-v1", "models": {
            "ced_library_path": library, "ced_model_path": model,
        }}),
        CED_ANALYZE_TIMEOUT,
        solstone_core_system::process::BoundedHelperResources::new(),
    )
    .map_err(|error| CapabilityDetail::from(error.detail()))?;
    if response.get("schema").and_then(Value::as_str) != Some("solstone-ced-probe-response-v1")
        || response.get("ok") != Some(&Value::Bool(true))
    {
        return Err(CapabilityDetail::message(
            "journal.cli.transcribe.sound.windows.probe",
            [],
        ));
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
    invoke_in(
        solstone_core_cli_copy::ENGLISH,
        program,
        leading_args,
        request,
        timeout,
        resources,
    )
}

pub fn invoke_in(
    locale: &str,
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
            detail: MessageDetail::message("journal.cli.transcribe.sound.windows.signed", []),
        });
    }
    let package = verified_windows_ced_package_detail()
        .map_err(|detail| CedAnalyzeError::Unresolved { detail })?;
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
            detail: MessageDetail::message("journal.cli.transcribe.sound.windows.library", []),
        });
    }
    let system_root = std::env::var_os("SystemRoot").ok_or_else(|| CedAnalyzeError::Spawn {
        detail: MessageDetail::message("journal.cli.transcribe.sound.windows.root", []),
    })?;
    let mut request = request.clone();
    request["models"]["ced_library_path"] = json!(package.library);
    let stdin = serde_json::to_vec(&request).map_err(|error| CedAnalyzeError::Io {
        detail: error.to_string().into(),
    })?;
    // The helper re-verifies this same signed payload for itself before it
    // will load the engine, so it has to resolve the manifest pin the way we
    // just did. Production adds nothing here; a fixture-pin build adds the one
    // entry that stops a test-signed package from being admitted by the parent
    // and refused by its own child.
    let mut environment = BTreeMap::from([(OsString::from("SystemRoot"), system_root)]);
    environment
        .extend(solstone_core_distribution::manifest_verify::signed_package_pin_environment());
    let output = run_bounded_helper(BoundedHelperRequest {
        resources,
        current_directory: package.root.join("bin"),
        package_root: package.root,
        executable: package.helper,
        arguments: leading_args.iter().map(|arg| (*arg).to_owned()).collect(),
        environment,
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
        stdout_sink: None,
        cancellation: None,
    })
    .map_err(|error| match error.cause() {
        BoundedHelperError::DeadlineExceeded {
            quiescent: true, ..
        } if error.cleanup().is_none() => CedAnalyzeError::Timeout,
        _ => {
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.transcribe.sound.windows.launch",
                    [("error", &format!("{error:?}"))]
                )
            );
            CedAnalyzeError::Spawn {
                detail: error.to_string().into(),
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
        detail: error.to_string().into(),
    })
}
