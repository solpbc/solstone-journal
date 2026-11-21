// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_transcribe::{
    VAD_RUNTIME_PROBE_TIMEOUT, VadRuntimeStatus, probe_from_executable_in, probe_vad_runtime_in,
    repair_in, status_detail_in,
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let status = match context.vad_runtime_probe {
        Some(resolver) => {
            let seam = resolver();
            probe_vad_runtime_in(
                seam.binary.as_ref(),
                seam.timeout,
                context.report_form.locale(),
            )
        }
        None => probe_from_executable_in(
            Ok(context.install_bin_dir.join("solstone-core")),
            VAD_RUNTIME_PROBE_TIMEOUT,
            context.report_form.locale(),
        ),
    };
    let detail = status_detail_in(&status, context.report_form.locale());
    let status_row = if matches!(status, VadRuntimeStatus::Ready) {
        Status::Ok
    } else {
        Status::Fail
    };
    Ok(make_result(
        check,
        status_row,
        detail,
        repair_in(&status, context.report_form.locale()),
    ))
}
