// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
const INSTALL: &str =
    "parakeet-cpp artifacts are not installed — fetch them with: journal install-provider parakeet";
const START: &str = "parakeet-server is not reachable — start the journal service: journal start";
pub fn ready(context: &CheckContext, check: Check) -> RunnerResult {
    if context.platform != crate::vocabulary::Platform::Linux {
        return Ok(make_result(
            check,
            Status::Skip,
            "parakeet-cpp is only supported on Linux",
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
    match solstone_core_system::provider_runtime::probe_parakeet_cpp_binary(&artifacts.binary_cpu, solstone_core_system::provider_runtime::PARAKEET_CPP_PROBE_TIMEOUT) { solstone_core_system::provider_runtime::ParakeetCppReadiness::Ready => {}, solstone_core_system::provider_runtime::ParakeetCppReadiness::OpenMpRuntimeUnavailable { .. } => return Ok(make_result(check, Status::Warn, "parakeet-cpp cannot start: OpenMP runtime unavailable (libgomp.so.1)", Some("install the system OpenMP runtime that provides libgomp.so.1, then rerun journal doctor"))), solstone_core_system::provider_runtime::ParakeetCppReadiness::BinaryUnstartable { .. } => return Ok(make_result(check, Status::Warn, "parakeet-cpp binary cannot start", Some(INSTALL))), _ => return Ok(make_result(check, Status::Warn, "parakeet-cpp binary cannot start", Some(INSTALL))) };
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
    Ok(make_result(
        check,
        Status::Ok,
        "parakeet-cpp ready (binaries + model installed, server reachable)",
        None::<String>,
    ))
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
