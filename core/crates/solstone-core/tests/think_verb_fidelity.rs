// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use tempfile::TempDir;

fn command(args: &[&str], journal: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(args)
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1");
    command
}

#[test]
fn think_usage_is_owner_facing_and_refusals_are_detailed() {
    let journal = TempDir::new().unwrap();
    let help = command(&["think", "--help"], &journal).output().unwrap();
    assert_eq!(help.status.code(), Some(0));
    assert_eq!(help.stderr, b"");
    assert_eq!(
        help.stdout,
        b"usage: journal think [-h] [--day DAY] [--segment SEGMENT] [--refresh] [--from-scratch] [--segments] [--facet NAME] [--activity ID] [--stream STREAM] [--flush] [-j N] [--no-timeout] [--segment-workers N] [--no-activity-prompts] [--skip-talents SKIP_TALENTS] [--live] [--updated] [--weekly] [--cadence] [--dry-run] [-v] [-d]\n"
    );
    let output = command(&["think", "--facet", "work"], &journal)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("usage: journal think"));
    assert!(stderr.ends_with("journal think: error: --facet requires --activity\n"));
}

#[test]
fn think_preserves_all_thirteen_semantic_refusal_messages() {
    let cases: &[(&[&str], &str)] = &[
        (
            &[
                "--updated",
                "--day",
                "20260813",
                "--segment",
                "x",
                "--facet",
                "f",
                "--activity",
                "a",
                "--flush",
                "--segments",
                "--cadence",
            ],
            "--updated is incompatible with --day, --segment, --facet, --activity, --flush, --segments, --cadence",
        ),
        (&["--facet", "f"], "--facet requires --activity"),
        (
            &["--activity", "a", "--day", "20260813"],
            "--activity requires --facet",
        ),
        (
            &["--activity", "a", "--facet", "f"],
            "--activity requires --day",
        ),
        (
            &[
                "--no-activity-prompts",
                "--activity",
                "a",
                "--facet",
                "f",
                "--day",
                "20260813",
            ],
            "--no-activity-prompts cannot be combined with --activity",
        ),
        (
            &["--segment-workers", "0"],
            "--segment-workers must be between 1 and 32",
        ),
        (
            &[
                "--activity",
                "a",
                "--facet",
                "f",
                "--day",
                "20260813",
                "--segment",
                "x",
            ],
            "--activity is incompatible with --segment, --segments, and --flush",
        ),
        (&["--flush"], "--flush requires --segment"),
        (
            &["--flush", "--segment", "x", "--segments"],
            "--flush is incompatible with --segments and --refresh",
        ),
        (
            &["--segments", "--segment", "x"],
            "--segments is incompatible with --segment and --facet",
        ),
        (
            &["--weekly", "--segment", "x"],
            "--weekly is incompatible with --segment, --segments, --activity, and --flush",
        ),
        (
            &["--cadence", "--segment", "x"],
            "--cadence is incompatible with --segment, --segments, --activity, --flush, and --weekly",
        ),
        (
            &["--segments", "--jobs", "0", "--segment-workers", "2"],
            "--jobs 0 is incompatible with multi-worker --segments; set --jobs to a positive bound or --segment-workers 1",
        ),
    ];
    for (args, message) in cases {
        let journal = TempDir::new().unwrap();
        let mut argv = vec!["think"];
        argv.extend_from_slice(args);
        let output = command(&argv, &journal).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .ends_with(&format!("journal think: error: {message}\n")),
            "{args:?}"
        );
    }
}

#[test]
fn think_creates_the_day_before_refusal_and_names_malformed_days_cleanly() {
    let journal = TempDir::new().unwrap();
    let refusal = command(&["think", "--day", "20260813", "--facet", "work"], &journal)
        .output()
        .unwrap();
    assert_eq!(refusal.status.code(), Some(2));
    assert!(journal.path().join("chronicle/20260813").is_dir());
    let malformed = command(&["think", "--day", "bad"], &journal)
        .output()
        .unwrap();
    assert_eq!(malformed.status.code(), Some(1));
    assert_eq!(malformed.stderr, b"journal think: day must be YYYYMMDD\n");
}

#[test]
fn every_supported_argument_spelling_reaches_think_not_top_level_usage() {
    let cases: &[&[&str]] = &[
        &["--day", "20260813"],
        &["--segment", "120000_300"],
        &["--refresh"],
        &["--from-scratch"],
        &["--segments"],
        &["--facet", "work"],
        &["--activity", "a"],
        &["--stream", "stream"],
        &["--flush"],
        &["-j", "2"],
        &["--jobs", "2"],
        &["--no-timeout"],
        &["--segment-workers", "1"],
        &["--no-activity-prompts"],
        &["--skip-talents", "sense"],
        &["--live"],
        &["--updated"],
        &["--weekly"],
        &["--cadence"],
        &["--dry-run"],
        &["-v"],
        &["--verbose"],
        &["-d"],
        &["--debug"],
    ];
    for args in cases {
        let journal = TempDir::new().unwrap();
        let mut argv = vec!["think"];
        argv.extend_from_slice(args);
        let output = command(&argv, &journal).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.code() != Some(64), "{args:?}: {stderr}");
        assert!(
            !stderr.contains("solstone-core --version"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn updated_ignores_invalid_segment_workers_and_run_modes_are_unavailable() {
    let journal = TempDir::new().unwrap();
    fs::create_dir_all(journal.path().join("chronicle/20260813/health")).unwrap();
    fs::create_dir_all(journal.path().join("chronicle/20260814/health")).unwrap();
    fs::write(
        journal
            .path()
            .join("chronicle/20260813/health/stream.updated"),
        b"",
    )
    .unwrap();
    fs::write(
        journal
            .path()
            .join("chronicle/20260814/health/stream.updated"),
        b"",
    )
    .unwrap();
    let updated = command(&["think", "--updated", "--segment-workers", "99"], &journal)
        .output()
        .unwrap();
    assert_eq!(updated.status.code(), Some(0));
    assert!(updated.stderr.is_empty());
    assert_eq!(updated.stdout, b"20260813\n");
    let unavailable = command(&["think", "--dry-run"], &journal).output().unwrap();
    assert_eq!(unavailable.status.code(), Some(69));
}

#[test]
fn negative_jobs_reaches_the_unavailable_run_mode() {
    let journal = TempDir::new().unwrap();
    let output = command(&["think", "--jobs", "-1"], &journal)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(69));
}

#[test]
fn think_never_reaches_interpreters_while_a_python_path_reaches_the_poison() {
    let temp = TempDir::new().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    for name in ["python", "python3"] {
        let shim = bin.join(name);
        fs::write(
            &shim,
            format!("#!/bin/sh\nprintf '%s' '{name}' > \"$POISON_DIR/{name}\"\nexit 97\n"),
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["think", "--dry-run"])
        .env("PATH", &bin)
        .env("POISON_DIR", temp.path())
        .env("SOLSTONE_JOURNAL", temp.path().join("journal"))
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(69));
    assert!(!temp.path().join("python").exists());
    assert!(!temp.path().join("python3").exists());
    let positive = Command::new("python3")
        .args(["-m", "solstone.think.thinking", "--dry-run"])
        .env("PATH", &bin)
        .env("POISON_DIR", temp.path())
        .output()
        .unwrap();
    assert_eq!(positive.status.code(), Some(97));
    assert!(temp.path().join("python3").exists());
}
