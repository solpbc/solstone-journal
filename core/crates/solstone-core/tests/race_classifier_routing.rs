// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[allow(dead_code)]
#[path = "support/await_outcome.rs"]
mod await_outcome;
#[allow(dead_code)]
#[path = "support/race_classification.rs"]
mod race_classification;

use race_classification::{RaceVerdict, classify};

const SIGKILL_FIXTURE: &str = include_str!("fixtures/cargo-test-sigkill-supervisor-tick.txt");

#[test]
fn successful_cargo_run_is_green() {
    assert_eq!(classify(0, "irrelevant"), RaceVerdict::Green);
}

#[test]
fn marker_tagged_named_libtest_failure_is_inconclusive() {
    let output = "\
---- wait_timeout stdout ----
thread 'wait_timeout' panicked at test.rs:1: W4B_INCONCLUSIVE elapsed 11ms

failures:
    wait_timeout

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";
    assert_eq!(
        classify(101, output),
        RaceVerdict::Inconclusive(
            "W4B_INCONCLUSIVE named libtest failure(s): wait_timeout".to_owned()
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
thread 'dilated_wait' panicked at test.rs:1: W4B_INCONCLUSIVE elapsed 11ms

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
thread 'first_wait' panicked at test.rs:1: W4B_INCONCLUSIVE elapsed 11ms

failures:
    first_wait

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out

---- second_wait stdout ----
thread 'second_wait' panicked at test.rs:1: W4B_INCONCLUSIVE elapsed 11ms

failures:
    second_wait

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";
    assert_eq!(
        classify(101, output),
        RaceVerdict::Inconclusive(
            "W4B_INCONCLUSIVE named libtest failure(s): first_wait, second_wait".to_owned()
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
