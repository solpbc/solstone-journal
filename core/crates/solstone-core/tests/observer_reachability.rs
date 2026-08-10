// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn observer_list_is_reachable_through_real_binary() {
    let root = std::env::temp_dir().join(format!(
        "solstone-core-observer-reachability-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let directory = root.join("apps/observer/observers");
    fs::create_dir_all(&directory).expect("directory");
    fs::write(directory.join("abcdefgh.json"), r#"{"key":"abcdefgh123","name":"fixture observer","created_at":1,"stats":{"segments_received":1,"bytes_received":2}}"#).expect("record");
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["observer", "list", "--json"])
        .env("SOLSTONE_JOURNAL", &root)
        .output()
        .expect("binary");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8(output.stdout)
            .expect("utf8")
            .contains("fixture observer")
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn observer_create_short_circuits_journal_resolution_and_exits_two() {
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["observer", "create"])
        .env_remove("SOLSTONE_JOURNAL")
        .env("HOME", "/definitely/missing-observer-home")
        .output()
        .expect("binary");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .expect("utf8")
            .starts_with("journal observer create is retired.")
    );
}
