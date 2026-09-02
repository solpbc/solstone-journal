// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal action-log records.
//!
//! Mirrors the Python reference's `solstone.think.facets._write_action_log`
//! both the journal-level (`facet=None`) and facet-scoped branches: a durable
//! audit trail of user- and agent-initiated actions.

use std::path::Path;

use chrono::{Local, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::{AppendError, append_text};

/// Append an action-log record to the journal-level or facet-scoped destination.
pub fn append_action_log(
    journal_root: &Path,
    facet: Option<&str>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
) -> Result<(), AppendError> {
    let day = Local::now().format("%Y%m%d").to_string();
    append_action_log_at_day(journal_root, facet, source, actor, action, params, &day)
}

pub fn append_action_log_for_day(
    journal_root: &Path,
    facet: Option<&str>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
    day: &str,
) -> Result<(), AppendError> {
    append_action_log_at_day(journal_root, facet, source, actor, action, params, day)
}

fn append_action_log_at_day(
    journal_root: &Path,
    facet: Option<&str>,
    source: &str,
    actor: &str,
    action: &str,
    params: Value,
    day: &str,
) -> Result<(), AppendError> {
    let destination = match facet {
        Some(facet) => journal_root
            .join("facets")
            .join(facet)
            .join("logs")
            .join(format!("{day}.jsonl")),
        None => journal_root
            .join("config/actions")
            .join(format!("{day}.jsonl")),
    };
    let mut record = serde_json::Map::new();
    record.insert("timestamp".to_owned(), json!(Utc::now().to_rfc3339()));
    record.insert("source".to_owned(), json!(source));
    record.insert("actor".to_owned(), json!(actor));
    record.insert("action".to_owned(), json!(action));
    record.insert("params".to_owned(), params);
    if let Some(facet) = facet {
        record.insert("facet".to_owned(), json!(facet));
    }
    append_text(destination, &python_json(&Value::Object(record)))
}

fn python_json(value: &Value) -> String {
    let compact = serde_json::to_string(value).expect("action log JSON");
    let mut output = String::with_capacity(compact.len());
    let mut in_string = false;
    let mut escaped = false;
    for character in compact.chars() {
        output.push(character);
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
        } else if matches!(character, ',' | ':') {
            output.push(' ');
        }
    }
    output
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::append_action_log;

    #[test]
    fn action_logs_preserve_python_insertion_order_and_unicode() {
        let root = crate::store_tests::TempDir::new();
        append_action_log(
            root.path(),
            Some("work"),
            "app",
            "settings",
            "activity_add",
            json!({"emoji":"🧪"}),
        )
        .expect("action log");
        let day = chrono::Local::now().format("%Y%m%d").to_string();
        let line = fs::read_to_string(
            root.path()
                .join("facets/work/logs")
                .join(format!("{day}.jsonl")),
        )
        .expect("action log file");
        assert!(line.starts_with("{\"timestamp\": "));
        assert!(line.contains("\"emoji\": \"🧪\""));
        assert!(!line.contains("\\u"));
    }
}
