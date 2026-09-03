// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use chrono::Local;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, OplogWriter, create_oplog_at},
};

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");

struct TestJournal(tempfile::TempDir);

impl TestJournal {
    fn new() -> Self {
        Self(tempfile::tempdir().expect("create journal"))
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn service_writer(&self) -> OplogWriter {
        create_oplog_at(
            JournalRoot::open(self.path()).expect("open journal"),
            "service",
            "supervisor",
            OplogFormat::Log,
            Local::now().fixed_offset(),
        )
        .expect("create service oplog")
    }
}

fn run(journal: &Path, arguments: &[&str]) -> Output {
    Command::new(BINARY)
        .args(arguments)
        .env("SOLSTONE_JOURNAL", journal)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run core")
}

fn wait_until(deadline: Instant, mut condition: impl FnMut() -> bool) {
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("condition did not become true before the deadline");
}

fn service_output(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_default()
}

#[test]
fn no_service_oplogs_reports_the_canonical_absence_message() {
    let journal = TestJournal::new();
    let output = run(journal.path(), &["service", "logs"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"=== service logs === (no oplog leaves)\n");
    assert!(output.stderr.is_empty());
    assert!(!journal.path().join("health/service.log").exists());
}

#[test]
fn one_shot_merges_interleaved_stdout_and_stderr_from_service_segments() {
    let journal = TestJournal::new();
    let mut first = journal.service_writer();
    first.write_all(b"stdout: started\n").unwrap();
    let mut second = journal.service_writer();
    second.write_all(b"stderr: listening\n").unwrap();
    let mut unrelated = create_oplog_at(
        JournalRoot::open(journal.path()).unwrap(),
        "heartbeat",
        "pass",
        OplogFormat::Log,
        Local::now().fixed_offset(),
    )
    .unwrap();
    unrelated.write_all(b"hidden\n").unwrap();

    let output = run(journal.path(), &["service", "logs"]);

    assert_eq!(output.status.code(), Some(0));
    let rendered = String::from_utf8(output.stdout).unwrap();
    assert!(rendered.starts_with("=== service logs ===\n"));
    assert!(rendered.contains("stdout: started\n"));
    assert!(rendered.contains("stderr: listening\n"));
    assert!(!rendered.contains("hidden"));
    assert!(output.stderr.is_empty());
}

#[test]
fn follow_keeps_the_core_process_and_handoffs_without_a_gap_or_duplicate() {
    let journal = TestJournal::new();
    let mut writer = journal.service_writer();
    writer.write_all(b"before\n").unwrap();
    writer.flush().unwrap();

    let capture = tempfile::tempdir().unwrap();
    let stdout_path = capture.path().join("stdout");
    let stderr_path = capture.path().join("stderr");
    let stdout = fs::File::create(&stdout_path).unwrap();
    let stderr = fs::File::create(&stderr_path).unwrap();
    let mut child = Command::new(BINARY)
        .args(["service", "logs", "--follow"])
        .env("SOLSTONE_JOURNAL", journal.path())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("start in-process follower");
    let deadline = Instant::now() + Duration::from_secs(5);
    wait_until(deadline, || {
        let output = service_output(&stdout_path);
        output
            .windows(b"before\n".len())
            .any(|row| row == b"before\n")
    });
    writer.write_all(b"after\n").unwrap();
    writer.flush().unwrap();
    wait_until(deadline, || {
        let output = service_output(&stdout_path);
        output
            .windows(b"after\n".len())
            .any(|row| row == b"after\n")
    });
    let pid = Pid::from_raw(i32::try_from(child.id()).unwrap());
    kill(pid, Signal::SIGTERM).unwrap();
    let status = child.wait().unwrap();

    assert_eq!(status.code(), Some(0));
    let output = service_output(&stdout_path);
    assert_eq!(
        output
            .windows(b"before\n".len())
            .filter(|row| *row == b"before\n")
            .count(),
        1
    );
    assert_eq!(
        output
            .windows(b"after\n".len())
            .filter(|row| *row == b"after\n")
            .count(),
        1
    );
    assert!(fs::read(stderr_path).unwrap().is_empty());
}

#[test]
fn follow_renders_unterminated_non_utf8_service_output() {
    let journal = TestJournal::new();
    let mut writer = journal.service_writer();
    writer.write_all(b"before\n").unwrap();
    writer.flush().unwrap();

    let capture = tempfile::tempdir().unwrap();
    let stdout_path = capture.path().join("stdout");
    let stderr_path = capture.path().join("stderr");
    let stdout = fs::File::create(&stdout_path).unwrap();
    let stderr = fs::File::create(&stderr_path).unwrap();
    let mut child = Command::new(BINARY)
        .args(["service", "logs", "--follow"])
        .env("SOLSTONE_JOURNAL", journal.path())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("start raw service follower");
    let deadline = Instant::now() + Duration::from_secs(5);
    wait_until(deadline, || {
        service_output(&stdout_path)
            .windows(7)
            .any(|row| row == b"before\n")
    });

    writer.write_all(b"progress:\xff").unwrap();
    writer.flush().unwrap();
    let expected = "progress:\u{fffd}".as_bytes();
    wait_until(deadline, || {
        service_output(&stdout_path)
            .windows(expected.len())
            .any(|row| row == expected)
    });
    let pid = Pid::from_raw(i32::try_from(child.id()).unwrap());
    kill(pid, Signal::SIGTERM).unwrap();
    let status = child.wait().unwrap();

    assert_eq!(status.code(), Some(0));
    assert!(fs::read(stderr_path).unwrap().is_empty());
}

#[test]
fn multiple_service_oplog_leaves_form_a_continuous_one_shot_stream() {
    let journal = TestJournal::new();
    let mut before_rollover = journal.service_writer();
    before_rollover.write_all(b"before rollover\n").unwrap();
    let mut after_rollover = journal.service_writer();
    after_rollover.write_all(b"after rollover\n").unwrap();

    let output = run(journal.path(), &["service", "logs"]);

    assert_eq!(output.status.code(), Some(0));
    let rendered = String::from_utf8(output.stdout).unwrap();
    assert!(rendered.contains("before rollover"));
    assert!(rendered.contains("after rollover"));
}
