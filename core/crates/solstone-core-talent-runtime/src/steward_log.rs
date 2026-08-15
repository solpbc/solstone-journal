// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde_json::{Map, Value};

pub fn append_event(
    journal: &Path,
    event: &str,
    mut fields: Map<String, Value>,
) -> std::io::Result<()> {
    let path = journal.join("health/steward.log");
    fs::create_dir_all(path.parent().expect("steward log parent"))?;
    fields.insert("event".to_owned(), Value::String(event.to_owned()));
    fields
        .entry("ts".to_owned())
        .or_insert_with(|| Value::from(chrono::Utc::now().timestamp_millis()));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer(&mut file, &Value::Object(fields)).map_err(std::io::Error::other)?;
    file.write_all(b"\n")
}
