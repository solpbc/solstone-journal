// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use chrono::Local;
use solstone_core_cli::HEALTH_LOGS_HELP;

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");

struct TestJournal(tempfile::TempDir);

impl TestJournal {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
    fn day_health(&self) -> PathBuf {
        self.path()
            .join("chronicle")
            .join(Local::now().format("%Y%m%d").to_string())
            .join("health")
    }
    fn log(&self, name: &str, content: &str) {
        let health = self.day_health();
        fs::create_dir_all(&health).unwrap();
        let target = self.path().join(format!("target-{name}"));
        fs::write(&target, content).unwrap();
        symlink(target, health.join(name)).unwrap();
    }
}

fn run(journal: &TestJournal, args: &[&str]) -> Output {
    run_path(journal.path().as_os_str().to_owned(), args)
}

fn run_path(journal: OsString, args: &[&str]) -> Output {
    Command::new(BINARY)
        .args(args)
        .env("SOLSTONE_JOURNAL", journal)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn logs() -> String {
    let day = Local::now().format("%Y-%m-%d").to_string();
    [
        format!("{day} 09:00:00 [observer:stdout] old error"),
        format!("{day} 10:00:00 [observer:stdout] new error"),
        format!("{day} 10:01:00 [other:stdout] new error"),
        format!("{day} 10:02:00 [observer:stdout] plain"),
    ]
    .join("\n")
        + "\n"
}

#[test]
fn one_shot_filters_and_count_reach_the_real_command_body() {
    let journal = TestJournal::new();
    let content = logs();
    let rows = content.lines().collect::<Vec<_>>();
    journal.log("service.log", &content);
    for (args, expected) in [
        (
            ["health", "logs", "-c", "1"].as_slice(),
            format!("{}\n", rows[3]),
        ),
        (
            ["health", "logs", "--since", "10:00"].as_slice(),
            format!("{}\n{}\n{}\n", rows[1], rows[2], rows[3]),
        ),
        (
            ["health", "logs", "--service", "other"].as_slice(),
            format!("{}\n", rows[2]),
        ),
        (
            ["health", "logs", "--grep", "plain"].as_slice(),
            format!("{}\n", rows[3]),
        ),
    ] {
        let output = run(&journal, args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert_eq!(output.stdout, expected.as_bytes(), "{args:?}");
    }
}

#[test]
fn detached_negative_number_prefixes_remain_service_values() {
    let journal = TestJournal::new();
    for value in ["-1e2", "-1.", "-0x1", "-.5x", "-.١x"] {
        let output = run(&journal, &["health", "logs", "--service", value]);
        assert_eq!(output.status.code(), Some(0), "{value:?}");
        assert!(output.stderr.is_empty(), "{value:?}");
    }
    let rejected = run(&journal, &["health", "logs", "--service", "-.x"]);
    assert_eq!(rejected.status.code(), Some(2));
}

#[test]
fn invalid_values_and_follow_missing_directory_keep_their_polarity() {
    let journal = TestJournal::new();
    for args in [
        ["health", "logs", "--grep", "("].as_slice(),
        ["health", "logs", "-c", "abc"].as_slice(),
        ["health", "logs", "-c", ""].as_slice(),
        ["health", "logs", "-cv"].as_slice(),
    ] {
        let output = run(&journal, args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("health logs: error:"));
    }
    let output = run(&journal, &["health", "logs", "-f"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"No health directory found.\n");
}

#[test]
fn help_and_bare_health_remain_reachable() {
    let journal = TestJournal::new();
    let help = run(&journal, &["health", "logs", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert_eq!(help.stdout, HEALTH_LOGS_HELP.as_bytes());
    let bare = run(&journal, &["health", "-v", "--debug"]);
    assert_ne!(bare.status.code(), Some(2));
}

#[test]
fn invalid_values_before_help_win_and_help_first_stops_parsing() {
    let journal = TestJournal::new();
    for args in [
        ["health", "logs", "-c", "bad", "-c", "5", "--help"].as_slice(),
        [
            "health", "logs", "--since", "bad", "--since", "1h", "--help",
        ]
        .as_slice(),
        ["health", "logs", "--grep", "(", "--grep", "ok", "--help"].as_slice(),
    ] {
        let output = run(&journal, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(output.stderr.starts_with(b"journal health logs: error:"));
    }

    let output = run(
        &journal,
        &["health", "logs", "--help", "-c", "bad", "--grep", "("],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, HEALTH_LOGS_HELP.as_bytes());
    assert!(output.stderr.is_empty());
}

#[test]
fn value_validation_and_help_precede_journal_resolution() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".config/solstone/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, b"\xff").unwrap();
    let run = |args: &[&str]| {
        Command::new(BINARY)
            .args(args)
            .env_remove("SOLSTONE_JOURNAL")
            .env("HOME", home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    };

    let journal_failure = run(&["health", "logs", "-c", "5"]);
    assert_eq!(journal_failure.status.code(), Some(75));
    assert!(journal_failure.stderr.starts_with(b"journal-path failed:"));

    let invalid = run(&["health", "logs", "-c", "bad", "--help"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert!(invalid.stderr.starts_with(b"journal health logs: error:"));

    for args in [
        ["health", "logs", "-c", "bad", "--bogus"].as_slice(),
        ["health", "logs", "--bogus", "-c", "bad"].as_slice(),
        ["health", "logs", "-c", "bad", "--service"].as_slice(),
        ["health", "logs", "-c", "bad", "--s", "x"].as_slice(),
    ] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("invalid count"),
            "{args:?}: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let missing_first = run(&["health", "logs", "--service", "-f", "-c", "bad"]);
    assert_eq!(missing_first.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&missing_first.stderr).contains("invalid count"));
    assert!(String::from_utf8_lossy(&missing_first.stderr).contains("invalid arguments"));

    let help = run(&["health", "logs", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert_eq!(help.stdout, HEALTH_LOGS_HELP.as_bytes());
    assert!(help.stderr.is_empty());
}

#[test]
fn path_diagnostics_escape_invalid_bytes_and_backslashes_once() {
    let outer = tempfile::tempdir().unwrap();
    let journal = outer
        .path()
        .join(OsString::from_vec(b"journal-\\-\xff".to_vec()));
    let day_health = journal
        .join("chronicle")
        .join(Local::now().format("%Y%m%d").to_string())
        .join("health");
    fs::create_dir_all(&day_health).unwrap();
    let name = OsString::from_vec(b"bad-\\-\xff.log".to_vec());
    let target = journal.join("invalid-target");
    fs::write(&target, b"\xff").unwrap();
    symlink(&target, day_health.join(&name)).unwrap();

    let output = run_path(
        journal.as_os_str().to_owned(),
        &["health", "logs", "-c", "5"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr,
        format!(
            "health logs: {}/journal-\\\\-\\xff/chronicle/{}/health/bad-\\\\-\\xff.log: invalid utf-8 sequence of 1 bytes from index 0\n",
            outer.path().display(),
            Local::now().format("%Y%m%d")
        )
    );
}

#[test]
fn follow_warning_keeps_invalid_path_identity_once() {
    let outer = tempfile::tempdir().unwrap();
    let journal = outer
        .path()
        .join(OsString::from_vec(b"journal-\\-\xff".to_vec()));
    let health = journal.join("health");
    fs::create_dir_all(&health).unwrap();
    let name = OsString::from_vec(b"broken-\\-\xff.log".to_vec());
    symlink("missing", health.join(name)).unwrap();

    let output = run_path(journal.as_os_str().to_owned(), &["health", "logs", "-f"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr,
        format!(
            "health logs: initial-open failed for {}/journal-\\\\-\\xff/health/broken-\\\\-\\xff.log\nNo log files found.\n",
            outer.path().display()
        )
    );
}
