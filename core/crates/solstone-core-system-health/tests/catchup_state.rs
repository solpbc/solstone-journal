// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_system::catchup::{
    SegmentRepairOutcome, record_daily_catchup_progress, record_segment_repair_attempt,
    record_segment_repair_outcome,
};

const EXPECTED_CATCHUP_STATE: &str = r#"{"entries": {"20250101:segment-repair": {"active": null, "attempts": 1, "bounded": true, "cleared": 1, "command_kind": "segment-repair", "consecutive_non_completion": 0, "daily_progress": null, "day": "20250101", "entered_backoff_at": null, "exit_reason": "wall_clock_exceeded", "fingerprint": "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945", "last_attempt_at": 1.0, "last_outcome": "progressing", "next_retry_at": 604.0, "notified_at": null, "reason_code": "wall_clock_exceeded", "remaining": 2, "timeout_seconds": 3.0}}, "version": 1}"#;

fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn catchup_writers_match_inlined_state() {
    let source = super::corpus::repository_root().join("tests/fixtures/journal");
    let native_root = tempfile::tempdir().unwrap();
    copy_tree(&source, native_root.path());
    let day = "20250101";
    record_daily_catchup_progress(native_root.path(), day, 1, 2);
    record_segment_repair_attempt(native_root.path(), day, 1.0);
    record_segment_repair_outcome(
        native_root.path(),
        day,
        SegmentRepairOutcome {
            success: false,
            timed_out: true,
            timeout_seconds: Some(3.0),
            ended_at: 4.0,
            cleared: Some(1),
            remaining: Some(2),
        },
    );
    let actual: Value = serde_json::from_slice(
        &std::fs::read(native_root.path().join("health/catchup-state.json")).unwrap(),
    )
    .unwrap();
    let expected: Value = serde_json::from_str(EXPECTED_CATCHUP_STATE).unwrap();
    assert_eq!(actual, expected, "catchup-state.json after three writers");
}
