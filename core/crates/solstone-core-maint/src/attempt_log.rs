// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::registry::MaintTask;

pub struct AttemptLogWriter {
    file: File,
    pub path: PathBuf,
    pub attempt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptExec {
    pub attempt_id: String,
    pub ts: i64,
    pub app: String,
    pub task: String,
    pub cmd: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptLine {
    pub attempt_id: String,
    pub ts: i64,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptExit {
    pub attempt_id: String,
    pub ts: i64,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub enum MaintAttemptEvent {
    Exec(AttemptExec),
    Line(AttemptLine),
    Exit(AttemptExit),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptLog {
    pub lines: Vec<String>,
    pub errors: Vec<String>,
    pub duration_ms: Option<i64>,
}

pub fn open_attempt_log(
    journal: &Path,
    task: MaintTask,
    attempt_id: String,
) -> io::Result<AttemptLogWriter> {
    let directory = journal.join("maint").join(task.app);
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{}.jsonl", task.name));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok(AttemptLogWriter {
        file,
        path,
        attempt_id,
    })
}

/// Append one JSONL event and flush it without fsyncing, matching Python.
pub fn append_attempt_event(
    writer: &mut AttemptLogWriter,
    event: &MaintAttemptEvent,
) -> io::Result<()> {
    let value = match event {
        MaintAttemptEvent::Exec(event) => serde_json::to_value(event),
        MaintAttemptEvent::Line(event) => serde_json::to_value(event),
        MaintAttemptEvent::Exit(event) => serde_json::to_value(event),
    }
    .map_err(io::Error::other)?;
    let mut object = value
        .as_object()
        .cloned()
        .expect("serialized event is an object");
    let name = match event {
        MaintAttemptEvent::Exec(_) => "exec",
        MaintAttemptEvent::Line(_) => "line",
        MaintAttemptEvent::Exit(_) => "exit",
    };
    object.insert("event".to_owned(), Value::String(name.to_owned()));
    serde_json::to_writer(&mut writer.file, &Value::Object(object)).map_err(io::Error::other)?;
    writer.file.write_all(b"\n")?;
    writer.file.flush()
}

pub fn read_attempt_logs(path: &Path) -> io::Result<Vec<AttemptLog>> {
    let file = File::open(path)?;
    let mut attempts = Vec::new();
    let mut current = None::<AttemptLog>;
    for raw_line in BufReader::new(file).lines() {
        let raw_line = raw_line?;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let event_type = event.get("event").and_then(Value::as_str);
        if event_type == Some("exec") {
            if let Some(attempt) = current.take() {
                attempts.push(attempt);
            }
            current = Some(empty_attempt());
            continue;
        }
        let attempt = current.get_or_insert_with(empty_attempt);
        match event_type {
            Some("line") => {
                if let Some(text) = event.get("line").and_then(Value::as_str) {
                    attempt.lines.push(text.to_owned());
                }
            }
            Some("exit") => {
                if let Some(duration_ms) = event.get("duration_ms").and_then(Value::as_i64) {
                    attempt.duration_ms = Some(duration_ms);
                }
                if let Some(error) = event.get("error")
                    && !error.is_null()
                {
                    attempt
                        .errors
                        .push(error.to_string().trim_matches('"').to_owned());
                }
            }
            _ => {}
        }
    }
    if let Some(attempt) = current {
        attempts.push(attempt);
    }
    Ok(attempts)
}

fn empty_attempt() -> AttemptLog {
    AttemptLog {
        lines: Vec::new(),
        errors: Vec::new(),
        duration_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tasks;
    use tempfile::tempdir;

    #[test]
    fn writer_flushes_python_shape_and_preserves_prior_attempts() {
        let journal = tempdir().expect("journal");
        let task = tasks()[0];
        let mut first = open_attempt_log(journal.path(), task, "first".to_owned()).expect("open");
        append_attempt_event(
            &mut first,
            &MaintAttemptEvent::Exec(AttemptExec {
                attempt_id: "first".to_owned(),
                ts: 1,
                app: task.app.to_owned(),
                task: task.name.to_owned(),
                cmd: vec!["cmd".to_owned()],
            }),
        )
        .expect("exec");
        assert!(
            fs::read_to_string(&first.path)
                .expect("read after flush")
                .contains("\"event\":\"exec\"")
        );
        append_attempt_event(
            &mut first,
            &MaintAttemptEvent::Line(AttemptLine {
                attempt_id: "first".to_owned(),
                ts: 2,
                line: "hello".to_owned(),
            }),
        )
        .expect("line");
        append_attempt_event(
            &mut first,
            &MaintAttemptEvent::Exit(AttemptExit {
                attempt_id: "first".to_owned(),
                ts: 3,
                exit_code: 0,
                duration_ms: Some(2),
                error: None,
            }),
        )
        .expect("exit");
        let mut second =
            open_attempt_log(journal.path(), task, "second".to_owned()).expect("open second");
        append_attempt_event(
            &mut second,
            &MaintAttemptEvent::Exec(AttemptExec {
                attempt_id: "second".to_owned(),
                ts: 4,
                app: task.app.to_owned(),
                task: task.name.to_owned(),
                cmd: vec!["cmd".to_owned()],
            }),
        )
        .expect("second exec");
        let raw = fs::read_to_string(&second.path).expect("read raw");
        assert!(raw.contains("\"attempt_id\":\"first\""));
        assert!(raw.contains("\"attempt_id\":\"second\""));
        let attempts = read_attempt_logs(&second.path).expect("read attempts");
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].lines, ["hello"]);
        assert_eq!(attempts[0].duration_ms, Some(2));
    }
}
