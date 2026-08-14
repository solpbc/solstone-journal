// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native parser, registry, state presentation, and JSONL attempt records for
//! one-time `journal maint` migrations.

pub mod attempt_log;
pub mod bodies;
pub mod parser;
pub mod registry;
pub mod render;
pub mod runner;
pub mod state;
pub mod worker;

use std::path::Path;

use attempt_log::read_attempt_logs;
use parser::{ParseOutcome, parse};
use registry::{get_task_by_name, tasks};
use render::{no_pending_tasks, render_list, render_task, render_task_details, task_not_found};
use runner::{ProductionRunnerPlatform, run_forced_task, run_pending_tasks};
use state::read_states;

/// Captured command output for the aggregate journal dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run the native maint command surface.
pub fn run_cli(args: &[String], journal: &Path) -> CliRun {
    let parsed = match parse(args) {
        ParseOutcome::Parsed(parsed) => parsed,
        ParseOutcome::Help => return success(parser::help().to_owned()),
        ParseOutcome::Error(stderr) => return usage_error(stderr),
    };
    let states = read_states(journal);
    if parsed.list {
        return success(render_list(&states));
    }
    if parsed.force {
        let Some(name) = parsed.task.as_deref() else {
            return failure(
                "--force requires a task name.\nUsage: journal maint --force <task>\n".to_owned(),
            );
        };
        if get_task_by_name(name).is_none() {
            return failure(task_not_found(name));
        }
        let platform = match ProductionRunnerPlatform::new() {
            Ok(platform) => platform,
            Err(error) => return failure(format!("Unable to start maint worker: {error}\n")),
        };
        let outcome = run_forced_task(
            &platform,
            &get_task_by_name(name).expect("checked above"),
            journal,
        );
        return CliRun {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: outcome.exit_code,
        };
    }
    if let Some(name) = parsed.task.as_deref() {
        let Some(task) = get_task_by_name(name) else {
            return failure(task_not_found(name));
        };
        let state = states
            .iter()
            .find(|state| state.app == task.app && state.task == task.name)
            .expect("static task state is present");
        let attempts =
            if state.status == state::MaintTaskStatus::Pending || !state.state_file.exists() {
                Vec::new()
            } else {
                read_attempt_logs(&state.state_file).unwrap_or_default()
            };
        return success(render_task_details(state, &attempts));
    }
    let mut stdout = String::new();
    let in_progress = states
        .iter()
        .filter(|state| state.status == state::MaintTaskStatus::InProgress)
        .collect::<Vec<_>>();
    if !in_progress.is_empty() {
        stdout.push_str(&format!("In Progress ({}):\n", in_progress.len()));
        for task in in_progress {
            render_task(task, &mut stdout);
        }
        stdout.push('\n');
    }
    let platform = match ProductionRunnerPlatform::new() {
        Ok(platform) => platform,
        Err(error) => {
            return failure_with_stdout(stdout, format!("Unable to start maint worker: {error}\n"));
        }
    };
    let outcomes = run_pending_tasks(&platform, journal);
    if outcomes.is_empty() {
        stdout.push_str(no_pending_tasks());
        return success(stdout);
    }
    let succeeded = outcomes.iter().filter(|outcome| outcome.success).count();
    stdout.push_str(&render::completed_tasks(succeeded, outcomes.len()));
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: if succeeded == outcomes.len() { 0 } else { 1 },
    }
}

pub fn registry_tasks() -> &'static [registry::MaintTask] {
    tasks()
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(stderr: String) -> CliRun {
    failure_with_stdout(String::new(), stderr)
}

fn usage_error(stderr: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr,
        exit_code: 2,
    }
}

fn failure_with_stdout(stdout: String, stderr: String) -> CliRun {
    CliRun {
        stdout,
        stderr,
        exit_code: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn list_wins_over_force_and_task() {
        let journal = tempdir().expect("journal");
        let result = run_cli(
            &args(&[
                "--list",
                "--force",
                "activities:000_migrate_activity_icon_to_emoji",
            ]),
            journal.path(),
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.starts_with("Pending (27):"));
    }

    #[test]
    fn details_and_force_argument_errors_are_clear() {
        let journal = tempdir().expect("journal");
        let detail = run_cli(&args(&["--", "--list"]), journal.path());
        assert_eq!(detail.exit_code, 1);
        assert_eq!(detail.stderr, task_not_found("--list"));
        let force = run_cli(&args(&["--force"]), journal.path());
        assert_eq!(force.exit_code, 1);
        assert_eq!(
            force.stderr,
            "--force requires a task name.\nUsage: journal maint --force <task>\n"
        );
    }
}
