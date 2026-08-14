// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::{Command, Output};

use std::fs;
use std::os::unix::ffi::OsStringExt;

use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");
const SENTINEL: &str = "\x1fsolstone-journal-brain-owner-v1";

fn run(tail: &[&str]) -> Output {
    Command::new(BINARY)
        .arg(SENTINEL)
        .arg("brain")
        .args(tail)
        .output()
        .expect("run owner brain")
}

#[test]
fn owner_brain_help_and_errors_never_fall_through_to_aggregate_usage() {
    let bare = run(&[]);
    assert_eq!(bare.status.code(), Some(2));
    assert!(bare.stderr.is_empty());
    assert!(bare.stdout.starts_with(b"usage: journal brain"));

    let help = run(&["refresh", "--help"]);
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    assert!(help.stdout.starts_with(b"usage: journal brain refresh"));
    assert!(
        help.stdout
            .windows(b"--expected-fingerprint".len())
            .any(|window| window == b"--expected-fingerprint")
    );

    let invalid = run(&["--nonsense"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert!(invalid.stderr.starts_with(b"usage: journal brain"));
}

#[test]
fn owner_non_utf8_expected_fingerprint_resolves_stale_instead_of_usage() {
    let journal = TempDir::new_in("/var/tmp").expect("journal");
    fs::create_dir_all(journal.path().join("config")).expect("config directory");
    fs::write(journal.path().join("config/journal.json"), b"{}").expect("config");
    let output = Command::new(BINARY)
        .arg(SENTINEL)
        .arg("brain")
        .arg("refresh")
        .arg("--expected-fingerprint")
        .arg(std::ffi::OsString::from_vec(vec![0xff]))
        .env("SOLSTONE_JOURNAL", journal.path())
        .output()
        .expect("run owner brain with non-UTF-8 expected fingerprint");
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        b"Brain unknown: stale expected fingerprint\n"
    );
    assert!(!solstone_core_brain::brain_state_path(journal.path()).exists());
}
