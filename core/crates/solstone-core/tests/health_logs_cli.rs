// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
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
    Command::new(BINARY)
        .args(args)
        .env("SOLSTONE_JOURNAL", journal.path())
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
fn invalid_values_and_follow_missing_directory_keep_their_polarity() {
    let journal = TestJournal::new();
    for args in [
        ["health", "logs", "--grep", "("].as_slice(),
        ["health", "logs", "-c", "abc"].as_slice(),
        ["health", "logs", "-c", ""].as_slice(),
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
