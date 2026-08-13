// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn core_top_dispatch_reaches_top_production_entry() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    let journal = std::env::temp_dir().join(format!("solstone-core-top-reach-{stamp}"));
    fs::create_dir(&journal).expect("create isolated journal path");

    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .arg("top")
        .env("SOLSTONE_JOURNAL", &journal)
        .output()
        .expect("solstone-core top should execute");

    fs::remove_dir_all(&journal).expect("remove isolated journal path");
    assert_eq!(output.status.code(), Some(69));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("solstone-core top: terminal failure:"),
        "top parser/dispatch did not reach the production Top entry: {stderr}"
    );
}
