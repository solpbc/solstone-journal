// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use axum::Router;
use serde_json::Value;
use tempfile::TempDir;

pub fn corpus() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/convey_settings_corpus.json"
    )))
    .expect("settings corpus")
}

pub fn established_root() -> TempDir {
    let root = TempDir::new().expect("temporary journal");
    write_json(
        root.path(),
        "config/journal.json",
        &serde_json::json!({"setup": {"completed_at": 1_700_000_000_000_i64}}),
    );
    root
}

pub fn populated_root() -> TempDir {
    let root = TempDir::new().expect("temporary journal");
    let corpus = corpus();
    let files = corpus["phases"]["populated"]["_journal_tree"]["files"]
        .as_object()
        .expect("populated tree");
    for (relative, value) in files {
        let target = root.path().join(relative);
        fs::create_dir_all(target.parent().expect("parent")).expect("tree parent");
        match value.as_str().expect("tree text") {
            "<BINARY>" if relative.ends_with("audio.flac") => {
                fs::write(target, vec![0_u8; 4096]).expect("audio")
            }
            "<BINARY>" if relative.ends_with("monitor_1_diff.png") => {
                fs::write(target, vec![0_u8; 2048]).expect("screen")
            }
            "<BINARY>" => panic!("unknown binary fixture file: {relative}"),
            text => fs::write(target, text).expect("tree file"),
        }
    }
    root
}

pub fn corrupt_root() -> TempDir {
    let root = TempDir::new().expect("temporary journal");
    fs::create_dir_all(root.path().join("config")).expect("config directory");
    fs::write(
        root.path().join("config/journal.json"),
        "{\"identity\": {\"name\": \"Ada\",\n",
    )
    .expect("corrupt config");
    root
}

pub fn shell_router(root: &Path) -> Router {
    solstone_core_convey_shell::router(root.to_path_buf())
}

fn write_json(root: &Path, relative: &str, value: &Value) {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("directory");
    fs::write(
        target,
        format!("{}\n", serde_json::to_string_pretty(value).expect("JSON")),
    )
    .expect("config");
}
