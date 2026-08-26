// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! End-to-end log retention through the real executor binary.

#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "integration fixture setup and teardown exercise the binary boundary"
)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct Journal {
    root: PathBuf,
}

impl Journal {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "retention-cli-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("journal root");
        Self { root }
    }

    fn file(&self, rel: &str, contents: &[u8]) -> PathBuf {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("parents");
        fs::write(&path, contents).expect("file");
        path
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let path = self.root.join(rel);
        fs::create_dir_all(&path).expect("directory");
        path
    }

    fn old_directory(&self, rel: &str) {
        let path = self.dir(rel);
        let file = fs::File::open(&path).expect("directory handle");
        file.set_times(fs::FileTimes::new().set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800),
        ))
        .expect("backdate directory");
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run(journal: &Journal, execute: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-retention"));
    command.args([
        "prune-logs",
        "--journal",
        journal.root.to_str().expect("journal utf-8"),
        "--today",
        "2026-08-05",
        "--days",
        "7",
    ]);
    if execute {
        command.args(["--execute", "true"]);
    }
    command.output().expect("executor runs")
}

fn populate_all_classes(journal: &Journal) {
    journal.file("chronicle/20260101/health/observer.log", b"old health");
    journal.file("talents/scribe/1767225600000.jsonl", b"old run\n");
    journal.file("talents/20260101.jsonl", b"{\"ts\":1767225600000}\n");
    journal.old_directory(".cache/cogitate-history/old-session");
    journal.file("tokens/20260101.jsonl", b"old token\n");
    journal.file("health/local-inference/20260101.jsonl", b"old telemetry\n");
    journal.file("awareness/20260101.jsonl", b"old awareness\n");
    journal.file("config/actions/20260101.jsonl", b"old action\n");
    journal.file("facets/work/logs/20260101.jsonl", b"old facet\n");
    journal.file("health/pruning-runs/20260101.jsonl", b"old pruning run\n");
    journal.file(
        "task_log.txt",
        b"1767225600\told task\n1786492800\trecent task\nnot a dated task\n",
    );
    journal.file(
        "health/retention.log",
        b"{\"timestamp\":\"2026-01-01T00:00:00\"}\n\
          {\"timestamp\":\"2026-08-04T00:00:00\"}\nnot json\n",
    );
}

#[test]
fn prune_logs_removes_every_class_compacts_both_logs_and_keeps_negative_twins() {
    let journal = Journal::new("all-classes");
    populate_all_classes(&journal);
    journal.file("tokens/20260804.jsonl", b"recent token\n");
    journal.file("talents/scribe/1767225600000_active.jsonl", b"live run\n");
    journal.file("talents/not-a-date.jsonl", b"undateable\n");
    journal.file("talents/20260102.jsonl", b"{\"ts\":1786492800000}\n");

    let preview = run(&journal, false);
    assert!(preview.status.success(), "{preview:?}");
    let preview_receipt: serde_json::Value =
        serde_json::from_slice(&preview.stdout).expect("preview JSON receipt");
    assert_eq!(
        preview_receipt["plan"]["compactions"]["root_task_log"]["exists"],
        true
    );
    assert_eq!(
        preview_receipt["plan"]["compactions"]["root_task_log"]["planned"],
        true
    );
    assert_eq!(
        preview_receipt["plan"]["compactions"]["retention_log"]["planned"],
        true
    );

    let output = run(&journal, true);
    assert!(output.status.success(), "{output:?}");
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON receipt");
    assert_eq!(
        receipt["detail"]["plan"]["by_class"]
            .as_object()
            .map(|map| map.len()),
        Some(10)
    );
    assert_eq!(
        receipt["detail"]["plan"]["compactions"]["root_task_log"]["rewritten"],
        true
    );
    assert_eq!(
        receipt["detail"]["plan"]["compactions"]["retention_log"]["rewritten"],
        true
    );
    assert_eq!(
        receipt["detail"]["plan"]["by_class"]["talent_run_logs"]["skipped"],
        1
    );
    assert_eq!(
        receipt["detail"]["plan"]["by_class"]["talent_day_index"]["skipped"],
        2
    );

    for rel in [
        "chronicle/20260101/health/observer.log",
        "talents/scribe/1767225600000.jsonl",
        "talents/20260101.jsonl",
        ".cache/cogitate-history/old-session",
        "tokens/20260101.jsonl",
        "health/local-inference/20260101.jsonl",
        "awareness/20260101.jsonl",
        "config/actions/20260101.jsonl",
        "facets/work/logs/20260101.jsonl",
        "health/pruning-runs/20260101.jsonl",
    ] {
        assert!(!journal.root.join(rel).exists(), "{rel} was not removed");
    }
    for rel in [
        "tokens/20260804.jsonl",
        "talents/scribe/1767225600000_active.jsonl",
        "talents/not-a-date.jsonl",
        "talents/20260102.jsonl",
    ] {
        assert!(journal.root.join(rel).exists(), "{rel} was removed");
    }
    assert_eq!(
        fs::read(journal.root.join("task_log.txt")).expect("task log"),
        b"1786492800\trecent task\nnot a dated task\n"
    );
    assert_eq!(
        fs::read(journal.root.join("health/retention.log")).expect("retention log"),
        b"{\"timestamp\":\"2026-08-04T00:00:00\"}\nnot json\n"
    );
}

#[test]
fn a_refusal_is_reported_without_aborting_other_log_removals() {
    use std::os::unix::fs::PermissionsExt;

    let journal = Journal::new("refusal");
    let blocked = journal.file("tokens/20260101.jsonl", b"blocked\n");
    let removable = journal.file("awareness/20260101.jsonl", b"removable\n");
    let parent = blocked.parent().expect("tokens parent");
    fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).expect("block removal");

    let output = run(&journal, true);
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).expect("restore removal");
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON receipt");
    assert!(
        receipt["outcome"]["targets"]
            .as_array()
            .expect("outcome targets")
            .iter()
            .flat_map(|target| target["not_removed"].as_array().into_iter().flatten())
            .any(|entry| entry["entry"] == "tokens/20260101.jsonl")
    );
    assert!(blocked.exists(), "the refused entry survives");
    assert!(!removable.exists(), "a sibling removal still completes");
}
