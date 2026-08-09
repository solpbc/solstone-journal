// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use solstone_core_entity::hold_entity_trust_lock;
use solstone_core_speaker_resolve::backfill::{
    BackfillRunRequest, SpeakerLabelsState, classify_speaker_labels_text, plan_backfill_segments,
    run_backfill,
};
use solstone_core_speaker_resolve::backfill_operations::{
    BackfillOperationPayload, backfill_operations_path, load_backfill_operations,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-backfill-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn audio_segment(root: &Path, key: &str, labels: &str) {
    let path = root.join("chronicle/20260808/mic").join(key);
    fs::create_dir_all(path.join("talents")).unwrap();
    fs::write(path.join("audio.npz"), b"placeholder").unwrap();
    fs::write(path.join("talents/speaker_labels.json"), labels).unwrap();
}

#[test]
fn ac6_stub_discriminator_matches_literal_and_rejects_empty_negative_twins() {
    let literal = r#"{"labels": [], "skipped": true, "reason": "no_owner_centroid"}"#;
    assert_eq!(
        classify_speaker_labels_text(literal),
        SpeakerLabelsState::Stubbed
    );
    assert_eq!(
        classify_speaker_labels_text(r#"{"labels": [], "skipped": false}"#),
        SpeakerLabelsState::Labelled
    );
    assert_eq!(
        classify_speaker_labels_text(r#"{"labels": []}"#),
        SpeakerLabelsState::Labelled
    );
}

#[test]
fn ac5_default_backfill_reprocesses_stub_and_skips_real_labelled_segment() {
    let temporary = Temp::new();
    audio_segment(
        temporary.path(),
        "120000_300",
        r#"{"labels": [], "skipped": true, "reason": "no_owner_centroid"}"#,
    );
    audio_segment(
        temporary.path(),
        "120500_300",
        r#"{"labels": [{"sentence_id": 1, "speaker": "person"}]}"#,
    );

    let plan = plan_backfill_segments(temporary.path(), false).unwrap();
    assert_eq!(plan.total_segments, 2);
    assert_eq!(plan.total_eligible, 2);
    assert_eq!(plan.already_labeled, 1);
    assert_eq!(plan.to_process.len(), 1);
    assert_eq!(plan.to_process[0].segment_key, "120000_300");
}

#[test]
fn backfill_waits_for_the_operation_lock_without_duplicate_prepared_rows() {
    let temporary = Temp::new();
    fs::create_dir_all(temporary.path().join("chronicle")).unwrap();
    let lock = hold_entity_trust_lock(temporary.path()).unwrap();
    let root = temporary.path().to_path_buf();
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender
            .send(run_backfill(&BackfillRunRequest {
                journal_root: root,
                operation_id: "bfop-lock".to_owned(),
                reattribute: false,
                now_ms: 1,
            }))
            .unwrap();
    });
    assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
    drop(lock);
    assert!(
        receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .done
    );
    worker.join().unwrap();

    let rows = load_backfill_operations(&backfill_operations_path(temporary.path())).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row.event.payload, BackfillOperationPayload::Prepared { .. }))
            .count(),
        1
    );
}
