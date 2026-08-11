// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            "no local journal",
            None::<String>,
        ));
    }
    match solstone_core_system::lifecycle::check_sync(
        &context.journal_path,
        "doctor.check",
        context.machine_id.as_deref().unwrap_or(""),
        None,
        context.now.timestamp() as f64,
    ) {
        Ok(result) if result.is_boot_conflict() => Ok(make_result(
            check,
            Status::Fail,
            solstone_core_system::lifecycle::format_conflict_message(&result),
            None::<String>,
        )),
        Ok(result) => {
            let prefix = context
                .machine_id
                .as_deref()
                .map(|value| value.chars().take(8).collect::<String>())
                .unwrap_or_else(|| "(unknown)".into());
            let clean = format!(
                "this device only ({}, machine {}...)",
                context.hostname, prefix
            );
            let detail = result.foreign_writers.last().map_or_else(
                || clean.to_owned(),
                |writer| {
                    format!(
                        "{clean}\n  last foreign writer: {} (machine {}...)",
                        writer.hostname,
                        writer.machine_id.chars().take(8).collect::<String>()
                    )
                },
            );
            Ok(make_result(check, Status::Ok, detail, None::<String>))
        }
        Err(error) => Ok(make_result(
            check,
            Status::Fail,
            format!("sync check failed: {error}"),
            None::<String>,
        )),
    }
}
