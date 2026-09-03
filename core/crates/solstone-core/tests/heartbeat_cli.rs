// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};

use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone};
use serde_json::{Value, json};
use solstone_core_journal_io::{
    JournalRoot, MalformedPolicy,
    operational_log::{OplogCatalogEntry, OplogFormat, catalog_oplogs, create_oplog_at},
    readers::read_jsonl_with_report,
};
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

fn append_record(
    journal: &Path,
    source: &str,
    run: &str,
    opened: DateTime<FixedOffset>,
    record: Value,
) {
    let mut writer = create_oplog_at(
        JournalRoot::open(journal).unwrap(),
        source,
        run,
        OplogFormat::Jsonl,
        opened,
    )
    .unwrap();
    serde_json::to_writer(&mut writer, &record).unwrap();
    writer.write_all(b"\n").unwrap();
    writer.flush().unwrap();
}

fn pass_outcome(ts: i64) -> Value {
    json!({
        "duration_seconds": 7,
        "event": "pass.outcome",
        "outcome": "success",
        "ts": ts,
    })
}

fn matching_entries(
    journal: &Path,
    days: &[NaiveDate],
    source: &str,
    run: &str,
) -> Vec<OplogCatalogEntry> {
    let snapshot = catalog_oplogs(JournalRoot::open(journal).unwrap(), days).unwrap();
    snapshot
        .entries()
        .iter()
        .filter(|entry| {
            entry.name().source().display_slug() == source
                && entry.name().run().display_slug() == run
                && entry.name().format() == OplogFormat::Jsonl
        })
        .cloned()
        .collect()
}

fn entry_records(journal: &Path, entry: &OplogCatalogEntry) -> Vec<Value> {
    read_jsonl_with_report(
        journal
            .join("chronicle")
            .join(entry.day())
            .join("health")
            .join(entry.leaf()),
        Vec::new(),
        MalformedPolicy::Raise,
    )
    .unwrap()
    .records
    .into_iter()
    .map(|record| record.value)
    .collect()
}

fn at_local_day(now: DateTime<FixedOffset>, day: NaiveDate) -> DateTime<FixedOffset> {
    now.offset()
        .from_local_datetime(&day.and_time(now.time()))
        .single()
        .unwrap()
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
fn heartbeat_appends_pass_and_success_records_to_one_oplog_then_removes_pid() {
    let journal = TempDir::new().unwrap();
    let output = run(&journal, &["heartbeat"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());

    let health = journal.path().join("health");
    assert!(!health.join("heartbeat.pid").exists());
    assert!(!health.join("heartbeat.log").exists());
    assert!(!health.join("steward.log").exists());

    let today = Local::now().date_naive();
    let entries = matching_entries(journal.path(), &[today], "heartbeat", "pass");
    assert_eq!(entries.len(), 1);
    let records = entry_records(journal.path(), &entries[0]);
    assert_eq!(records.len(), 2);
    let pass = records
        .iter()
        .find(|record| record["event"] == "pass")
        .unwrap();
    assert_eq!(pass["fired"], json!([]));
    assert_eq!(pass["escalated_targets"], json!([]));
    assert_eq!(pass["data_source_errors"], json!([]));
    assert!(pass["ts"].as_i64().is_some());
    let outcome = records
        .iter()
        .find(|record| record["event"] == "pass.outcome")
        .unwrap();
    assert_eq!(outcome["outcome"], "success");
    assert!(outcome["duration_seconds"].as_u64().is_some());
    assert!(outcome["ts"].as_i64().is_some());
}

#[test]
fn recent_success_skips_and_force_creates_a_new_heartbeat_oplog() {
    let journal = TempDir::new().unwrap();
    let now = Local::now().fixed_offset();
    append_record(
        journal.path(),
        "heartbeat",
        "pass",
        now,
        pass_outcome(now.timestamp_millis()),
    );

    let skipped = run(&journal, &["heartbeat"]);
    assert!(skipped.status.success(), "{skipped:?}");
    let today = now.date_naive();
    assert_eq!(
        matching_entries(journal.path(), &[today], "heartbeat", "pass").len(),
        1
    );

    let forced = run(&journal, &["heartbeat", "--force"]);
    assert!(forced.status.success(), "{forced:?}");
    let entries = matching_entries(journal.path(), &[today], "heartbeat", "pass");
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries
            .iter()
            .flat_map(|entry| entry_records(journal.path(), entry))
            .filter(|record| record["event"] == "pass")
            .count(),
        1
    );
}

#[test]
fn prior_day_success_within_twelve_hours_suppresses_the_after_midnight_check() {
    let journal = TempDir::new().unwrap();
    let now = Local::now().fixed_offset();
    let previous = now.date_naive().pred_opt().unwrap();
    append_record(
        journal.path(),
        "heartbeat",
        "pass",
        at_local_day(now, previous),
        pass_outcome(now.timestamp_millis() - 11 * 3_600_000),
    );

    let output = run(&journal, &["heartbeat"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        matching_entries(
            journal.path(),
            &[previous, now.date_naive()],
            "heartbeat",
            "pass"
        )
        .len(),
        1
    );
}

#[test]
fn old_success_does_not_suppress_a_heartbeat_pass() {
    let journal = TempDir::new().unwrap();
    let now = Local::now().fixed_offset();
    append_record(
        journal.path(),
        "heartbeat",
        "pass",
        now,
        pass_outcome(now.timestamp_millis() - 12 * 3_600_000 - 1),
    );

    let output = run(&journal, &["heartbeat"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        matching_entries(journal.path(), &[now.date_naive()], "heartbeat", "pass").len(),
        2
    );
}

#[test]
fn malformed_heartbeat_record_does_not_suppress_a_pass() {
    let journal = TempDir::new().unwrap();
    let now = Local::now().fixed_offset();
    let mut writer = create_oplog_at(
        JournalRoot::open(journal.path()).unwrap(),
        "heartbeat",
        "pass",
        OplogFormat::Jsonl,
        now,
    )
    .unwrap();
    writer.write_all(b"not-json\n").unwrap();
    writer.flush().unwrap();

    let output = run(&journal, &["heartbeat"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        matching_entries(journal.path(), &[now.date_naive()], "heartbeat", "pass").len(),
        2
    );
}

#[test]
fn wrong_source_record_does_not_suppress_a_heartbeat_pass() {
    let journal = TempDir::new().unwrap();
    let now = Local::now().fixed_offset();
    append_record(
        journal.path(),
        "steward",
        "pre-hook",
        now,
        pass_outcome(now.timestamp_millis()),
    );

    let output = run(&journal, &["heartbeat"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        matching_entries(journal.path(), &[now.date_naive()], "heartbeat", "pass").len(),
        1
    );
}

#[test]
fn heartbeat_removes_stale_pid_before_writing_its_oplog() {
    let journal = TempDir::new().unwrap();
    let health = journal.path().join("health");
    fs::create_dir_all(&health).unwrap();
    fs::write(health.join("heartbeat.pid"), "2147483647").unwrap();

    let output = run(&journal, &["heartbeat", "--force"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!health.join("heartbeat.pid").exists());
    let today = Local::now().date_naive();
    assert_eq!(
        matching_entries(journal.path(), &[today], "heartbeat", "pass").len(),
        1
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
    assert!(!health.join("heartbeat.pid").exists());
    assert!(
        matching_entries(
            journal.path(),
            &[Local::now().date_naive()],
            "heartbeat",
            "pass"
        )
        .is_empty()
    );
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
    assert!(
        matching_entries(
            journal.path(),
            &[Local::now().date_naive()],
            "heartbeat",
            "pass"
        )
        .is_empty()
    );
}

#[test]
fn heartbeat_oplog_creation_failure_is_fatal() {
    let journal = TempDir::new().unwrap();
    let today = Local::now().format("%Y%m%d").to_string();
    fs::create_dir_all(journal.path().join("chronicle")).unwrap();
    fs::write(
        journal.path().join("chronicle").join(today),
        b"not a directory",
    )
    .unwrap();

    let output = run(&journal, &["heartbeat", "--force"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to create heartbeat oplog"));
    assert!(!journal.path().join("health/heartbeat.pid").exists());
}

#[test]
fn concurrent_heartbeat_writers_create_distinct_intact_oplogs() {
    let journal = TempDir::new().unwrap();
    let root = Arc::new(journal.path().to_path_buf());
    let opened = Local::now().fixed_offset();
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|worker| {
            let root = Arc::clone(&root);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                append_record(
                    &root,
                    "heartbeat",
                    "pass",
                    opened,
                    json!({"event": "concurrent", "worker": worker}),
                );
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }

    let entries = matching_entries(journal.path(), &[opened.date_naive()], "heartbeat", "pass");
    assert_eq!(entries.len(), 2);
    let mut workers = entries
        .iter()
        .flat_map(|entry| entry_records(journal.path(), entry))
        .map(|record| record["worker"].as_i64().unwrap())
        .collect::<Vec<_>>();
    workers.sort_unstable();
    assert_eq!(workers, vec![0, 1]);
}
