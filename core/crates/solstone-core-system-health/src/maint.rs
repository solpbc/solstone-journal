// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only maintenance task state from `journal/maint`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Static task metadata supplied by the native maint registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintTaskDefinition<'a> {
    pub app: &'a str,
    pub task: &'a str,
    pub description: &'a str,
    pub retry_on_next_start: bool,
    pub blocks_supervisor_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintTaskStatus {
    Pending,
    InProgress,
    Success,
    Failed,
}

/// Whether state input was sufficiently readable to trust its interpretation.
///
/// This is deliberately separate from the four observable task statuses:
/// Python renders present-but-unusable files as in-progress, while doctor must
/// continue to warn that it could not fully determine their state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintStateIntegrity {
    Parsed,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintTaskState {
    pub app: String,
    pub task: String,
    pub description: String,
    pub retry_on_next_start: bool,
    pub blocks_supervisor_start: bool,
    pub status: MaintTaskStatus,
    pub exit_code: Option<i64>,
    pub ran_ts: Option<i64>,
    pub duration_ms: Option<i64>,
    pub line_count: usize,
    pub state_file: PathBuf,
    pub integrity: MaintStateIntegrity,
}

/// Read every static task and any pre-existing state file not yet represented
/// by that registry.  The supplemental state-file pass preserves doctor
/// visibility for historical task records during the native cutover.
pub fn read_maint_task_states(
    journal_path: &Path,
    tasks: &[MaintTaskDefinition<'_>],
) -> Vec<MaintTaskState> {
    let mut states = tasks
        .iter()
        .map(|task| read_maint_task_state(journal_path, *task))
        .collect::<Vec<_>>();
    let root = journal_path.join("maint");
    let Ok(apps) = sorted_entries(&root) else {
        return states;
    };
    for app in apps {
        if !app.is_dir() {
            continue;
        }
        let Some(app_name) = app.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(files) = sorted_entries(&app) else {
            continue;
        };
        for file in files {
            if file.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(task_name) = file.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            if tasks
                .iter()
                .any(|task| task.app == app_name && task.task == task_name)
            {
                continue;
            }
            states.push(read_maint_task_state(
                journal_path,
                MaintTaskDefinition {
                    app: app_name,
                    task: task_name,
                    description: "",
                    retry_on_next_start: false,
                    blocks_supervisor_start: false,
                },
            ));
        }
    }
    states
}

/// Read one named task. A missing durable state file is pending.
pub fn read_maint_task_state(journal_path: &Path, task: MaintTaskDefinition<'_>) -> MaintTaskState {
    let state_file = maint_state_file(journal_path, task.app, task.task);
    let state = if state_file.exists() {
        read_task_state(&state_file)
    } else {
        ParsedState::pending()
    };
    MaintTaskState {
        app: task.app.to_owned(),
        task: task.task.to_owned(),
        description: task.description.to_owned(),
        retry_on_next_start: task.retry_on_next_start,
        blocks_supervisor_start: task.blocks_supervisor_start,
        status: state.status,
        exit_code: state.exit_code,
        ran_ts: state.ran_ts,
        duration_ms: state.duration_ms,
        line_count: state.line_count,
        state_file,
        integrity: state.integrity,
    }
}

pub fn maint_state_file(journal_path: &Path, app: &str, task: &str) -> PathBuf {
    journal_path
        .join("maint")
        .join(app)
        .join(format!("{task}.jsonl"))
}

fn sorted_entries(path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    Ok(entries)
}

struct ParsedState {
    status: MaintTaskStatus,
    exit_code: Option<i64>,
    ran_ts: Option<i64>,
    duration_ms: Option<i64>,
    line_count: usize,
    integrity: MaintStateIntegrity,
}

impl ParsedState {
    fn pending() -> Self {
        Self {
            status: MaintTaskStatus::Pending,
            exit_code: None,
            ran_ts: None,
            duration_ms: None,
            line_count: 0,
            integrity: MaintStateIntegrity::Parsed,
        }
    }
}

fn read_task_state(path: &Path) -> ParsedState {
    let Ok(text) = fs::read_to_string(path) else {
        return ParsedState {
            status: MaintTaskStatus::InProgress,
            exit_code: None,
            ran_ts: None,
            duration_ms: None,
            line_count: 0,
            integrity: MaintStateIntegrity::Unreadable,
        };
    };
    let events = latest_attempt_events(&text);
    let integrity = if events.is_empty() && text.lines().any(|line| !line.trim().is_empty()) {
        MaintStateIntegrity::Unreadable
    } else {
        MaintStateIntegrity::Parsed
    };
    let mut exec_ts = None;
    let mut last = None;
    let mut line_count = 0;
    for event in &events {
        match event.get("event").and_then(Value::as_str) {
            Some("exec") if exec_ts.is_none() => {
                exec_ts = event.get("ts").and_then(Value::as_i64);
            }
            Some("line") => line_count += 1,
            _ => {}
        }
        last = Some(event);
    }
    if let Some(event) = last
        && event.get("event").and_then(Value::as_str) == Some("exit")
    {
        let exit_code = event.get("exit_code").and_then(Value::as_i64).unwrap_or(-1);
        let timestamp = event.get("ts").and_then(Value::as_i64);
        return ParsedState {
            status: if exit_code == 0 {
                MaintTaskStatus::Success
            } else {
                MaintTaskStatus::Failed
            },
            exit_code: Some(exit_code),
            ran_ts: timestamp.or(exec_ts),
            duration_ms: event.get("duration_ms").and_then(Value::as_i64),
            line_count,
            integrity,
        };
    }
    ParsedState {
        status: MaintTaskStatus::InProgress,
        exit_code: None,
        ran_ts: exec_ts,
        duration_ms: None,
        line_count,
        integrity,
    }
}

/// Return the valid JSON-object rows in the final attempt block.
pub fn latest_attempt_events(text: &str) -> Vec<Value> {
    let events = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter(Value::is_object)
        .collect::<Vec<_>>();
    let mut blocks = Vec::<Vec<Value>>::new();
    let mut current = Vec::new();
    let mut attempt = None::<String>;
    for event in events {
        let kind = event.get("event").and_then(Value::as_str);
        if kind == Some("exec") {
            if !current.is_empty() {
                blocks.push(current);
            }
            attempt = event
                .get("attempt_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            current = vec![event];
            continue;
        }
        let event_attempt = event.get("attempt_id").and_then(Value::as_str);
        if attempt.is_some() && event_attempt.is_some_and(|value| Some(value) != attempt.as_deref())
        {
            if !current.is_empty() {
                blocks.push(current);
            }
            attempt = event_attempt.map(str::to_owned);
            current = vec![event];
            continue;
        }
        current.push(event);
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks.pop().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const TASK: MaintTaskDefinition<'static> = MaintTaskDefinition {
        app: "app",
        task: "task",
        description: "description",
        retry_on_next_start: true,
        blocks_supervisor_start: false,
    };

    #[test]
    fn registry_tasks_include_missing_files_and_latest_attempt_metadata() {
        let root = tempdir().expect("temp journal");
        let state = maint_state_file(root.path(), "app", "task");
        fs::create_dir_all(state.parent().expect("parent")).expect("create state parent");
        fs::write(&state, "{\"event\":\"exec\",\"attempt_id\":\"old\",\"ts\":1}\n{\"event\":\"exit\",\"attempt_id\":\"old\",\"exit_code\":1,\"ts\":2}\n{\"event\":\"exec\",\"attempt_id\":\"new\",\"ts\":3}\n{\"event\":\"line\",\"attempt_id\":\"new\",\"line\":\"one\"}\n{\"event\":\"line\",\"attempt_id\":\"new\",\"line\":\"two\"}\n{\"event\":\"exit\",\"attempt_id\":\"new\",\"exit_code\":0,\"duration_ms\":9,\"ts\":4}\n").expect("write state");
        let states = read_maint_task_states(root.path(), &[TASK]);
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].status, MaintTaskStatus::Success);
        assert_eq!(states[0].duration_ms, Some(9));
        assert_eq!(states[0].line_count, 2);
        assert_eq!(states[0].description, "description");
        assert!(states[0].retry_on_next_start);
    }

    #[test]
    fn absent_blank_malformed_and_incomplete_states_match_python_fallbacks() {
        let root = tempdir().expect("temp journal");
        assert_eq!(
            read_maint_task_state(root.path(), TASK).status,
            MaintTaskStatus::Pending
        );
        let state = maint_state_file(root.path(), "app", "task");
        fs::create_dir_all(state.parent().expect("parent")).expect("create state parent");
        fs::write(&state, "\nnot json\n").expect("write malformed state");
        let malformed = read_maint_task_state(root.path(), TASK);
        assert_eq!(malformed.status, MaintTaskStatus::InProgress);
        assert_eq!(malformed.integrity, MaintStateIntegrity::Unreadable);
        fs::write(&state, "\n\n").expect("write blank state");
        assert_eq!(
            read_maint_task_state(root.path(), TASK).integrity,
            MaintStateIntegrity::Parsed
        );
        fs::write(&state, "not json\n{\"event\":\"exec\",\"ts\":5}\n")
            .expect("write incomplete state");
        let incomplete = read_maint_task_state(root.path(), TASK);
        assert_eq!(incomplete.status, MaintTaskStatus::InProgress);
        assert_eq!(incomplete.ran_ts, Some(5));
    }

    #[test]
    fn failed_exit_defaults_missing_code_to_minus_one() {
        let root = tempdir().expect("temp journal");
        let state = maint_state_file(root.path(), "app", "task");
        fs::create_dir_all(state.parent().expect("parent")).expect("create state parent");
        fs::write(
            &state,
            "{\"event\":\"exec\",\"ts\":1}\n{\"event\":\"exit\",\"ts\":2}\n",
        )
        .expect("write state");
        let failed = read_maint_task_state(root.path(), TASK);
        assert_eq!(failed.status, MaintTaskStatus::Failed);
        assert_eq!(failed.exit_code, Some(-1));
        assert_eq!(failed.ran_ts, Some(2));
    }

    #[test]
    fn supplemental_historical_state_remains_visible() {
        let root = tempdir().expect("temp journal");
        let state = maint_state_file(root.path(), "settings", "reindex");
        fs::create_dir_all(state.parent().expect("parent")).expect("create state parent");
        fs::write(state, "{\"event\":\"exit\",\"exit_code\":3,\"ts\":2}\n").expect("write state");
        let states = read_maint_task_states(root.path(), &[TASK]);
        assert_eq!(states.len(), 2);
        assert_eq!(states[1].app, "settings");
        assert_eq!(states[1].status, MaintTaskStatus::Failed);
    }
}
