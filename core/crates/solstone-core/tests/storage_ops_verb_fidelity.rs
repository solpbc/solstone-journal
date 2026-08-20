// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fidelity coverage for the storage-operation bodies served by solstone-core.

#![cfg(unix)]

use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const FIXTURE: &str = include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");
const VERBS: &[&str] = &[
    "streams",
    "segment",
    "journal-stats",
    "reprocess",
    "backfill-processing-records",
];
const SUPERVISOR_MESSAGE: &str = "journal isn't running. start it with 'journal up' and retry.\n";
const DUMMY_JOURNAL: &str = "/nonexistent-storage-ops-dummy-journal";

fn block(name: &str) -> &str {
    let header = format!("=== {name}\n");
    let start = FIXTURE.find(&header).expect("fixture block") + header.len();
    let rest = &FIXTURE[start..];
    &rest[..rest.find("\n=== ").unwrap_or(rest.len())]
}

fn command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command.args(args);
    command
}

fn run(args: &[&str]) -> Output {
    command(args).output().expect("run solstone-core")
}

fn pinned_command(args: &[&str]) -> Command {
    let mut command = command(args);
    command
        .env("SOLSTONE_JOURNAL", DUMMY_JOURNAL)
        .env_remove("SOL_SUPERVISOR_SPAWNED")
        .env_remove("SOL_SKIP_SUPERVISOR_CHECK");
    command
}

fn run_pinned(args: &[&str]) -> Output {
    pinned_command(args).output().expect("run solstone-core")
}

fn run_journal(journal: &Path, args: &[&str], skip_supervisor: bool) -> Output {
    let mut command = command(args);
    command.env("SOLSTONE_JOURNAL", journal);
    if skip_supervisor {
        command.env("SOL_SKIP_SUPERVISOR_CHECK", "1");
    } else {
        command.env_remove("SOL_SKIP_SUPERVISOR_CHECK");
    }
    command.env_remove("SOL_SUPERVISOR_SPAWNED");
    command.output().expect("run solstone-core")
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("UTF-8 output")
}

fn segment(journal: &Path, day: &str, key: &str) -> PathBuf {
    let path = journal.join("chronicle").join(day).join(key);
    fs::create_dir_all(path.join("talents")).expect("create segment");
    fs::write(path.join("audio.jsonl"), "{}\n").expect("write transcript");
    fs::write(path.join("talents/audio.md"), "indexed content\n").expect("write output");
    path
}

fn seed_stream(journal: &Path) {
    fs::create_dir_all(journal.join("streams")).expect("create streams");
    fs::write(
        journal.join("streams/alpha.json"),
        "{\n  \"name\": \"alpha\",\n  \"type\": \"observer\",\n  \"host\": null,\n  \"platform\": null,\n  \"created_at\": 1,\n  \"last_day\": \"20250101\",\n  \"last_segment\": \"090000_60\",\n  \"seq\": 1\n}\n",
    )
    .expect("write stream record");
}

fn seed_eligible_sidecar(journal: &Path, day: &str) {
    let path = segment(journal, day, "090000_60");
    fs::write(path.join("audio.flac"), b"audio").expect("write audio");
    fs::write(path.join("audio.jsonl"), "{\"raw\":\"audio.flac\"}\n")
        .expect("write header-only sidecar");
}

#[test]
fn storage_ops_help_is_byte_identical_for_both_spellings() {
    for verb in VERBS {
        let fixture_name = format!("{verb} --help");
        for flag in ["--help", "-h"] {
            let output = run(&[verb, flag]);
            assert_eq!(output.status.code(), Some(0), "{verb} {flag}");
            assert_eq!(output.stderr, b"", "{verb} {flag}");
            assert_eq!(text(output.stdout), block(&fixture_name), "{verb} {flag}");
        }
    }
}

#[test]
fn segment_subcommand_help_is_byte_identical_for_both_spellings() {
    for subcommand in ["list", "inspect", "verify", "move"] {
        let fixture_name = format!("segment {subcommand} --help");
        for flag in ["--help", "-h"] {
            let output = run(&["segment", subcommand, flag]);
            assert_eq!(output.status.code(), Some(0), "{subcommand} {flag}");
            assert_eq!(output.stderr, b"");
            assert_eq!(text(output.stdout), block(&fixture_name));
        }
    }
}

#[test]
fn malformed_storage_ops_use_their_own_usage() {
    for verb in VERBS {
        let output = run_pinned(&[verb, "--nonsense"]);
        assert_eq!(output.status.code(), Some(2), "{verb}");
        assert!(output.stdout.is_empty(), "{verb}");
        let stderr = text(output.stderr);
        assert!(
            stderr.contains(&format!("usage: journal {verb}")),
            "{stderr}"
        );
        assert!(!stderr.contains("solstone-core --version"), "{stderr}");
    }
}

#[test]
fn reprocess_missing_day_matches_its_fixture_before_unknown_arguments() {
    for args in [
        ["reprocess"].as_slice(),
        ["reprocess", "--nonsense"].as_slice(),
    ] {
        let output = run_pinned(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(output.stdout, b"");
        assert_eq!(text(output.stderr), block("reprocess (missing day)"));
    }
}

#[test]
fn storage_ops_mutual_exclusion_errors_are_exact() {
    for (args, line) in [
        (
            ["backfill-processing-records", "--commit", "--dry-run"].as_slice(),
            "journal backfill-processing-records: error: argument --dry-run: not allowed with argument --commit\n",
        ),
        (
            ["reprocess", "20250101", "--from-scratch", "--mark-updated"].as_slice(),
            "journal reprocess: error: argument --mark-updated: not allowed with argument --from-scratch\n",
        ),
    ] {
        let output = run_pinned(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let stderr = text(output.stderr);
        assert!(stderr.ends_with(line), "{stderr}");
    }
}

#[test]
fn streams_and_segment_preserve_supervisor_refusal_branches() {
    for verb in ["streams", "segment"] {
        let output = run_pinned(&[verb]);
        assert_eq!(output.status.code(), Some(1), "{verb}");
        assert_eq!(output.stdout, b"");
        assert_eq!(text(output.stderr), SUPERVISOR_MESSAGE);

        let mut spawned = pinned_command(&[verb]);
        spawned.env("SOL_SUPERVISOR_SPAWNED", "1");
        let output = spawned.output().expect("run spawned child");
        assert_eq!(output.status.code(), Some(75), "{verb}");
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{verb}"
        );
    }

    let journal = TempDir::new().expect("journal");
    let output = run_journal(
        journal.path(),
        &["backfill-processing-records", "--dry-run"],
        false,
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(!text(output.stderr).contains("solstone isn't running"));
}

#[test]
fn storage_ops_real_work_reaches_each_body() {
    let journal = TempDir::new().expect("journal");
    seed_stream(journal.path());
    let source = journal.path().join("chronicle/20250101/work/090000_60");
    fs::create_dir_all(source.join("talents")).unwrap();
    fs::write(source.join("audio.jsonl"), "{}\n").unwrap();
    fs::write(source.join("talents/audio.md"), "indexed content\n").unwrap();
    fs::write(
        source.join("stream.json"),
        "{\"stream\":\"work\",\"seq\":1}",
    )
    .unwrap();
    fs::write(
        journal.path().join("streams/work.json"),
        "{\"name\":\"work\",\"type\":\"observer\",\"host\":null,\"platform\":null,\"created_at\":1,\"last_day\":\"20250101\",\"last_segment\":\"090000_60\",\"seq\":1}\n",
    )
    .unwrap();
    let before = fs::read(source.join("audio.jsonl")).expect("source bytes");

    let streams = run_journal(journal.path(), &["streams"], true);
    assert_eq!(streams.status.code(), Some(0));
    assert!(text(streams.stdout).contains("alpha"));

    let inspect = run_journal(
        journal.path(),
        &["segment", "inspect", "20250101/work/090000_60"],
        true,
    );
    assert_eq!(inspect.status.code(), Some(0), "{}", text(inspect.stderr));
    assert!(text(inspect.stdout).contains("090000_60"));

    let moved = run_journal(
        journal.path(),
        &[
            "segment",
            "move",
            "20250101/work/090000_60",
            "--to-day",
            "20250102",
            "--dry-run",
        ],
        true,
    );
    assert_eq!(moved.status.code(), Some(0));
    assert!(text(moved.stdout).contains("[dry run] No changes made"));
    assert!(source.is_dir());
    assert_eq!(fs::read(source.join("audio.jsonl")).unwrap(), before);

    let stats = run_journal(journal.path(), &["journal-stats"], true);
    assert_eq!(stats.status.code(), Some(0));
    let root_stats = journal.path().join("stats.json");
    assert!(root_stats.is_file());
    assert!(!fs::read(&root_stats).unwrap().is_empty());
    let day_cache = source
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("stats.json");
    let cache_before = fs::read(&day_cache).expect("day cache");
    let rerun = run_journal(journal.path(), &["journal-stats"], true);
    assert_eq!(rerun.status.code(), Some(0));
    assert_eq!(
        fs::read(&day_cache).unwrap(),
        cache_before,
        "unchanged day cache is reused"
    );

    seed_eligible_sidecar(journal.path(), "20250102");
    let backfill = run_journal(
        journal.path(),
        &["backfill-processing-records", "--dry-run"],
        true,
    );
    assert_eq!(backfill.status.code(), Some(0));
    assert!(text(backfill.stdout).contains("stamp_empty: 1"));

    let empty = TempDir::new().expect("empty journal");
    let empty_backfill = run_journal(
        empty.path(),
        &["backfill-processing-records", "--dry-run"],
        false,
    );
    assert_eq!(empty_backfill.status.code(), Some(0));
    assert!(text(empty_backfill.stdout).contains("stamp_empty: 0"));
    let empty_stats = run_journal(empty.path(), &["journal-stats"], false);
    assert_eq!(empty_stats.status.code(), Some(0));
    let empty_document: serde_json::Value =
        serde_json::from_slice(&fs::read(empty.path().join("stats.json")).expect("empty stats"))
            .expect("parse empty stats");
    assert_eq!(empty_document["day_count"], 0);
}

#[test]
fn storage_ops_body_diagnostics_and_reprocess_unreachable_are_preserved() {
    let journal = TempDir::new().expect("journal");
    let marker = segment(journal.path(), "20250101", "090000_60").join("stream.json");
    fs::write(&marker, b"not json").expect("bad marker");
    let rebuilt = run_journal(journal.path(), &["streams", "--rebuild"], true);
    assert_eq!(rebuilt.status.code(), Some(3));
    assert_eq!(rebuilt.stderr, b"");
    assert!(text(rebuilt.stdout).contains(&format!(
        "unreadable marker {}: could not read marker",
        marker.display()
    )));

    let plain = journal.path().join("chronicle/20250102/plain-file");
    fs::create_dir_all(plain.parent().unwrap()).unwrap();
    fs::write(&plain, b"plain").unwrap();
    let backfill = run_journal(
        journal.path(),
        &["backfill-processing-records", "--dry-run"],
        false,
    );
    assert_eq!(backfill.status.code(), Some(0));
    assert_eq!(
        text(backfill.stderr),
        format!(
            "Could not list stream directory {}: not a directory\n",
            plain.display()
        )
    );

    let reprocess = run_journal(
        journal.path(),
        &["reprocess", "20250101", "--from-scratch"],
        false,
    );
    assert_eq!(reprocess.status.code(), Some(1));
    assert_eq!(reprocess.stdout, b"");
    assert_eq!(
        text(reprocess.stderr),
        "supervisor not reachable - start it (journal start), then retry\n"
    );
}

#[test]
fn help_skips_journal_resolution_but_non_help_does_not() {
    for verb in VERBS {
        let mut command = command(&[verb, "--help"]);
        command.env_remove("SOLSTONE_JOURNAL").env("HOME", "~");
        let output = command.output().expect("run help");
        assert_eq!(output.status.code(), Some(0), "{verb}");
        assert_eq!(output.stderr, b"");
        assert_eq!(text(output.stdout), block(&format!("{verb} --help")));
    }
    let mut command = command(&["streams"]);
    command.env_remove("SOLSTONE_JOURNAL").env("HOME", "~");
    let output = command.output().expect("run unresolved journal");
    assert_eq!(output.status.code(), Some(75));
    assert!(text(output.stderr).contains("could not determine home directory"));
}

// The UTF-8 gate is sanctioned dispatcher-side handling only for this storage-ops carve-out.
#[test]
fn utf8_gate_preserves_raw_failure_and_backfill_keeps_its_inherited_lossy_divergence() {
    for verb in ["streams", "segment", "reprocess"] {
        let mut command = command(&[verb]);
        command.arg(std::ffi::OsString::from_vec(vec![0xff]));
        let output = command.output().expect("run non-UTF8 argv");
        assert_eq!(output.status.code(), Some(75), "{verb}");
        assert!(output.stdout.is_empty());
        assert!(text(output.stderr.clone()).contains("arguments are not valid UTF-8"));
        assert!(
            !output
                .stdout
                .windows(3)
                .any(|bytes| bytes == [0xef, 0xbf, 0xbd])
        );
        assert!(
            !output
                .stderr
                .windows(3)
                .any(|bytes| bytes == [0xef, 0xbf, 0xbd])
        );
    }

    let mut command = pinned_command(&["backfill-processing-records"]);
    command.arg(std::ffi::OsString::from_vec(vec![0xff]));
    let output = command.output().expect("run backfill non-UTF8 argv");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output
            .stderr
            .starts_with(b"usage: journal backfill-processing-records")
    );
    assert_eq!(
        output
            .stderr
            .windows(3)
            .filter(|bytes| *bytes == [0xef, 0xbf, 0xbd])
            .count(),
        1
    );
}
