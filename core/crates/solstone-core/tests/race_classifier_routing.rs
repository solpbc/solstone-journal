// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::race_classification::{RaceVerdict, classify};

const SIGKILL_FIXTURE: &str = include_str!("fixtures/cargo-test-sigkill-supervisor-tick.txt");
const REAL_MARKED_FIXTURE: &str = include_str!("fixtures/cargo-test-real-marked-inconclusive.txt");

#[test]
fn successful_cargo_run_is_green() {
    assert_eq!(classify(0, "irrelevant"), RaceVerdict::Green);
}

#[test]
fn marker_tagged_named_libtest_failure_is_inconclusive() {
    let output = "\
---- wait_timeout stdout ----
thread 'wait_timeout' panicked at test.rs:1: SUPERVISOR_RACE_INCONCLUSIVE elapsed 11ms

failures:
    wait_timeout

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";
    assert_eq!(
        classify(101, output),
        RaceVerdict::Inconclusive(
            "SUPERVISOR_RACE_INCONCLUSIVE named libtest failure(s): wait_timeout".to_owned()
        )
    );
}

#[test]
fn ordinary_named_libtest_failure_is_failed() {
    let output = "\
---- ordering_failure stdout ----
thread 'ordering_failure' panicked at test.rs:1: assertion failed

failures:
    ordering_failure

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";
    assert_eq!(
        classify(101, output),
        RaceVerdict::Failed("named libtest failure(s): ordering_failure".to_owned())
    );
}

#[test]
fn combined_capture_keeps_ordinary_failure_when_another_block_is_marked() {
    let output = "\
---- dilated_wait stdout ----
thread 'dilated_wait' panicked at test.rs:1: SUPERVISOR_RACE_INCONCLUSIVE elapsed 11ms

failures:
    dilated_wait

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out

---- ordering_failure stdout ----
thread 'ordering_failure' panicked at test.rs:1: assertion failed

failures:
    ordering_failure

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";
    assert_eq!(
        classify(101, output),
        RaceVerdict::Failed("named libtest failure(s): ordering_failure".to_owned())
    );
}

#[test]
fn combined_capture_with_only_marked_blocks_is_inconclusive() {
    let output = "\
---- first_wait stdout ----
thread 'first_wait' panicked at test.rs:1: SUPERVISOR_RACE_INCONCLUSIVE elapsed 11ms

failures:
    first_wait

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out

---- second_wait stdout ----
thread 'second_wait' panicked at test.rs:1: SUPERVISOR_RACE_INCONCLUSIVE elapsed 11ms

failures:
    second_wait

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";
    assert_eq!(
        classify(101, output),
        RaceVerdict::Inconclusive(
            "SUPERVISOR_RACE_INCONCLUSIVE named libtest failure(s): first_wait, second_wait"
                .to_owned()
        )
    );
}

#[test]
fn sigkill_fixture_is_inconclusive() {
    assert_eq!(
        classify(101, SIGKILL_FIXTURE),
        RaceVerdict::Inconclusive(
            "cargo test aborted before a parseable outcome: -p solstone-core --test supervisor_tick"
                .to_owned()
        )
    );
}

#[test]
fn nonzero_without_test_binary_evidence_is_failed() {
    assert_eq!(
        classify(101, "error: failed to compile dependency"),
        RaceVerdict::Failed("cargo build or runner failure before test-binary evidence".to_owned())
    );
}

// Captured from a real `make check-rust-race` run on a measured-quiet 16-core
// host: a supervisor-race wait exceeded its budget with dilation 1.22x, panicked carrying
// SUPERVISOR_RACE_INCONCLUSIVE, and the classifier still reported a hard FAILED.
//
// The synthetic capture in marker_tagged_named_libtest_failure_is_inconclusive
// passes with or without that bug, because it OMITS the leading bare
// `failures:` block that libtest emits before the per-test stdout sections.
// With the leading block present the marker search window pointed at the half
// of the output that cannot contain the header. This fixture is the real shape.
#[test]
fn real_capture_with_leading_failures_block_routes_marked_wait_to_inconclusive() {
    let verdict = classify(101, REAL_MARKED_FIXTURE);
    assert_eq!(
        verdict,
        RaceVerdict::Inconclusive(
            "SUPERVISOR_RACE_INCONCLUSIVE named libtest failure(s): \
ac14_shutdown_clears_lifecycle_in_order_and_reaps_task_child"
                .to_owned()
        ),
        "a dilated wait carrying SUPERVISOR_RACE_INCONCLUSIVE must never route to FAILED -- \
that is the false red AC3 exists to prevent"
    );
}
