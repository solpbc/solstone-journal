// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Local;
use serde_json::Value;

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempJournal {
    path: PathBuf,
}

impl TempJournal {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-facet-candidates-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary journal");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_sense(journal: &Path, day: &str, segment: &str, name: &str) {
    let talents = journal
        .join("chronicle")
        .join(day)
        .join("archon")
        .join(segment)
        .join("talents");
    fs::create_dir_all(&talents).expect("create sense fixture directory");
    fs::write(
        talents.join("sense.json"),
        serde_json::json!({"speculative_facet": name}).to_string(),
    )
    .expect("write sense fixture");
}

fn two_candidate_fixture() -> TempJournal {
    let journal = TempJournal::new();
    let day = Local::now().format("%Y%m%d").to_string();
    for segment in ["090000_300", "093000_300", "100000_300"] {
        write_sense(journal.path(), &day, segment, "Home Reno");
    }
    for segment in ["103000_300", "110000_300", "113000_300"] {
        write_sense(journal.path(), &day, segment, "Field Notes");
    }
    journal
}

fn candidate_rows(journal: &Path) -> Vec<Value> {
    let path = journal.join("facets/review-candidates.jsonl");
    BufReader::new(fs::File::open(path).expect("open candidate store"))
        .lines()
        .map(|line| serde_json::from_str(&line.expect("read candidate row")).expect("JSON row"))
        .collect()
}

fn native_command(journal: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .arg("facet-candidates")
        .env("SOLSTONE_JOURNAL", journal)
        .env_remove("SOL_SUPERVISOR_SPAWNED");
    command
}

fn write_forbidden_shim(path: &Path) {
    fs::write(
        path,
        "#!/bin/sh\nset -eu\nprintf '%s %s\\n' \"$0\" \"$*\" >> \"$SOLSTONE_CI_SENTINEL\"\nexit 97\n",
    )
    .expect("write forbidden shim");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make shim executable");
}

#[test]
fn spawned_binary_records_exactly_two_candidates_and_prints_their_count() {
    let journal = two_candidate_fixture();
    let output = native_command(journal.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run solstone-core facet-candidates");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        "Recorded/updated 2 facet candidate(s).\n"
    );

    let rows = candidate_rows(journal.path());
    assert_eq!(rows.len(), 2);
    for (name_key, expected_segments) in [
        ("home reno", ["090000_300", "093000_300", "100000_300"]),
        ("field notes", ["103000_300", "110000_300", "113000_300"]),
    ] {
        let row = rows
            .iter()
            .find(|row| row.get("name_key").and_then(Value::as_str) == Some(name_key))
            .expect("recorded candidate");
        assert_eq!(row.get("count").and_then(Value::as_u64), Some(3));
        let samples = row
            .pointer("/evidence/samples")
            .and_then(Value::as_array)
            .expect("candidate samples");
        assert_eq!(samples.len(), 3);
        assert_eq!(
            samples
                .iter()
                .map(|sample| sample.get("segment").and_then(Value::as_str))
                .collect::<Vec<_>>(),
            expected_segments.map(Some).to_vec()
        );
    }
}

#[test]
fn native_facet_candidates_does_not_invoke_python_from_path() {
    let journal = two_candidate_fixture();
    let shim_dir = journal.path().join("python-shims");
    fs::create_dir_all(&shim_dir).expect("create shim directory");
    write_forbidden_shim(&shim_dir.join("python"));
    write_forbidden_shim(&shim_dir.join("python3"));
    let sentinel = journal.path().join("python-invocation");
    let mut path = OsString::from(shim_dir);
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());

    // This PATH-shim proves the native `facet-candidates` implementation does
    // not shell out to Python. It does not cover the still-Python-routed
    // `journal facet-candidates` dispatcher, which finds a sibling interpreter.
    let output = native_command(journal.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .env("PATH", path)
        .env("SOLSTONE_CI_SENTINEL", &sentinel)
        .output()
        .expect("run native facet-candidates with forbidden Python shims");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        "Recorded/updated 2 facet candidate(s).\n"
    );
    assert!(
        !sentinel.exists(),
        "native command invoked Python from PATH"
    );
}

#[test]
fn supervisor_gate_blocks_before_any_candidate_write() {
    let journal = TempJournal::new();
    let output = native_command(journal.path())
        .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
        .env_remove("SOL_SUPERVISOR_SPAWNED")
        .output()
        .expect("run native facet-candidates");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        "sol: solstone isn't running. Start it with 'journal up' and retry.\n"
    );
    assert!(!journal.path().join("facets").exists());
}

#[test]
fn spawned_supervisor_gate_returns_tempfail_without_writes() {
    let journal = TempJournal::new();
    let output = native_command(journal.path())
        .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
        .env("SOL_SUPERVISOR_SPAWNED", "1")
        .output()
        .expect("run native facet-candidates");
    assert_eq!(output.status.code(), Some(75));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert!(!journal.path().join("facets").exists());
}

#[test]
fn skipped_supervisor_gate_proceeds_with_empty_journal() {
    let journal = TempJournal::new();
    let output = native_command(journal.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run native facet-candidates");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        "Recorded/updated 0 facet candidate(s).\n"
    );
    assert_eq!(output.stderr, b"");
}

#[test]
fn invalid_journal_roots_fail_without_a_success_line() {
    let journal = TempJournal::new();
    let missing = journal.path().join("missing");
    let file = journal.path().join("not-a-directory");
    fs::write(&file, "not a journal").expect("write regular-file root");

    for root in [&missing, &file] {
        let output = native_command(root)
            .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
            .output()
            .expect("run native facet-candidates");
        assert_ne!(output.status.code(), Some(0), "{root:?}");
        assert_eq!(output.stdout, b"", "{root:?}");
        assert!(
            String::from_utf8(output.stderr)
                .expect("stderr is UTF-8")
                .contains("journal facet-candidates"),
            "{root:?}"
        );
    }
}
