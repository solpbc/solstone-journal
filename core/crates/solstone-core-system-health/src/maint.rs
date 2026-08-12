// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only maintenance task state from `journal/maint`.

use std::fs;
use std::path::Path;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintTaskStatus {
    Pending,
    InProgress,
    Unreadable,
    Success,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintTaskState {
    pub app: String,
    pub task: String,
    pub status: MaintTaskStatus,
    pub exit_code: Option<i64>,
    pub ran_ts: Option<i64>,
}

/// Read the state files that exist beneath `journal/maint`.
///
/// Source tasks without a state file are intentionally invisible here: this
/// owner reads durable task state, not the Python source tree.
pub fn read_maint_task_states(journal_path: &Path) -> Vec<MaintTaskState> {
    let root = journal_path.join("maint");
    let Ok(apps) = sorted_entries(&root) else {
        return Vec::new();
    };
    let mut states = Vec::new();
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
            let Some(task) = file.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            states.push(read_maint_task_state(journal_path, app_name, task));
        }
    }
    states
}

/// Read one named maintenance task. A missing durable state file is pending.
pub fn read_maint_task_state(journal_path: &Path, app: &str, task: &str) -> MaintTaskState {
    let state_file = journal_path
        .join("maint")
        .join(app)
        .join(format!("{task}.jsonl"));
    let (status, exit_code, ran_ts) = if state_file.exists() {
        read_task_state(&state_file)
    } else {
        (MaintTaskStatus::Pending, None, None)
    };
    MaintTaskState {
        app: app.to_owned(),
        task: task.to_owned(),
        status,
        exit_code,
        ran_ts,
    }
}

fn sorted_entries(path: &Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    Ok(entries)
}

fn read_task_state(path: &Path) -> (MaintTaskStatus, Option<i64>, Option<i64>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (MaintTaskStatus::Unreadable, None, None);
    };
    let events = latest_attempt_events(&text);
    if events.is_empty() && text.lines().any(|line| !line.trim().is_empty()) {
        return (MaintTaskStatus::Unreadable, None, None);
    }
    let mut exec_ts = None;
    let mut last = None;
    for event in &events {
        if event.get("event").and_then(Value::as_str) == Some("exec") && exec_ts.is_none() {
            exec_ts = event.get("ts").and_then(Value::as_i64);
        }
        last = Some(event);
    }
    if let Some(event) = last
        && event.get("event").and_then(Value::as_str) == Some("exit")
    {
        let exit_code = event.get("exit_code").and_then(Value::as_i64).unwrap_or(-1);
        let timestamp = event.get("ts").and_then(Value::as_i64);
        return if exit_code == 0 {
            (MaintTaskStatus::Success, Some(0), timestamp)
        } else {
            (MaintTaskStatus::Failed, Some(exit_code), timestamp)
        };
    }
    if exec_ts.is_some() {
        return (MaintTaskStatus::InProgress, None, exec_ts);
    }
    (MaintTaskStatus::InProgress, None, None)
}

fn latest_attempt_events(text: &str) -> Vec<Value> {
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

    #[test]
    fn reads_latest_attempt_and_exit_status() {
        let root = tempdir().unwrap();
        let state = root.path().join("maint/app/task.jsonl");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, "{\"event\":\"exec\",\"attempt_id\":\"old\",\"ts\":1}\n{\"event\":\"exit\",\"attempt_id\":\"old\",\"exit_code\":1,\"ts\":2}\n{\"event\":\"exec\",\"attempt_id\":\"new\",\"ts\":3}\n{\"event\":\"exit\",\"attempt_id\":\"new\",\"exit_code\":0,\"ts\":4}\n").unwrap();
        assert_eq!(
            read_maint_task_states(root.path()),
            vec![MaintTaskState {
                app: "app".into(),
                task: "task".into(),
                status: MaintTaskStatus::Success,
                exit_code: Some(0),
                ran_ts: Some(4)
            }]
        );
    }

    #[test]
    fn incomplete_or_invalid_file_is_in_progress() {
        let root = tempdir().unwrap();
        let state = root.path().join("maint/app/task.jsonl");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, "not json\n{\"event\":\"exec\",\"ts\":5}\n").unwrap();
        let result = read_maint_task_states(root.path());
        assert_eq!(result[0].status, MaintTaskStatus::InProgress);
        assert_eq!(result[0].ran_ts, Some(5));
    }

    #[test]
    fn distinguishes_unreadable_state_from_missing_exec_timestamp() {
        let root = tempdir().unwrap();
        let app = root.path().join("maint/app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("unreadable.jsonl"), "not json\n").unwrap();
        fs::write(
            app.join("missing-timestamp.jsonl"),
            "{\"event\":\"exec\"}\n",
        )
        .unwrap();

        let states = read_maint_task_states(root.path());
        assert_eq!(states[0].status, MaintTaskStatus::InProgress);
        assert_eq!(states[0].ran_ts, None);
        assert_eq!(states[1].status, MaintTaskStatus::Unreadable);
    }

    #[test]
    fn missing_named_task_is_pending() {
        let root = tempdir().unwrap();
        assert_eq!(
            read_maint_task_state(root.path(), "app", "task").status,
            MaintTaskStatus::Pending
        );
    }
}
