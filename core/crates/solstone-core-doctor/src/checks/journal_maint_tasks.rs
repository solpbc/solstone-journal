// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
const FIX: &str = "inspect with journal maint <task>; re-run with journal maint --force <task>";
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            "no local journal",
            None::<String>,
        ));
    }
    let tasks = solstone_core_system_health::read_maint_task_states(&context.journal_path);
    let failed = tasks
        .iter()
        .filter(|task| task.status == solstone_core_system_health::MaintTaskStatus::Failed)
        .map(|task| {
            format!(
                "{}.{} (exit {})",
                task.app,
                task.task,
                task.exit_code.unwrap_or(-1)
            )
        })
        .collect::<Vec<_>>();
    if !failed.is_empty() {
        return Ok(make_result(
            check,
            Status::Fail,
            format!("failed maint task(s): {}", failed.join(", ")),
            Some(FIX),
        ));
    }
    let stale = tasks
        .iter()
        .filter(|task| {
            task.status == solstone_core_system_health::MaintTaskStatus::InProgress
                && task
                    .ran_ts
                    .is_some_and(|ts| context.now.timestamp_millis() - ts > 300_000)
        })
        .map(|task| format!("{}.{}", task.app, task.task))
        .collect::<Vec<_>>();
    if !stale.is_empty() {
        return Ok(make_result(
            check,
            Status::Warn,
            format!("started, no exit: {}", stale.join(", ")),
            Some(FIX),
        ));
    }
    let unreadable = tasks
        .iter()
        .filter(|task| {
            task.status == solstone_core_system_health::MaintTaskStatus::Unreadable
                || (task.status == solstone_core_system_health::MaintTaskStatus::InProgress
                    && task.ran_ts.is_none())
        })
        .map(|task| format!("{}.{}", task.app, task.task))
        .collect::<Vec<_>>();
    if !unreadable.is_empty() {
        return Ok(make_result(
            check,
            Status::Warn,
            format!(
                "couldn't fully determine — maint state unreadable: {}",
                unreadable.join(", ")
            ),
            Some(FIX),
        ));
    }
    Ok(make_result(
        check,
        Status::Ok,
        "no unresolved maint tasks",
        None::<String>,
    ))
}
