// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{DateTime, Local, Utc};

use crate::attempt_log::AttemptLog;
use crate::state::{MaintTaskState, MaintTaskStatus};

pub fn format_duration(ms: i64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    if ms < 60_000 {
        return format!("{}s", ms / 1_000);
    }
    format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1_000)
}

pub fn render_list(tasks: &[MaintTaskState]) -> String {
    if tasks.is_empty() {
        return "No maintenance tasks found.\n".to_owned();
    }
    let groups = [
        ("Pending", MaintTaskStatus::Pending),
        ("In Progress", MaintTaskStatus::InProgress),
        ("Failed", MaintTaskStatus::Failed),
        ("Completed", MaintTaskStatus::Success),
    ];
    let mut output = String::new();
    for (label, status) in groups {
        let matching = tasks
            .iter()
            .filter(|task| task.status == status)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            continue;
        }
        output.push_str(&format!("{label} ({}):\n", matching.len()));
        for task in matching {
            render_task(task, &mut output);
        }
    }
    output
}

pub fn render_task(task: &MaintTaskState, output: &mut String) {
    let description = if task.description.is_empty() {
        String::new()
    } else {
        format!(" - {}", task.description)
    };
    let status = match task.status {
        MaintTaskStatus::InProgress => " (in progress)".to_owned(),
        _ if task.exit_code.is_some_and(|code| code != 0) => {
            format!(" (exit {})", task.exit_code.expect("checked above"))
        }
        _ => String::new(),
    };
    output.push_str(&format!(
        "  {}:{}{description}{status}\n",
        task.app, task.task
    ));
    let Some(timestamp) = task.ran_ts else {
        return;
    };
    let mut parts = vec![format!("ran {}", format_timestamp(timestamp))];
    let mut detail = Vec::new();
    if let Some(duration_ms) = task.duration_ms {
        detail.push(format_duration(duration_ms));
    }
    if task.line_count > 0 {
        detail.push(format!("{} lines", task.line_count));
    }
    if !detail.is_empty() {
        parts.push(format!("({})", detail.join(", ")));
    }
    output.push_str(&format!("    {}\n", parts.join(" ")));
}

pub fn render_task_details(task: &MaintTaskState, attempts: &[AttemptLog]) -> String {
    let mut output = format!("{}:{}\n", task.app, task.task);
    if !task.description.is_empty() {
        output.push_str(&format!("{}\n", task.description));
    }
    let status = match task.status {
        MaintTaskStatus::Pending => "Status: pending".to_owned(),
        MaintTaskStatus::InProgress => "Status: in progress".to_owned(),
        MaintTaskStatus::Success => "Status: success (exit 0)".to_owned(),
        MaintTaskStatus::Failed if task.exit_code.is_none() => "Status: failed".to_owned(),
        MaintTaskStatus::Failed => format!(
            "Status: failed (exit {})",
            task.exit_code.expect("failed exit")
        ),
    };
    output.push_str(&status);
    output.push('\n');
    if let Some(timestamp) = task.ran_ts {
        output.push_str(&format!("Ran: {}", format_timestamp(timestamp)));
        if let Some(duration_ms) = attempts.last().and_then(|attempt| attempt.duration_ms) {
            output.push_str(&format!(" ({duration_ms}ms)"));
        }
        output.push('\n');
    }
    if task.state_file.exists() {
        output.push_str(&format!("Log: {}\n", task.state_file.display()));
    }
    output.push('\n');
    if task.status == MaintTaskStatus::Pending {
        output.push_str("Task has not been run yet.\n");
        return output;
    }
    for (index, attempt) in attempts.iter().rev().enumerate() {
        if index > 0 {
            output.push('\n');
            output.push_str(&format!("Prior attempt {}:\n", index + 1));
        }
        for line in &attempt.lines {
            output.push_str(line);
            output.push('\n');
        }
        for error in &attempt.errors {
            output.push_str(&format!("Error: {error}\n"));
        }
    }
    output
}

pub fn task_not_found(name: &str) -> String {
    format!("Task not found: {name}\nUse 'journal maint --list' to see available tasks.\n")
}

pub const fn no_pending_tasks() -> &'static str {
    "No pending maintenance tasks.\n"
}

pub fn completed_tasks(succeeded: usize, ran: usize) -> String {
    format!("Completed {succeeded}/{ran} task(s)\n")
}

fn format_timestamp(timestamp_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .map(|timestamp| {
            timestamp
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::state::MaintStateIntegrity;

    fn task(
        status: MaintTaskStatus,
        exit_code: Option<i64>,
        ran_ts: Option<i64>,
    ) -> MaintTaskState {
        MaintTaskState {
            app: "app".to_owned(),
            task: format!("{status:?}").to_lowercase(),
            description: "description".to_owned(),
            retry_on_next_start: false,
            blocks_supervisor_start: false,
            status,
            exit_code,
            ran_ts,
            duration_ms: Some(61_001),
            line_count: 2,
            state_file: PathBuf::from("/missing/log.jsonl"),
            integrity: MaintStateIntegrity::Parsed,
        }
    }

    #[test]
    fn list_uses_all_status_groups_and_distinct_metadata() {
        let output = render_list(&[
            task(MaintTaskStatus::Success, Some(0), Some(0)),
            task(MaintTaskStatus::Pending, None, None),
            task(MaintTaskStatus::InProgress, None, Some(1_000)),
            task(MaintTaskStatus::Failed, Some(7), Some(2_000)),
        ]);
        assert!(output.starts_with("Pending (1):"));
        assert!(output.contains("In Progress (1):"));
        assert!(output.contains("(in progress)"));
        assert!(output.contains("Failed (1):"));
        assert!(output.contains("(exit 7)"));
        assert!(output.contains("Completed (1):"));
        assert!(output.contains("(1m 1s, 2 lines)"));
        assert_eq!(render_list(&[]), "No maintenance tasks found.\n");
        assert_eq!(format_duration(999), "999ms");
        assert_eq!(format_duration(1_000), "1s");
    }

    #[test]
    fn details_replay_current_then_prior_attempt() {
        let detail = render_task_details(
            &task(MaintTaskStatus::Failed, Some(3), Some(0)),
            &[
                AttemptLog {
                    lines: vec!["old".to_owned()],
                    errors: vec!["old error".to_owned()],
                    duration_ms: Some(1),
                },
                AttemptLog {
                    lines: vec!["new".to_owned()],
                    errors: Vec::new(),
                    duration_ms: Some(2),
                },
            ],
        );
        assert!(detail.contains("Status: failed (exit 3)"));
        assert!(detail.contains("Ran: "));
        assert!(detail.contains("new\n\nPrior attempt 2:\nold\nError: old error\n"));
        assert_eq!(
            task_not_found("missing"),
            "Task not found: missing\nUse 'journal maint --list' to see available tasks.\n"
        );
    }
}
