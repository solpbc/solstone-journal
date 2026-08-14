// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use solstone_core_maint::registry::get_task_by_name;
use solstone_core_maint::runner::{ProductionRunnerPlatform, run_task_with};
use tempfile::tempdir;

#[test]
fn helper_binary_self_spawn_captures_native_worker_output() {
    let journal = tempdir().expect("journal");
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_maint-worker-helper"));
    let platform = ProductionRunnerPlatform::with_executable(helper);
    let task =
        get_task_by_name("timeline:002_register_segment_summary_model").expect("registered task");
    let outcome = run_task_with(&platform, &task, journal.path());
    assert!(outcome.success);
    assert_eq!(outcome.exit_code, 0);
    let rows = fs::read_to_string(outcome.state_file.expect("state file"))
        .expect("read state")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON row"))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["event"], "exec");
    assert_eq!(rows[1]["line"], "Skipped retired migration.");
    assert_eq!(rows[2]["exit_code"], 0);
}
