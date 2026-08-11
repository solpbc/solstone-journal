// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `journal observer` is served natively. A malformed invocation must therefore
//! still answer the way the Python reference answers -- argparse's exit code 2,
//! and `journal observer`'s own usage block.
//!
//! This regressed once already: the observer parse failure was mapped into the
//! generic `UsageError`, which prints `solstone-core`'s top-level usage and
//! exits 64. Every gate stayed green, because no differential covers the
//! usage-error path. The cheap way back to that bug is to route a new observer
//! parse failure through `UsageError` again, so this test pins the observable.

#![cfg(unix)]

use std::process::Command;

/// Every one of these is rejected by the reference with exit 2.
const MALFORMED: &[&[&str]] = &[
    &["observer", "bogusverb"],
    &["observer", "prune", "--nonsense"],
    &["observer", "rename", "onlyone"],
    // `--dry-run` is not part of prune's grammar: the native default IS dry-run
    // and `--execute` is the write opt-in. The reference rejects it too.
    &["observer", "prune", "--all", "--dry-run"],
];

#[test]
fn malformed_observer_invocations_exit_2_with_the_journal_observer_usage() {
    for args in MALFORMED {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(*args)
            .output()
            .expect("run solstone-core");
        let code = output.status.code().expect("exit code");
        assert_eq!(
            code,
            2,
            "`{}` exited {code}; the reference exits 2 (argparse usage error)",
            args.join(" ")
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("usage: journal observer"),
            "`{}` did not print journal observer's usage; got:\n{stderr}",
            args.join(" ")
        );
        // The failure mode this pins: printing the WRONG program's usage.
        assert!(
            !stderr.contains("solstone-core --version"),
            "`{}` printed solstone-core's top-level usage, which names the \
             wrong program to an owner who mistyped an observer argument; \
             got:\n{stderr}",
            args.join(" ")
        );
    }
}

/// Two-directional: help must NOT be swept up by the usage-error path. Without
/// this, an implementation that always exits 2 passes the test above -- and
/// that is not hypothetical, it is what the cut actually shipped. `--help` is
/// not one of the observer parser's tokens, so it fell through to the usage
/// error and an owner asking for help got exit 2 and three lines.
#[test]
fn observer_help_is_served_not_treated_as_a_usage_error() {
    for args in [
        ["observer", "--help"].as_slice(),
        ["observer", "-h"].as_slice(),
        ["observer", "prune", "--help"].as_slice(),
        ["observer", "prune", "-h"].as_slice(),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(args)
            .output()
            .expect("run solstone-core");
        assert_eq!(
            output.status.code(),
            Some(0),
            "`{}` did not exit 0; the reference serves help here",
            args.join(" ")
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Help goes to stdout and is substantial -- the usage error is three
        // lines on stderr, so a length floor separates them unambiguously.
        assert!(
            stdout.lines().count() >= 15,
            "`{}` produced {} lines of help; the reference produces ~20",
            args.join(" "),
            stdout.lines().count()
        );
        assert!(
            stdout.starts_with("usage: journal observer"),
            "`{}` help does not name journal observer; got:\n{stdout}",
            args.join(" ")
        );
    }
}

/// `prune`'s help is its own, not the observer-level help. A single shared help
/// string would satisfy the test above while losing prune's documented exit
/// contract, which is the part an operator actually needs.
#[test]
fn prune_help_is_prunes_own_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["observer", "prune", "--help"])
        .output()
        .expect("run solstone-core");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("usage: journal observer prune"),
        "prune help did not start with prune's own usage; got:\n{stdout}"
    );
    assert!(
        stdout.contains("Exit codes: 0 clean, 2 refusals present, 1 usage/error."),
        "prune help lost its documented exit contract; got:\n{stdout}"
    );
}

// --- transfer -------------------------------------------------------------
//
// `journal transfer` had the same defect and worse: EVERY invocation, including
// --help, exited 64 with solstone-core's top-level usage. The verb shipped with
// no help at all.

const TRANSFER_MALFORMED: &[&[&str]] = &[&["transfer", "--nonsense"], &["transfer", "bogus"]];

#[test]
fn malformed_transfer_invocations_exit_2_not_64() {
    for args in TRANSFER_MALFORMED {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(*args)
            .output()
            .expect("run solstone-core");
        let code = output.status.code().expect("exit code");
        assert_eq!(
            code,
            2,
            "`{}` exited {code}; the reference exits 2 (argparse usage error)",
            args.join(" ")
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("usage: journal transfer"),
            "`{}` did not print journal transfer's usage; got:\n{stderr}",
            args.join(" ")
        );
        assert!(
            !stderr.contains("solstone-core --version"),
            "`{}` printed solstone-core's top-level usage instead of the verb's; \
             got:\n{stderr}",
            args.join(" ")
        );
    }
}

#[test]
fn transfer_help_is_served_for_the_verb_and_each_subcommand() {
    // Each subcommand has its OWN help. A single shared string would pass a
    // laxer assertion while hiding every subcommand's actual grammar.
    for (args, expected_usage) in [
        (
            ["transfer", "--help"].as_slice(),
            "usage: journal transfer [-h]",
        ),
        (
            ["transfer", "-h"].as_slice(),
            "usage: journal transfer [-h]",
        ),
        (
            ["transfer", "export", "--help"].as_slice(),
            "usage: journal transfer export",
        ),
        (
            ["transfer", "import", "--help"].as_slice(),
            "usage: journal transfer import",
        ),
        (
            ["transfer", "send", "--help"].as_slice(),
            "usage: journal transfer send",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(args)
            .output()
            .expect("run solstone-core");
        assert_eq!(
            output.status.code(),
            Some(0),
            "`{}` did not exit 0; the reference serves help here",
            args.join(" ")
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.starts_with(expected_usage),
            "`{}` did not print its own help (expected it to start {expected_usage:?}); \
             got:\n{stdout}",
            args.join(" ")
        );
    }
}
