// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_transcribe::{
    VAD_RUNTIME_PROBE_TIMEOUT, VadRuntimeStatus, check_vad_runtime_with, probe_from_executable,
    status_detail, vad_runtime_repair_for,
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let status = match context.vad_runtime_probe {
        Some(resolver) => {
            let seam = resolver();
            check_vad_runtime_with(seam.binary, seam.timeout)
        }
        None => probe_from_executable(
            Ok(context.install_bin_dir.join("solstone-core")),
            VAD_RUNTIME_PROBE_TIMEOUT,
        ),
    };
    let detail = status_detail(&status);
    let status_row = if matches!(status, VadRuntimeStatus::Ready) {
        Status::Ok
    } else {
        Status::Fail
    };
    Ok(make_result(
        check,
        status_row,
        detail,
        vad_runtime_repair_for(&status),
    ))
}
