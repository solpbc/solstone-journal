// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use serde_json::{Value, json};
use solstone_core_journal_io::{
    PublishOutcome, bump_stream_marker, publish_daily_marker_if_current,
};
use solstone_core_system::catchup::{
    DailyCatchupOutcome, SegmentRepairOutcome, read_raw_input_fingerprint,
    record_daily_catchup_attempt, record_daily_catchup_outcome, record_daily_catchup_progress,
    record_segment_repair_attempt, record_segment_repair_outcome,
};

const DAY: &str = "20250101";
const RAW_INPUT: &[u8] = b"catchup-raw-input\n";
const EXPECTED_CATCHUP_STATE: &str = r#"{"entries": {"20250101:daily-catchup": {"active": null, "attempts": 0, "bounded": null, "cleared": null, "command_kind": "daily-catchup", "consecutive_non_completion": 0, "daily_progress": {"cleared": 1, "remaining": 2}, "day": "20250101", "entered_backoff_at": null, "exit_reason": null, "fingerprint": null, "last_attempt_at": 0, "last_outcome": "", "next_retry_at": 0, "notified_at": null, "reason_code": null, "remaining": null, "timeout_seconds": null}, "20250101:segment-repair": {"active": null, "attempts": 1, "bounded": true, "cleared": 1, "command_kind": "segment-repair", "consecutive_non_completion": 0, "daily_progress": null, "day": "20250101", "entered_backoff_at": null, "exit_reason": "wall_clock_exceeded", "fingerprint": "234065b2dde0314867154444b2638257585bd5ee5332dc6e48e2ff8afa9be040", "last_attempt_at": 1.0, "last_outcome": "progressing", "next_retry_at": 604.0, "notified_at": null, "reason_code": "wall_clock_exceeded", "remaining": 2, "timeout_seconds": 3.0}}, "version": 1}"#;

fn seed_raw_day(root: &std::path::Path) {
    let path = root
        .join("chronicle")
        .join(DAY)
        .join("audio")
        .join("120000_30")
        .join("audio.jsonl");
    std::fs::create_dir_all(path.parent().expect("segment dir")).unwrap();
    std::fs::write(path, RAW_INPUT).unwrap();
}

#[test]
fn catchup_writers_match_inlined_state() {
    let native_root = tempfile::tempdir().unwrap();
    seed_raw_day(native_root.path());
    let day = DAY;
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

#[test]
fn daily_catchup_completion_keeps_admitted_marker_provenance_in_the_envelope() {
    let native_root = tempfile::tempdir().unwrap();
    seed_raw_day(native_root.path());
    let fingerprint = read_raw_input_fingerprint(native_root.path(), DAY).unwrap();
    let admitted_generation = bump_stream_marker(native_root.path(), DAY).unwrap();

    record_daily_catchup_attempt(
        native_root.path(),
        DAY,
        "supervisor-catchup-20250101",
        10.0,
        admitted_generation,
        &fingerprint,
    );
    let pending: Value = serde_json::from_slice(
        &std::fs::read(native_root.path().join("health/catchup-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        pending["entries"]["20250101:daily-catchup"]["active"],
        json!({"ref": "supervisor-catchup-20250101", "started_at": 10.0})
    );
    assert_eq!(
        pending["entries"]["20250101:daily-catchup"]["admitted_generation"],
        admitted_generation
    );
    assert_eq!(
        pending["entries"]["20250101:daily-catchup"]["fingerprint"],
        fingerprint
    );

    assert_eq!(
        publish_daily_marker_if_current(
            native_root.path(),
            DAY,
            admitted_generation,
            &fingerprint,
            || read_raw_input_fingerprint(native_root.path(), DAY)
                .map_err(|error| error.to_string()),
        )
        .unwrap(),
        PublishOutcome::Published(admitted_generation)
    );
    assert!(
        record_daily_catchup_outcome(
            native_root.path(),
            DAY,
            "supervisor-catchup-20250101",
            admitted_generation,
            &fingerprint,
            DailyCatchupOutcome {
                success: false,
                timed_out: false,
                timeout_seconds: None,
                ended_at: 20.0,
                exit_code: 7,
                exit_status: "error".to_owned(),
            },
        )
        .unwrap()
    );

    let actual: Value = serde_json::from_slice(
        &std::fs::read(native_root.path().join("health/catchup-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        actual,
        json!({
            "version": 1,
            "entries": {
                "20250101:daily-catchup": {
                    "active": null,
                    "admitted_generation": admitted_generation,
                    "attempts": 0,
                    "bounded": false,
                    "cleared": null,
                    "command_kind": "daily-catchup",
                    "consecutive_non_completion": 0,
                    "daily_progress": null,
                    "day": DAY,
                    "entered_backoff_at": null,
                    "exit_code": 7,
                    "exit_reason": null,
                    "exit_status": "error",
                    "fingerprint": fingerprint,
                    "last_attempt_at": 10.0,
                    "last_outcome": "completed",
                    "next_retry_at": 0,
                    "notified_at": null,
                    "reason_code": null,
                    "remaining": null,
                    "timeout_seconds": null,
                }
            }
        }),
        "terminal daily catchup envelope retains only maintained provenance and terminal fields"
    );
}
