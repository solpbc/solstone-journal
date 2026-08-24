// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::json;
use solstone_core_entity::hold_entity_trust_lock;
use solstone_core_npy::write_npy;
use solstone_core_speaker_resolve::backfill::{
    BackfillRunRequest, SpeakerLabelsState, classify_speaker_labels_text, plan_backfill_segments,
    run_backfill,
};
use solstone_core_speaker_resolve::backfill_operations::{
    BackfillCheckpointOutcome, BackfillOperationPayload, backfill_operations_path,
    fold_backfill_operation, load_backfill_operations,
};
use solstone_core_speaker_resolve::owner_admission::OWNER_IDENTITY_INVALID_REASON;
use solstone_core_speaker_resolve::owner_centroid::{
    OwnerCentroidWriteInput, write_owner_centroid,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = PathBuf::from("/var/tmp").join(format!(
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

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn i32_bytes(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn owner_embedding() -> Vec<f32> {
    let mut embedding = vec![0.0; 256];
    embedding[0] = 1.0;
    embedding
}

fn resolution_segment(root: &Path, key: &str) {
    let path = root.join("chronicle/20260808/mic").join(key);
    fs::create_dir_all(path.join("talents")).unwrap();
    fs::write(
        path.join("audio.jsonl"),
        "{\"raw\":\"audio.flac\"}\n{\"id\":1,\"text\":\"test\"}\n",
    )
    .unwrap();
    fs::write(
        path.join("talents/speaker_labels.json"),
        json!({"labels":[{"sentence_id":1}]}).to_string(),
    )
    .unwrap();
    fs::write(
        path.join("talents/speaker_corrections.json"),
        json!({"corrections":[]}).to_string(),
    )
    .unwrap();
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive.start_file("embeddings.npy", options).unwrap();
    archive
        .write_all(&write_npy(
            "<f4",
            "(1, 256)",
            &f32_bytes(&owner_embedding()),
        ))
        .unwrap();
    archive.start_file("statement_ids.npy", options).unwrap();
    archive
        .write_all(&write_npy("<i4", "(1,)", &i32_bytes(&[1])))
        .unwrap();
    fs::write(
        path.join("audio.npz"),
        archive.finish().unwrap().into_inner(),
    )
    .unwrap();
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, snapshot);
            } else if path.is_file() {
                snapshot.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    collect(root, root, &mut snapshot);
    snapshot
}

fn snapshot_without_backfill_ledger(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut snapshot = snapshot_files(root);
    snapshot.remove(Path::new("speakers/backfill-operations.jsonl"));
    snapshot
}

fn repair_owner_identity(root: &Path) {
    fs::write(
        root.join("entities/owner/entity.json"),
        json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}).to_string(),
    )
    .unwrap();
    write_owner_centroid(
        root,
        "owner",
        &OwnerCentroidWriteInput {
            centroid: owner_embedding(),
            cluster_size: 1,
            timestamp: "2026-08-08T00:00:00Z".to_owned(),
            evidence_tier: "test".to_owned(),
        },
    )
    .unwrap();
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

#[test]
fn identity_invalid_checkpoint_retries_after_owner_repair_without_nonledger_writes() {
    let temporary = Temp::new();
    fs::create_dir_all(temporary.path().join("entities/owner")).unwrap();
    fs::write(
        temporary.path().join("entities/owner/entity.json"),
        json!({"id":"owner","name":"Owner","is_principal":true}).to_string(),
    )
    .unwrap();
    fs::create_dir_all(temporary.path().join("entities/candidate")).unwrap();
    fs::write(
        temporary.path().join("entities/candidate/entity.json"),
        json!({"id":"candidate","name":"Candidate","type":"Person"}).to_string(),
    )
    .unwrap();
    fs::write(
        temporary.path().join("entities/candidate/voiceprints.npz"),
        b"preserved voiceprints",
    )
    .unwrap();
    fs::create_dir_all(temporary.path().join("awareness")).unwrap();
    fs::write(
        temporary.path().join("awareness/speaker_candidates.json"),
        b"preserved candidates",
    )
    .unwrap();
    fs::create_dir_all(temporary.path().join("config/actions")).unwrap();
    fs::write(
        temporary.path().join("config/actions/test.jsonl"),
        b"preserved action\n",
    )
    .unwrap();
    fs::create_dir_all(temporary.path().join("health/locks")).unwrap();
    fs::write(temporary.path().join("health/locks/entity-trust.lock"), b"").unwrap();
    for key in ["120000_300", "120500_300"] {
        resolution_segment(temporary.path(), key);
    }

    let ledger_path = backfill_operations_path(temporary.path());
    fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
    fs::write(ledger_path.with_extension("jsonl.lock"), b"").unwrap();
    let legacy_prefix = br#"{"schema_version":1,"event_id":"bfop-owner:prepared","operation_id":"bfop-owner","event_kind":"prepared","ts":"2026-08-08T00:00:00Z","started_at":"2026-08-08T00:00:00Z","reattribute":false,"total_count":2,"segments":[{"day":"20260808","stream":"mic","segment_key":"120000_300"},{"day":"20260808","stream":"mic","segment_key":"120500_300"}]}
"#;
    fs::write(&ledger_path, legacy_prefix).unwrap();
    let before = snapshot_without_backfill_ledger(temporary.path());

    let refused = run_backfill(&BackfillRunRequest {
        journal_root: temporary.path().to_path_buf(),
        operation_id: "bfop-owner".to_owned(),
        reattribute: false,
        now_ms: 1,
    })
    .unwrap();
    assert_eq!(refused.error_count, 2);
    assert_eq!(refused.pending_count, 2);
    assert!(!refused.done);
    assert_eq!(snapshot_without_backfill_ledger(temporary.path()), before);

    let refused_rows = load_backfill_operations(&ledger_path).unwrap();
    let refused_checkpoint = refused_rows
        .iter()
        .find(|row| row.event.event_id == "bfop-owner:checkpoint:20260808:mic:120000_300")
        .expect("identity-invalid checkpoint");
    assert!(matches!(
        &refused_checkpoint.event.payload,
        BackfillOperationPayload::Checkpoint {
            outcome: BackfillCheckpointOutcome::Error,
            error_detail: Some(detail),
            ..
        } if detail == OWNER_IDENTITY_INVALID_REASON
    ));
    assert!(!refused_rows.iter().any(|row| matches!(
        row.event.payload,
        BackfillOperationPayload::Completed { .. }
    )));
    let earlier_ledger_bytes = fs::read(&ledger_path).unwrap();
    assert!(earlier_ledger_bytes.starts_with(legacy_prefix));

    repair_owner_identity(temporary.path());
    let repaired = run_backfill(&BackfillRunRequest {
        journal_root: temporary.path().to_path_buf(),
        operation_id: "bfop-owner".to_owned(),
        reattribute: false,
        now_ms: 2,
    })
    .unwrap();
    assert_eq!(repaired.processed_count, 2);
    assert_eq!(repaired.error_count, 0);
    assert_eq!(repaired.pending_count, 0);
    assert!(repaired.done);

    let repaired_rows = load_backfill_operations(&ledger_path).unwrap();
    for key in ["120000_300", "120500_300"] {
        let event_id = format!("bfop-owner:checkpoint:20260808:mic:{key}:retry:1");
        assert!(repaired_rows.iter().any(|row| {
            row.event.event_id == event_id
                && matches!(
                    row.event.payload,
                    BackfillOperationPayload::Checkpoint {
                        outcome: BackfillCheckpointOutcome::Processed,
                        ..
                    }
                )
        }));
    }
    assert_eq!(
        repaired_rows
            .iter()
            .filter(|row| matches!(
                row.event.payload,
                BackfillOperationPayload::Completed { .. }
            ))
            .count(),
        1
    );
    let state = fold_backfill_operation(&repaired_rows, "bfop-owner")
        .unwrap()
        .expect("operation remains present");
    assert!(state.pending_segments.is_empty());
    assert!(state.error_details.is_empty());
    let repaired_ledger_bytes = fs::read(&ledger_path).unwrap();
    assert!(repaired_ledger_bytes.starts_with(&earlier_ledger_bytes));
}
