// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::service_status,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let status = service_status::fetch(context);
    from_status(check, status.as_ref().ok())
}

pub(crate) fn from_status(check: Check, status: Option<&serde_json::Value>) -> RunnerResult {
    let Some(status) = status else {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.task.pace.status.unavailable"),
            None::<String>,
        ));
    };
    let slow = status
        .get("tasks")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|task| {
            task.get("slow").and_then(serde_json::Value::as_bool) == Some(true)
                || task.get("stuck").and_then(serde_json::Value::as_bool) == Some(true)
        })
        .map(|task| {
            copy::display(
                "journal.cli.doctor.tasks.duration",
                [
                    (
                        "task",
                        &(task
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("?")),
                    ),
                    (
                        "seconds",
                        &(task
                            .get("duration_seconds")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0)),
                    ),
                    (
                        "limit",
                        &(task
                            .get("max_runtime_seconds")
                            .and_then(serde_json::Value::as_i64)
                            .map_or("?".into(), |v| v.to_string())),
                    ),
                ],
            )
        })
        .collect::<Vec<_>>();
    if slow.is_empty() {
        Ok(make_result(
            check,
            Status::Ok,
            copy::text("journal.cli.doctor.task.pace.healthy"),
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            copy::display(
                "journal.cli.doctor.tasks.long",
                [("tasks", &(slow.join(", ")))],
            ),
            Some(copy::text("journal.cli.doctor.task.pace.about.cap")),
        ))
    }
}
