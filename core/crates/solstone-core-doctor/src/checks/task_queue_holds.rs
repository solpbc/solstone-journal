// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_system::process::SystemProcessInstanceSource;
use solstone_core_system::{HoldPlatform, classify_task_queue_holds, current_boot_identity};

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let in_flight_dir = context.journal_path.join("health/task-queue/in-flight");
    if !in_flight_dir.exists() {
        return Ok(make_result(
            check,
            Status::Ok,
            "no in-flight or held task partition records present",
            None::<String>,
        ));
    }

    let source = SystemProcessInstanceSource;
    let boot_id = current_boot_identity();
    let platform = match context.platform {
        crate::vocabulary::Platform::Linux => HoldPlatform::Linux,
        crate::vocabulary::Platform::Darwin => HoldPlatform::Macos,
        crate::vocabulary::Platform::Windows => HoldPlatform::Windows,
    };

    let findings =
        classify_task_queue_holds(&context.journal_path, &source, boot_id.as_deref(), platform);

    if findings.is_empty() {
        return Ok(make_result(
            check,
            Status::Ok,
            "all task partition records are clean",
            None::<String>,
        ));
    }

    let has_fail = findings.iter().any(|f| f.is_fail);
    let status = if has_fail { Status::Fail } else { Status::Warn };

    let details = findings
        .iter()
        .map(|f| {
            if let Some(action) = &f.action {
                format!("{}: {} ({action})", f.path.display(), f.detail)
            } else {
                format!("{}: {}", f.path.display(), f.detail)
            }
        })
        .collect::<Vec<_>>()
        .join("; ");

    Ok(make_result(check, status, details, None::<String>))
}
