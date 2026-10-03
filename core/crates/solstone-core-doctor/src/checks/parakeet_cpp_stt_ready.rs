// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::{common, service_status},
    context::CheckContext,
    vocabulary::{Check, CheckResult, RunnerResult, Status, make_result},
};
const INSTALL: &str = "parakeet-cpp artifacts are not installed — fetch them with: solstone journal install-provider parakeet";
// ⛔ This named the service and then gave the FOREGROUND command, in one
// sentence. `journal up` is the documented alias for `journal service start`.
const START: &str =
    "parakeet-server is not reachable — start the journal service: solstone journal up";
// ⛔ A Windows install carries no per-journal parakeet cache to fetch into:
// the server and model are members of the signed package, so a failed
// verification means the package itself changed.
const WINDOWS_REINSTALL: &str = "reinstall the journal";

/// Windows answers the same question from the signed package: the server and
/// model the resident launches are declared members of the verified payload,
/// and the server is the same authenticated one the resident runs.
fn windows_ready(context: &CheckContext, check: Check) -> RunnerResult {
    if let Err(error) =
        solstone_core_local::install::parakeet_readiness::verified_windows_parakeet_package()
    {
        return Ok(make_result(
            check,
            Status::Warn,
            error,
            Some(WINDOWS_REINSTALL),
        ));
    }
    resident_server_ready(context, check)
}

#[cfg(test)]
pub(crate) fn windows_resident_for_test(context: &CheckContext, check: Check) -> CheckResult {
    resident_server_ready(context, check).expect("the resident answer never errors")
}

/// ⛔ On Windows the parakeet server answers only requests carrying a
/// capability the resident hands its own children, so a probe from outside
/// can never get a 200 and would call a healthy install unreachable. The
/// resident holds the private probe; its status frame reports the provider's
/// phase, and that is the Windows answer to "is the server reachable?".
fn resident_server_ready(context: &CheckContext, check: Check) -> RunnerResult {
    let status = match service_status::fetch(context) {
        Ok(status) => status,
        Err(service_status::Unavailable::NoSocket) => {
            return Ok(make_result(
                check,
                Status::Warn,
                "parakeet-server not reachable: your journal isn't running",
                Some(START),
            ));
        }
        Err(cause) => {
            return Ok(make_result(
                check,
                Status::Warn,
                format!("parakeet-server not reachable: {}", cause.as_str()),
                None::<String>,
            ));
        }
    };
    Ok(parakeet_phase_result(check, &status))
}

pub(crate) fn parakeet_phase_result(check: Check, status: &serde_json::Value) -> CheckResult {
    let phase = status
        .get("services")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|service| {
            service.get("name").and_then(serde_json::Value::as_str)
                == Some(solstone_core_system::provider_runtime::ProviderName::Parakeet.as_str())
        })
        .and_then(|service| service.get("phase").and_then(serde_json::Value::as_str));
    match phase {
        Some("ready") => make_result(
            check,
            Status::Ok,
            "parakeet-cpp ready (server + model match the signed package, server reachable)",
            None::<String>,
        ),
        // The state is shown as a labelled code, not read into a sentence;
        // `observing` is not an owner word, so it reads as "checking".
        Some(phase) => make_result(
            check,
            Status::Warn,
            format!(
                "parakeet-server not reachable: your journal reports its state as {}",
                if phase == "observing" {
                    "checking"
                } else {
                    phase
                }
            ),
            None::<String>,
        ),
        None => make_result(
            check,
            Status::Warn,
            "parakeet-server not reachable: your journal is running, but not parakeet-server",
            Some(START),
        ),
    }
}

fn server_ready(context: &CheckContext, check: Check, ready: &str) -> RunnerResult {
    let probe = context
        .parakeet_server_probe_override
        .unwrap_or(solstone_core_system::provider_runtime::probe_parakeet_cpp_server);
    if let Err(error) = probe(
        &context.journal_path,
        solstone_core_system::provider_runtime::PARAKEET_CPP_PROBE_TIMEOUT,
    ) {
        return Ok(make_result(
            check,
            Status::Warn,
            format!("parakeet-server not reachable: {error}"),
            Some(START),
        ));
    }
    Ok(make_result(check, Status::Ok, ready, None::<String>))
}

pub fn ready(context: &CheckContext, check: Check) -> RunnerResult {
    if context.platform == crate::vocabulary::Platform::Windows {
        return windows_ready(context, check);
    }
    if context.platform != crate::vocabulary::Platform::Linux {
        return Ok(make_result(
            check,
            Status::Skip,
            "parakeet-cpp is only supported on linux and windows",
            None::<String>,
        ));
    }
    let artifacts = match solstone_core_system::provider_runtime::parakeet_cpp_artifacts(
        &context.journal_path,
        "linux",
        &context.host_arch,
    ) {
        Ok(value) => value,
        Err(error) => return Ok(make_result(check, Status::Warn, error, Some(INSTALL))),
    };
    if let Err(error) = solstone_core_system::provider_runtime::check_parakeet_cpp_files(&artifacts)
    {
        return Ok(make_result(check, Status::Warn, error, Some(INSTALL)));
    }
    match solstone_core_system::provider_runtime::probe_parakeet_cpp_binary(
        &artifacts.binary_cpu,
        solstone_core_system::provider_runtime::PARAKEET_CPP_PROBE_TIMEOUT,
    ) {
        solstone_core_system::provider_runtime::ParakeetCppReadiness::Ready => {}
        solstone_core_system::provider_runtime::ParakeetCppReadiness::OpenMpRuntimeUnavailable {
            ..
        } => {
            return Ok(make_result(
                check,
                Status::Warn,
                "parakeet-cpp cannot start: OpenMP runtime unavailable (libgomp.so.1)",
                Some(
                    "install the system OpenMP runtime that provides libgomp.so.1, then rerun solstone journal doctor",
                ),
            ));
        }
        solstone_core_system::provider_runtime::ParakeetCppReadiness::BinaryUnstartable { .. } => {
            return Ok(make_result(
                check,
                Status::Warn,
                "parakeet-cpp binary cannot start",
                Some(INSTALL),
            ));
        }
    };
    server_ready(
        context,
        check,
        "parakeet-cpp ready (binaries + model installed, server reachable)",
    )
}
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    match common::config_backend(context) {
        Err(error) => Ok(make_result(
            check,
            Status::Fail,
            error,
            Some("repair or restore config/journal.json from a backup"),
        )),
        Ok(Some(backend)) if backend == "parakeet-cpp" => ready(context, check),
        Ok(_) => Ok(make_result(
            check,
            Status::Skip,
            "configured backend is not parakeet-cpp; check not applicable",
            None::<String>,
        )),
    }
}
