// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::{Command, Output};

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
