// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::service_status,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let status = service_status::fetch(context);
    from_status(check, status.as_ref())
}

pub(crate) fn from_status(check: Check, status: Option<&serde_json::Value>) -> RunnerResult {
    let Some(status) = status else {
        return Ok(make_result(
            check,
            Status::Skip,
            "supervisor status unavailable",
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
            format!(
                "{} ({}s of {}s cap)",
                task.get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?"),
                task.get("duration_seconds")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
                task.get("max_runtime_seconds")
                    .and_then(serde_json::Value::as_i64)
                    .map_or("?".into(), |value| value.to_string())
            )
        })
        .collect::<Vec<_>>();
    if slow.is_empty() {
        Ok(make_result(
            check,
            Status::Ok,
            "tasks on pace",
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            format!("running long: {}", slow.join(", ")),
            Some(
                "a job is running long; it will be stopped automatically if it passes its cap — no action needed unless it persists",
            ),
        ))
    }
}
