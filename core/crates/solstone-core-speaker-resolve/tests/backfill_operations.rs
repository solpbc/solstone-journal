// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use solstone_core_speaker_resolve::backfill_operations::{
    BACKFILL_OPERATION_SCHEMA_VERSION, BackfillCheckpointOutcome, BackfillOperationError,
    BackfillOperationEvent, BackfillOperationPayload, BackfillOperationTerminalStatus,
    BackfillSegmentKey, append_backfill_event, backfill_operation_status, backfill_operations_path,
    fold_backfill_operation, load_backfill_operations,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-backfill-operations-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn ledger(&self) -> PathBuf {
        backfill_operations_path(&self.0)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn segment(index: usize) -> BackfillSegmentKey {
    BackfillSegmentKey {
        day: "20260808".to_owned(),
        stream: "mic".to_owned(),
        segment_key: format!("120{index:02}_300"),
    }
}

fn prepared() -> BackfillOperationEvent {
    BackfillOperationEvent {
        schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
        event_id: "prepared".to_owned(),
        operation_id: "bfop_test".to_owned(),
        ts: "2026-08-08T00:00:00Z".to_owned(),
        payload: BackfillOperationPayload::Prepared {
            started_at: "2026-08-08T00:00:00Z".to_owned(),
            reattribute: false,
            total_count: 5,
            segments: (0..5).map(segment).collect(),
        },
    }
}

fn checkpoint(index: usize, outcome: BackfillCheckpointOutcome) -> BackfillOperationEvent {
    BackfillOperationEvent {
        schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
        event_id: format!("checkpoint-{index}"),
        operation_id: "bfop_test".to_owned(),
        ts: format!("2026-08-08T00:0{index}:00Z"),
        payload: BackfillOperationPayload::Checkpoint {
            segment: segment(index),
            outcome,
            error_detail: (outcome == BackfillCheckpointOutcome::Error)
                .then(|| "temporary resolver failure".to_owned()),
        },
    }
}

fn completed() -> BackfillOperationEvent {
    BackfillOperationEvent {
        schema_version: BACKFILL_OPERATION_SCHEMA_VERSION,
        event_id: "completed".to_owned(),
        operation_id: "bfop_test".to_owned(),
        ts: "2026-08-08T01:00:00Z".to_owned(),
        payload: BackfillOperationPayload::Completed {
            completed_at: "2026-08-08T01:00:00Z".to_owned(),
        },
    }
}

#[test]
fn malformed_row_fails_loudly_and_prevents_append() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{not json}\n").unwrap();
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        load_backfill_operations(&path),
        Err(BackfillOperationError::MalformedJson { line: 1, .. })
    ));
    assert!(matches!(
        append_backfill_event(&path, &prepared()),
        Err(BackfillOperationError::MalformedJson { line: 1, .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn backfill_operation_append_is_append_only() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    append_backfill_event(&path, &prepared()).unwrap();
    let before = fs::read(&path).unwrap();
    let mut corruptible_model = load_backfill_operations(&path).unwrap();
    corruptible_model.clear();
    append_backfill_event(&path, &checkpoint(0, BackfillCheckpointOutcome::Processed)).unwrap();
    let after = fs::read(&path).unwrap();
    assert!(after.starts_with(&before));
    assert_eq!(&after[..before.len()], before.as_slice());
}

#[test]
fn resume_fold_returns_exactly_uncheckpointed_segments() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    append_backfill_event(&path, &prepared()).unwrap();
    append_backfill_event(&path, &checkpoint(0, BackfillCheckpointOutcome::Processed)).unwrap();
    append_backfill_event(&path, &checkpoint(1, BackfillCheckpointOutcome::Error)).unwrap();

    let rows = load_backfill_operations(&path).unwrap();
    let state = fold_backfill_operation(&rows, "bfop_test")
        .unwrap()
        .unwrap();
    assert_eq!(
        state.terminal_status,
        BackfillOperationTerminalStatus::Resumable
    );
    assert_eq!(
        state.pending_segments,
        vec![segment(1), segment(2), segment(3), segment(4)]
    );
    assert_eq!(
        state.error_details.get(&segment(1)).map(String::as_str),
        Some("temporary resolver failure")
    );
}

#[test]
fn status_reports_resumable_then_done_from_terminal_row() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    append_backfill_event(&path, &prepared()).unwrap();
    append_backfill_event(&path, &checkpoint(0, BackfillCheckpointOutcome::Processed)).unwrap();
    let rows = load_backfill_operations(&path).unwrap();
    let resumable = backfill_operation_status(&rows, "bfop_test")
        .unwrap()
        .unwrap();
    assert_eq!(resumable.total_count, 5);
    assert_eq!(resumable.completed_count, 1);
    assert_eq!(resumable.pending_count, 4);
    assert_eq!(resumable.error_count, 0);
    assert!(resumable.resumable);
    assert!(!resumable.done);

    append_backfill_event(&path, &completed()).unwrap();
    let rows = load_backfill_operations(&path).unwrap();
    let done = backfill_operation_status(&rows, "bfop_test")
        .unwrap()
        .unwrap();
    assert!(!done.resumable);
    assert!(done.done);
}

#[test]
fn errored_checkpoint_remains_retryable_and_reports_its_detail() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    append_backfill_event(&path, &prepared()).unwrap();
    append_backfill_event(&path, &checkpoint(0, BackfillCheckpointOutcome::Error)).unwrap();

    let status = backfill_operation_status(&load_backfill_operations(&path).unwrap(), "bfop_test")
        .unwrap()
        .unwrap();
    assert!(status.resumable);
    assert!(!status.done);
    assert_eq!(status.completed_count, 0);
    assert_eq!(status.pending_count, 5);
    assert_eq!(status.error_count, 1);
    assert_eq!(status.error_segments[0].segment, segment(0));
    assert_eq!(
        status.error_segments[0].detail,
        "temporary resolver failure"
    );
}

#[test]
fn legacy_error_checkpoint_without_detail_remains_readable_and_retryable() {
    let temporary = TempDir::new();
    let path = temporary.ledger();
    append_backfill_event(&path, &prepared()).unwrap();
    let mut legacy = checkpoint(0, BackfillCheckpointOutcome::Error);
    let BackfillOperationPayload::Checkpoint { error_detail, .. } = &mut legacy.payload else {
        unreachable!("checkpoint fixture has checkpoint payload");
    };
    *error_detail = None;
    append_backfill_event(&path, &legacy).unwrap();

    let status = backfill_operation_status(&load_backfill_operations(&path).unwrap(), "bfop_test")
        .unwrap()
        .unwrap();
    assert!(status.resumable);
    assert_eq!(status.error_count, 1);
    assert_eq!(
        status.error_segments[0].detail,
        "legacy checkpoint did not retain error detail"
    );
}
