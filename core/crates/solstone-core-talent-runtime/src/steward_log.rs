// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
use std::path::Path;

use chrono::Local;
use serde_json::{Map, Value};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, create_oplog_at},
};

pub fn append_event(
    journal: &Path,
    event: &str,
    mut fields: Map<String, Value>,
) -> std::io::Result<()> {
    fields.insert("event".to_owned(), Value::String(event.to_owned()));
    fields
        .entry("ts".to_owned())
        .or_insert_with(|| Value::from(chrono::Utc::now().timestamp_millis()));
    let root =
        JournalRoot::open(journal).map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut log = create_oplog_at(
        root,
        "steward",
        "pre-hook",
        OplogFormat::Jsonl,
        Local::now().fixed_offset(),
    )
    .map_err(|error| std::io::Error::other(format!("failed to create steward oplog: {error}")))?;
    serde_json::to_writer(&mut log, &Value::Object(fields)).map_err(std::io::Error::other)?;
    log.write_all(b"\n")?;
    log.flush()
}
