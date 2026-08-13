// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal action-log records.
//!
//! Mirrors the Python reference's `solstone.think.facets._write_action_log`
//! journal-level (`facet=None`) branch: a durable audit trail of user- and
//! agent-initiated actions that are not scoped to any single facet.

use std::path::Path;

use chrono::{Local, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::AppendError;

/// Append a journal-level action-log record to `config/actions/<day>.jsonl`.
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
    solstone_core_journal_io::append_jsonl(destination, &Value::Object(record))
}
