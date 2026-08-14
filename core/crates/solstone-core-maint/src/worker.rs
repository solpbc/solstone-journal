// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use crate::registry::{MaintBodyContext, get_task_by_name};

/// Output from the private one-task worker invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRun {
    pub stdout: Vec<String>,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run exactly one registered task body for the aggregate `solstone-core`
/// private worker command.
pub fn run(args: &[String], journal: &Path) -> WorkerRun {
    let Some(task_name) = parse_worker_args(args) else {
        return WorkerRun {
            stdout: Vec::new(),
            stderr: "journal maint worker: expected __maint-worker --one-task --task <app:name>\n"
                .to_owned(),
            exit_code: 2,
        };
    };
    let Some(task) = get_task_by_name(task_name) else {
        return WorkerRun {
            stdout: Vec::new(),
            stderr: format!("journal maint worker: task not found: {task_name}\n"),
            exit_code: 2,
        };
    };
    if !task_name.contains(':') {
        return WorkerRun {
            stdout: Vec::new(),
            stderr: format!("journal maint worker: task must be qualified: {task_name}\n"),
            exit_code: 2,
        };
    }
    let result = (task.body)(&MaintBodyContext {
        journal,
        dry_run: false,
        verbose: false,
        task_name: Some(task_name),
    });
    WorkerRun {
        stdout: result.stdout,
        stderr: String::new(),
        exit_code: result.exit_code,
    }
}

fn parse_worker_args(args: &[String]) -> Option<&str> {
    match args {
        [command, one_task, task_flag, task_name]
            if command == "__maint-worker" && one_task == "--one-task" && task_flag == "--task" =>
        {
            Some(task_name)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn worker_requires_exact_private_invocation_and_runs_one_native_body() {
        let invalid = run(
            &args(&[
                "__maint-worker",
                "--task",
                "timeline:002_register_segment_summary_model",
            ]),
            Path::new("/unused"),
        );
        assert_eq!(invalid.exit_code, 2);
        let unqualified = run(
            &args(&[
                "__maint-worker",
                "--one-task",
                "--task",
                "000_unify_provider_config",
            ]),
            Path::new("/unused"),
        );
        assert_eq!(unqualified.exit_code, 2);
        let result = run(
            &args(&[
                "__maint-worker",
                "--one-task",
                "--task",
                "timeline:002_register_segment_summary_model",
            ]),
            Path::new("/unused"),
        );
        assert_eq!(result.stdout, ["Skipped retired migration."]);
        assert_eq!(result.exit_code, 0);
    }
}
