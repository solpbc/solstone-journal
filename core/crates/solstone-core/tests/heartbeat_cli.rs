// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");

fn run(journal: &TempDir, args: &[&str]) -> Output {
    Command::new(BINARY)
        .args(args)
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("HOME", journal.path().join("home"))
        .output()
        .expect("run solstone-core")
}

#[test]
fn heartbeat_parser_owns_invalid_arguments_and_help() {
    let journal = TempDir::new().unwrap();
    let invalid = run(&journal, &["heartbeat", "--nonsense"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert!(
        invalid
            .stderr
            .starts_with(b"usage: journal heartbeat [-h] [--force]\n")
    );

    let help = run(&journal, &["heartbeat", "--help"]);
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    assert!(help.stdout.starts_with(b"usage: journal heartbeat"));

    let repeated_force = run(&journal, &["heartbeat", "--force", "--force"]);
    assert!(repeated_force.status.success(), "{repeated_force:?}");
    let option_end = run(&journal, &["heartbeat", "--"]);
    assert!(option_end.status.success(), "{option_end:?}");
    let post_end_help = run(&journal, &["heartbeat", "--", "--help"]);
    assert_eq!(post_end_help.status.code(), Some(2));
}

#[test]
fn heartbeat_appends_pass_and_success_rows_then_removes_pid() {
    let journal = TempDir::new().unwrap();
    let output = run(&journal, &["heartbeat"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());

    let health = journal.path().join("health");
    assert!(!health.join("heartbeat.pid").exists());
    let steward = fs::read_to_string(health.join("steward.log")).unwrap();
    let event: Value = serde_json::from_str(steward.trim()).unwrap();
    assert_eq!(event["event"], "pass");
    assert_eq!(event["fired"], serde_json::json!([]));
    assert_eq!(event["escalated_targets"], serde_json::json!([]));
    assert_eq!(event["data_source_errors"], serde_json::json!([]));
    assert!(event["ts"].as_i64().is_some());
    let heartbeat = fs::read_to_string(health.join("heartbeat.log")).unwrap();
    assert!(heartbeat.contains("duration=0s outcome=success\n"));
}

#[test]
fn recent_success_skips_without_touching_steward_and_force_runs() {
    let journal = TempDir::new().unwrap();
    let health = journal.path().join("health");
    fs::create_dir_all(&health).unwrap();
    let stamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    let original = format!("{stamp} duration=7s outcome=success\n");
    fs::write(health.join("heartbeat.log"), &original).unwrap();

    let skipped = run(&journal, &["heartbeat"]);
    assert!(skipped.status.success());
    assert!(!health.join("steward.log").exists());
    assert_eq!(
        fs::read_to_string(health.join("heartbeat.log")).unwrap(),
        original
    );

    let forced = run(&journal, &["heartbeat", "--force"]);
    assert!(forced.status.success(), "{forced:?}");
    assert!(health.join("steward.log").exists());
    assert_eq!(
        fs::read_to_string(health.join("heartbeat.log"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[test]
fn heartbeat_removes_stale_pid_and_prunes_old_or_malformed_steward_rows() {
    let journal = TempDir::new().unwrap();
    let health = journal.path().join("health");
    fs::create_dir_all(&health).unwrap();
    fs::write(health.join("heartbeat.pid"), "2147483647").unwrap();
    let old = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 31 * 86_400_000;
    fs::write(
        health.join("steward.log"),
        format!("{{\"event\":\"old\",\"ts\":{old}}}\nnot-json\n"),
    )
    .unwrap();

    let output = run(&journal, &["heartbeat", "--force"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!health.join("heartbeat.pid").exists());
    let rows = fs::read_to_string(health.join("steward.log")).unwrap();
    assert_eq!(rows.lines().count(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(rows.trim()).unwrap()["event"],
        "pass"
    );
}

#[test]
fn live_pid_guard_skips_pass_and_reference_cleanup_removes_pid_file() {
    let journal = TempDir::new().unwrap();
    let health = journal.path().join("health");
    fs::create_dir_all(&health).unwrap();
    fs::write(health.join("heartbeat.pid"), std::process::id().to_string()).unwrap();

    let output = run(&journal, &["heartbeat", "--force"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!health.join("steward.log").exists());
    assert!(!health.join("heartbeat.log").exists());
    assert!(!health.join("heartbeat.pid").exists());
}

#[test]
fn invalid_utf8_pid_fails_and_reference_cleanup_removes_it() {
    let journal = TempDir::new().unwrap();
    let health = journal.path().join("health");
    fs::create_dir_all(&health).unwrap();
    fs::write(health.join("heartbeat.pid"), [0xff]).unwrap();

    let output = run(&journal, &["heartbeat", "--force"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("journal heartbeat:"));
    assert!(!health.join("heartbeat.pid").exists());
    assert!(!health.join("steward.log").exists());
}

#[test]
fn invalid_utf8_heartbeat_log_fails_before_pid_or_pass_mutation() {
    let journal = TempDir::new().unwrap();
    let health = journal.path().join("health");
    fs::create_dir_all(&health).unwrap();
    fs::write(health.join("heartbeat.log"), [0xff]).unwrap();

    let output = run(&journal, &["heartbeat"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("journal heartbeat:"));
    assert_eq!(fs::read(health.join("heartbeat.log")).unwrap(), [0xff]);
    assert!(!health.join("heartbeat.pid").exists());
    assert!(!health.join("steward.log").exists());
}
