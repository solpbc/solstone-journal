// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::{common, service_status},
    context::CheckContext,
    vocabulary::{Check, CheckResult, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
const INSTALL: &str = "journal.cli.doctor.parakeet.cpp.stt.ready.install";
// ⛔ This named the service and then gave the FOREGROUND command, in one
// sentence. `journal up` is the documented alias for `journal service start`.
const START: &str = "journal.cli.doctor.parakeet.cpp.stt.ready.start";
// ⛔ A Windows install carries no per-journal parakeet cache to fetch into:
// the server and model are members of the signed package, so a failed
// verification means the package itself changed.
const WINDOWS_REINSTALL: &str = "journal.cli.doctor.parakeet.reinstall";

/// Windows answers the same question from the signed package: the server and
/// model the resident launches are declared members of the verified payload,
/// and the server is the same authenticated one the resident runs.
fn windows_ready(context: &CheckContext, check: Check) -> RunnerResult {
    if let Err(error) =
        solstone_core_local::install::parakeet_readiness::verified_windows_parakeet_package_in(
            context.report_form.locale(),
        )
    {
        return Ok(make_result(
            check,
            Status::Warn,
            error,
            Some(copy::text(WINDOWS_REINSTALL)),
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
                copy::text("journal.cli.doctor.parakeet.cpp.stt.ready.offline"),
                Some(copy::text(START)),
            ));
        }
        Err(cause) => {
            return Ok(make_result(
                check,
                Status::Warn,
                copy::display(
                    "journal.cli.doctor.parakeet.unreachable",
                    [("cause", &cause.key())],
                ),
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
            copy::text("journal.cli.doctor.parakeet.cpp.stt.ready.online"),
            None::<String>,
        ),
        // The state is shown as a labelled code, not read into a sentence;
        // `observing` is not an owner word, so it reads as "checking".
        Some(phase) => make_result(
            check,
            Status::Warn,
            if phase == "observing" {
                copy::text("journal.cli.doctor.parakeet.phase.checking")
            } else {
                copy::display("journal.cli.doctor.parakeet.phase", [("phase", &phase)])
            },
            None::<String>,
        ),
        None => make_result(
            check,
            Status::Warn,
            copy::text("journal.cli.doctor.parakeet.cpp.stt.ready.server.missing"),
            Some(copy::text(START)),
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
            copy::display(
                "journal.cli.doctor.parakeet.cpp.stt.ready.unreachable",
                [("error", &error)],
            ),
            Some(copy::text(START)),
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
            copy::text("journal.cli.doctor.parakeet.cpp.stt.ready.unsupported"),
            None::<String>,
        ));
    }
    let artifacts = match solstone_core_system::provider_runtime::parakeet_cpp_artifacts_in(
        &context.journal_path,
        "linux",
        &context.host_arch,
        context.report_form.locale(),
    ) {
        Ok(value) => value,
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Warn,
                error,
                Some(copy::text(INSTALL)),
            ));
        }
    };
    if let Err(error) = solstone_core_system::provider_runtime::check_parakeet_cpp_files_in(
        &artifacts,
        context.report_form.locale(),
    ) {
        return Ok(make_result(
            check,
            Status::Warn,
            error,
            Some(copy::text(INSTALL)),
        ));
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
                copy::text("journal.cli.doctor.parakeet.cpp.stt.ready.runtime.unavailable"),
                Some(
                    copy::text("journal.cli.doctor.parakeet.cpp.stt.ready.runtime.install"),
                ),
            ));
        }
        solstone_core_system::provider_runtime::ParakeetCppReadiness::BinaryUnstartable { .. } => {
            return Ok(make_result(
                check,
                Status::Warn,
                copy::text("journal.cli.doctor.parakeet.cpp.stt.ready.binary.unavailable"),
                Some(copy::text(INSTALL)),
            ));
        }
    };
    server_ready(
        context,
        check,
        &copy::text("journal.cli.doctor.parakeet.cpp.stt.ready.ready"),
    )
}
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    match common::config_backend(context) {
        Err(error) => Ok(make_result(
            check,
            Status::Fail,
            error,
            Some(copy::text("journal.cli.doctor.config.restore")),
        )),
        Ok(Some(backend)) if backend == "parakeet-cpp" => ready(context, check),
        Ok(_) => Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.parakeet.cpp.stt.ready.backend.not.selected"),
            None::<String>,
        )),
    }
}
