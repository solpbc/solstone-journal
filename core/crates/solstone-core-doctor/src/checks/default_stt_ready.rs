// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::{common, parakeet_cpp_stt_ready},
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let backend = match common::config_backend(context) {
        Ok(value) => value,
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Fail,
                error,
                Some("repair or restore config/journal.json from a backup"),
            ));
        }
    };
    if backend
        .as_deref()
        .is_some_and(|backend| backend != "parakeet")
    {
        return Ok(make_result(
            check,
            Status::Skip,
            format!(
                "configured backend is {}; parakeet readiness not applicable",
                backend.unwrap()
            ),
            None::<String>,
        ));
    }
    if context.platform == crate::vocabulary::Platform::Linux && context.host_arch == "x86_64" {
        return parakeet_cpp_stt_ready::ready(context, check);
    }
    if context.platform == crate::vocabulary::Platform::Darwin && context.host_arch == "arm64" {
        return match solstone_core_system::provider_runtime::check_parakeet_coreml_cache(
            &context.home_dir,
            "darwin",
            "arm64",
        ) {
            Ok(path) => Ok(make_result(
                check,
                Status::Ok,
                format!("parakeet model ready at {}", path.display()),
                None::<String>,
            )),
            Err(error) => Ok(make_result(
                check,
                Status::Warn,
                error,
                Some(
                    "CoreML parakeet model is not downloaded — fetch it with: journal install-models",
                ),
            )),
        };
    }
    Ok(make_result(
        check,
        Status::Skip,
        "parakeet not supported on this platform",
        None::<String>,
    ))
}
