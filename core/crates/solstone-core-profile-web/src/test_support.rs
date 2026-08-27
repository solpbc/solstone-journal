// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::Value;
use tempfile::TempDir;

pub(crate) fn journal() -> TempDir {
    TempDir::new_in("/var/tmp").expect("temporary journal")
}

pub(crate) fn write_json(root: &Path, relative: &str, value: Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
    fs::write(
        path,
        format!("{}\n", serde_json::to_string(&value).expect("JSON")),
    )
    .expect("write JSON");
}

pub(crate) fn write_jsonl(root: &Path, relative: &str, rows: &[Value]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
    let content = rows
        .iter()
        .map(|row| serde_json::to_string(row).expect("JSON"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, format!("{content}\n")).expect("write JSONL");
}
