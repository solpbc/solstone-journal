// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal action-log records.
//!
//! Mirrors the Python reference's `solstone.think.facets._write_action_log`
//! both the journal-level (`facet=None`) and facet-scoped branches: a durable
//! audit trail of user- and agent-initiated actions.

use std::path::Path;

use chrono::{Local, Utc};
use serde_json::{Map, Value, json};
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
    append_text(
        destination,
        &python_json(&sorted_json(&Value::Object(record))),
    )
}

fn sorted_json(value: &Value) -> Value {
    match value {
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), sorted_json(&values[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.iter().map(sorted_json).collect()),
        value => value.clone(),
    }
}

fn python_json(value: &Value) -> String {
    let compact = serde_json::to_string(value).expect("action log JSON");
    let mut output = String::with_capacity(compact.len());
    let mut in_string = false;
    let mut escaped = false;
    for character in compact.chars() {
        if character.is_ascii() {
            output.push(character);
        } else {
            push_ascii_escape(&mut output, character);
        }
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

fn push_ascii_escape(output: &mut String, character: char) {
    let code_point = character as u32;
    if code_point <= 0xffff {
        output.push_str(&format!("\\u{code_point:04x}"));
        return;
    }
    let surrogate = code_point - 0x1_0000;
    let high = 0xd800 + (surrogate >> 10);
    let low = 0xdc00 + (surrogate & 0x3ff);
    output.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
}
