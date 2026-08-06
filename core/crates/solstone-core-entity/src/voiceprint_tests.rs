// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::store::voiceprints::write_voiceprints_npz;
use crate::{
    VoiceprintItem, VoiceprintOperationError, VoiceprintRemoval, load_entity_voiceprints_file,
    load_existing_voiceprint_keys, normalize_embedding, remove_voiceprints_by_key,
    rewrite_voiceprint_metadata, save_voiceprints_batch,
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);
const VOICEPRINT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/voiceprint_operations.json"
));

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-voiceprints-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn normalize_embedding_matches_python_and_rejects_zero() {
    let normalized = normalize_embedding(&[3.0, 4.0]).unwrap();
    assert_eq!(normalized, vec![0.6, 0.8]);
    assert_eq!(normalize_embedding(&[0.0, 0.0]), None);
    assert_eq!(normalize_embedding(&[f32::NAN, 1.0]), None);
}

#[test]
fn empty_inputs_return_before_entity_resolution() {
    let temporary = TempDir::new();
    assert_eq!(
        save_voiceprints_batch(temporary.path(), "missing", &[]).unwrap(),
        0
    );
    assert_eq!(
        remove_voiceprints_by_key(temporary.path(), "missing", &[]).unwrap(),
        Default::default()
    );
}

#[test]
fn load_is_lenient_for_missing_entity_and_missing_file() {
    let temporary = fixture_journal();
    assert!(load_entity_voiceprints_file(temporary.path(), "missing").is_none());
    let path = voiceprint_path(temporary.path());
    fs::remove_file(&path).unwrap();
    assert!(load_entity_voiceprints_file(temporary.path(), fixture_entity_id()).is_none());
}

#[test]
fn list_keys_collapses_null_absent_and_python_equal_numbers() {
    let temporary = fixture_journal();
    let keys = load_existing_voiceprint_keys(temporary.path(), fixture_entity_id());
    assert_eq!(keys.len(), 5);
}

#[test]
fn remove_matches_null_key_when_request_omits_field() {
    let temporary = fixture_journal();
    let metadata = fixture()["metadata"]["null_key"].clone();
    let report = remove_voiceprints_by_key(
        temporary.path(),
        fixture_entity_id(),
        &[VoiceprintRemoval {
            key: json!({
                "day": metadata["day"],
                "segment_key": metadata["segment_key"],
                "sentence_id": metadata["sentence_id"],
            }),
            expected_metadata: Some(metadata),
        }],
    )
    .unwrap();
    assert_eq!(report.removed_count, 1);
    assert_eq!(report.skipped_count, 0);
}

#[test]
fn save_batch_appends_and_round_trips_decoded_metadata() {
    let temporary = fixture_journal();
    let item = VoiceprintItem {
        embedding: embedding(8.0),
        metadata: json!({
            "day": "20260805",
            "segment_key": "appended",
            "source": "mic_audio",
            "sentence_id": 8,
            "note": "saved without implicit normalization",
        }),
    };
    assert_eq!(
        save_voiceprints_batch(temporary.path(), fixture_entity_id(), &[item]).unwrap(),
        1
    );
    let archive = load_entity_voiceprints_file(temporary.path(), fixture_entity_id()).unwrap();
    assert_eq!(archive.rows, 8);
    assert_eq!(archive.embeddings[7 * 256], 8.0);
    assert_eq!(
        serde_json::from_str::<Value>(&archive.metadata[7]).unwrap()["segment_key"],
        "appended"
    );
}

#[test]
fn remove_preserves_decoded_survivors_after_metadata_width_changes() {
    let temporary = fixture_journal();
    let removal = fixture_removal("width");
    let report =
        remove_voiceprints_by_key(temporary.path(), fixture_entity_id(), &[removal]).unwrap();
    assert_eq!(report.removed_count, 1);
    let archive = load_entity_voiceprints_file(temporary.path(), fixture_entity_id()).unwrap();
    let survivor = fixture()["metadata"]["survivor"].clone();
    let expected = fixture()["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["metadata"]["segment_key"] != "width")
        .map(|row| row["metadata"].clone())
        .collect::<Vec<_>>();
    let decoded = archive
        .metadata
        .iter()
        .map(|metadata| serde_json::from_str::<Value>(metadata).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(archive.rows, 6);
    assert_eq!(decoded.len(), expected.len());
    for metadata in expected {
        assert!(decoded.contains(&metadata));
    }
    let survivor_index = decoded
        .iter()
        .position(|metadata| *metadata == survivor)
        .unwrap();
    assert_eq!(archive.embeddings[survivor_index * 256], 4.0);
}

#[test]
fn remove_matches_python_equal_int_and_float_metadata_and_key_values() {
    let temporary = fixture_journal();
    let report = remove_voiceprints_by_key(
        temporary.path(),
        fixture_entity_id(),
        &[fixture_removal("numeric")],
    )
    .unwrap();
    assert_eq!(report.removed_count, 1);
    assert_eq!(report.skipped_count, 0);
}

#[test]
fn remove_reports_missing_and_metadata_mismatch_without_writing() {
    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    let before = fs::read(&path).unwrap();
    let report = remove_voiceprints_by_key(
        temporary.path(),
        fixture_entity_id(),
        &[fixture_removal("missing"), fixture_removal("mismatch")],
    )
    .unwrap();
    assert_eq!(report.removed_count, 0);
    assert_eq!(report.skipped_count, 2);
    assert_eq!(report.skipped_reasons.missing, 1);
    assert_eq!(report.skipped_reasons.metadata_mismatch, 1);
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn remove_nonempty_requests_skips_unresolvable_entity_and_absent_archive() {
    let removals = [fixture_removal("numeric"), fixture_removal("missing")];

    let temporary = TempDir::new();
    let report = remove_voiceprints_by_key(temporary.path(), "missing", &removals).unwrap();
    assert_all_missing(&report, removals.len());

    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    fs::remove_file(&path).unwrap();
    let report =
        remove_voiceprints_by_key(temporary.path(), fixture_entity_id(), &removals).unwrap();
    assert_all_missing(&report, removals.len());
    assert!(!path.exists());
}

#[test]
fn remove_all_deletes_archive() {
    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    fs::remove_file(&path).unwrap();
    let metadata = json!({
        "day": "20260805",
        "segment_key": "sole",
        "source": "mic_audio",
        "sentence_id": 12,
    });
    save_voiceprints_batch(
        temporary.path(),
        fixture_entity_id(),
        &[VoiceprintItem {
            embedding: embedding(12.0),
            metadata: metadata.clone(),
        }],
    )
    .unwrap();
    let report = remove_voiceprints_by_key(
        temporary.path(),
        fixture_entity_id(),
        &[VoiceprintRemoval {
            key: key_value(&metadata),
            expected_metadata: Some(metadata),
        }],
    )
    .unwrap();
    assert_eq!(report.removed_count, 1);
    assert!(report.file_removed);
    assert!(!path.exists());
    assert!(path.parent().unwrap().exists());
    assert!(path.with_file_name("voiceprints.npz.lock").exists());
}

#[test]
fn rewrite_no_change_is_a_noop() {
    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    let before = fs::read(&path).unwrap();
    assert_eq!(
        rewrite_voiceprint_metadata(temporary.path(), fixture_entity_id(), |_| 0).unwrap(),
        0
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn rewrite_then_remove_uses_rewritten_metadata() {
    let temporary = fixture_journal();
    assert_eq!(
        rewrite_voiceprint_metadata(temporary.path(), fixture_entity_id(), |rows| {
            let row = rows
                .iter_mut()
                .find(|row| row["segment_key"] == "numeric")
                .unwrap();
            row["rank"] = json!(2);
            1
        })
        .unwrap(),
        1
    );
    let stale_report = remove_voiceprints_by_key(
        temporary.path(),
        fixture_entity_id(),
        &[fixture_removal("numeric")],
    )
    .unwrap();
    assert_eq!(stale_report.removed_count, 0);
    assert_eq!(stale_report.skipped_reasons.metadata_mismatch, 1);
    assert_eq!(stale_report.skipped_reasons.missing, 0);

    let mut removal = fixture_removal("numeric");
    removal.expected_metadata.as_mut().unwrap()["rank"] = json!(2.0);
    let report =
        remove_voiceprints_by_key(temporary.path(), fixture_entity_id(), &[removal]).unwrap();
    assert_eq!(report.removed_count, 1);
}

#[test]
fn duplicate_python_equal_exact_matches_are_refused_without_partial_write() {
    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    let before = fs::read(&path).unwrap();
    let error = remove_voiceprints_by_key(
        temporary.path(),
        fixture_entity_id(),
        &[fixture_removal("numeric"), fixture_removal("duplicate")],
    )
    .unwrap_err();
    assert!(matches!(
        error,
        VoiceprintOperationError::DuplicateExactMatch
    ));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn concurrent_batch_saves_preserve_both_updates() {
    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    fs::remove_file(path).unwrap();
    let root = Arc::new(temporary.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(2));
    let (sender, receiver) = mpsc::channel();
    let mut workers = Vec::new();
    for sentence_id in [21_u64, 22] {
        let root = Arc::clone(&root);
        let barrier = Arc::clone(&barrier);
        let sender = sender.clone();
        workers.push(thread::spawn(move || {
            barrier.wait();
            let result = save_voiceprints_batch(
                &root,
                fixture_entity_id(),
                &[VoiceprintItem {
                    embedding: embedding(sentence_id as f32),
                    metadata: json!({
                        "day": "20260805",
                        "segment_key": format!("parallel-{sentence_id}"),
                        "source": "mic_audio",
                        "sentence_id": sentence_id,
                    }),
                }],
            );
            sender.send(result).unwrap();
        }));
    }
    drop(sender);
    for _ in 0..2 {
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap(),
            1
        );
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(
        load_entity_voiceprints_file(temporary.path(), fixture_entity_id())
            .unwrap()
            .rows,
        2
    );
}

#[test]
fn corrupt_archive_refuses_mutation_without_overwrite() {
    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    fs::write(&path, b"not an npz archive").unwrap();
    let before = fs::read(&path).unwrap();
    let error = save_voiceprints_batch(
        temporary.path(),
        fixture_entity_id(),
        &[VoiceprintItem {
            embedding: embedding(30.0),
            metadata: json!({"day": "20260805", "sentence_id": 30}),
        }],
    )
    .unwrap_err();
    assert!(matches!(error, VoiceprintOperationError::Npz(_)));
    assert_eq!(fs::read(path).unwrap(), before);

    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    fs::write(&path, b"not an npz archive").unwrap();
    let before = fs::read(&path).unwrap();
    let error =
        rewrite_voiceprint_metadata(temporary.path(), fixture_entity_id(), |_| 1).unwrap_err();
    assert!(matches!(error, VoiceprintOperationError::Npz(_)));
    assert_eq!(fs::read(path).unwrap(), before);

    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    fs::write(&path, b"not an npz archive").unwrap();
    let before = fs::read(&path).unwrap();
    let error = remove_voiceprints_by_key(
        temporary.path(),
        fixture_entity_id(),
        &[fixture_removal("numeric")],
    )
    .unwrap_err();
    assert!(matches!(error, VoiceprintOperationError::Npz(_)));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn load_and_list_collapse_corrupt_archive_to_absence() {
    let temporary = fixture_journal();
    fs::write(voiceprint_path(temporary.path()), b"not an npz archive").unwrap();
    assert!(load_entity_voiceprints_file(temporary.path(), fixture_entity_id()).is_none());
    assert!(load_existing_voiceprint_keys(temporary.path(), fixture_entity_id()).is_empty());
}

#[test]
fn non_scalar_removal_key_is_refused() {
    let temporary = fixture_journal();
    let error = remove_voiceprints_by_key(
        temporary.path(),
        fixture_entity_id(),
        &[VoiceprintRemoval {
            key: json!({
                "day": "20260805",
                "segment_key": "numeric",
                "source": "mic_audio",
                "sentence_id": [4],
            }),
            expected_metadata: Some(fixture()["metadata"]["numeric"].clone()),
        }],
    )
    .unwrap_err();
    assert!(matches!(
        error,
        VoiceprintOperationError::UnsupportedKeyField {
            field: "sentence_id"
        }
    ));
}

fn fixture() -> Value {
    serde_json::from_str(VOICEPRINT_FIXTURE).unwrap()
}

fn fixture_entity_id() -> &'static str {
    "voiceprint_fixture"
}

fn fixture_journal() -> TempDir {
    let fixture = fixture();
    let temporary = TempDir::new();
    let identity_path = temporary
        .path()
        .join("entities")
        .join(fixture["entity_id"].as_str().unwrap())
        .join("entity.json");
    fs::create_dir_all(identity_path.parent().unwrap()).unwrap();
    fs::write(
        identity_path,
        json!({"id": fixture_entity_id(), "name": "Voiceprint Fixture", "type": "Person"})
            .to_string(),
    )
    .unwrap();
    let rows = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .collect::<Vec<_>>();
    let embeddings = rows
        .iter()
        .flat_map(|row| row["embedding"].as_array().unwrap())
        .map(|value| value.as_f64().unwrap() as f32)
        .collect::<Vec<_>>();
    let metadata = rows
        .iter()
        .map(|row| serde_json::to_string(&row["metadata"]).unwrap())
        .collect::<Vec<_>>();
    let path = voiceprint_path(temporary.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, write_voiceprints_npz(&embeddings, &metadata).unwrap()).unwrap();
    temporary
}

fn voiceprint_path(root: &Path) -> PathBuf {
    root.join("entities")
        .join(fixture_entity_id())
        .join("voiceprints.npz")
}

fn fixture_removal(name: &str) -> VoiceprintRemoval {
    let removal = fixture()["removals"][name].clone();
    VoiceprintRemoval {
        key: removal["key"].clone(),
        expected_metadata: Some(removal["expected_metadata"].clone()),
    }
}

fn key_value(metadata: &Value) -> Value {
    json!({
        "day": metadata.get("day"),
        "segment_key": metadata.get("segment_key"),
        "source": metadata.get("source"),
        "sentence_id": metadata.get("sentence_id"),
    })
}

fn assert_all_missing(report: &crate::VoiceprintRemovalReport, count: usize) {
    assert_eq!(report.removed_count, 0);
    assert_eq!(report.skipped_count, count);
    assert_eq!(report.skipped_reasons.missing, count);
    assert_eq!(report.skipped_reasons.metadata_mismatch, 0);
}

fn embedding(value: f32) -> Vec<f32> {
    let mut embedding = vec![0.0; 256];
    embedding[0] = value;
    embedding
}
