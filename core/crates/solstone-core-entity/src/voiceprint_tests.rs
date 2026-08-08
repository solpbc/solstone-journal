// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::store::voiceprints::{read_voiceprints_npz, write_voiceprints_npz};
use crate::{
    EncoderIdentity, VoiceprintItem, VoiceprintOperationError, VoiceprintRemoval,
    load_entity_voiceprints_file, load_existing_voiceprint_keys, normalize_embedding,
    remove_voiceprints_by_key as remove_voiceprints_by_key_with_encoder,
    rewrite_voiceprint_metadata as rewrite_voiceprint_metadata_with_encoder,
    save_voiceprints_batch as save_voiceprints_batch_with_encoder,
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

fn test_encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test-encoder".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

fn save_voiceprints_batch(
    journal_root: &Path,
    entity_id: &str,
    new_items: &[VoiceprintItem],
) -> Result<usize, VoiceprintOperationError> {
    save_voiceprints_batch_with_encoder(journal_root, entity_id, new_items, &test_encoder())
}

fn rewrite_voiceprint_metadata<F>(
    journal_root: &Path,
    entity_id: &str,
    mutator: F,
) -> Result<usize, VoiceprintOperationError>
where
    F: FnOnce(&mut [Value]) -> usize,
{
    rewrite_voiceprint_metadata_with_encoder(journal_root, entity_id, &test_encoder(), mutator)
}

fn remove_voiceprints_by_key(
    journal_root: &Path,
    entity_id: &str,
    removals: &[VoiceprintRemoval],
) -> Result<crate::VoiceprintRemovalReport, VoiceprintOperationError> {
    remove_voiceprints_by_key_with_encoder(journal_root, entity_id, removals, &test_encoder())
}

fn rewrite_member(archive: &[u8], name: &str, replacement: Option<&[u8]>) -> Vec<u8> {
    let mut source = ZipArchive::new(Cursor::new(archive)).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut found = false;
    for index in 0..source.len() {
        let mut member = source.by_index(index).unwrap();
        let member_name = member.name().to_owned();
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).unwrap();
        if member_name == name {
            found = true;
            if let Some(replacement) = replacement {
                writer.start_file(member_name, options).unwrap();
                writer.write_all(replacement).unwrap();
            }
        } else {
            writer.start_file(member_name, options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
    }
    if !found {
        if let Some(replacement) = replacement {
            writer.start_file(name, options).unwrap();
            writer.write_all(replacement).unwrap();
        }
    }
    writer.finish().unwrap().into_inner()
}

fn legacy_archive(embeddings: &[f32], metadata: &[String]) -> Vec<u8> {
    let stamped = write_voiceprints_npz(
        embeddings,
        metadata,
        &crate::VoiceprintEnvelope::default(),
        &test_encoder(),
    )
    .unwrap();
    rewrite_member(&stamped, "envelope.npy", None)
}

fn envelope(
    identity: &EncoderIdentity,
    version: u32,
    extra: serde_json::Map<String, Value>,
) -> crate::VoiceprintEnvelope {
    crate::VoiceprintEnvelope {
        version,
        encoder: Some(identity.clone()),
        extra,
    }
}

fn npy(descr: &str, shape: &str, payload: &[u8]) -> Vec<u8> {
    let mut header = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
    let padding = (64 - ((10 + header.len() + 1) % 64)) % 64;
    header.push_str(&" ".repeat(padding));
    header.push('\n');
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x93NUMPY");
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn unicode_npy(values: &[String], shape: &str) -> Vec<u8> {
    let width = values
        .iter()
        .map(|value| value.chars().count())
        .max()
        .unwrap_or(0);
    let mut payload = Vec::new();
    for value in values {
        for character in value.chars() {
            payload.extend_from_slice(&(character as u32).to_le_bytes());
        }
        for _ in value.chars().count()..width {
            payload.extend_from_slice(&0_u32.to_le_bytes());
        }
    }
    npy(&format!("<U{width}"), shape, &payload)
}

fn envelope_npy(
    identity: &EncoderIdentity,
    version: u32,
    extra: serde_json::Map<String, Value>,
) -> Vec<u8> {
    let mut object = extra;
    object.insert(
        "format".to_owned(),
        Value::String("solstone-voiceprint-envelope".to_owned()),
    );
    object.insert("version".to_owned(), Value::from(version));
    object.insert("encoder".to_owned(), Value::String(identity.id.clone()));
    object.insert(
        "encoder_sha256".to_owned(),
        Value::String(identity.sha256.clone()),
    );
    object.insert("width".to_owned(), Value::from(identity.width));
    unicode_npy(
        &[serde_json::to_string(&Value::Object(object)).unwrap()],
        "(1,)",
    )
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
fn envelope_round_trips_extras_and_declared_width_without_shape_cross_validation() {
    let identity = EncoderIdentity {
        id: "other".to_owned(),
        sha256: "1111111111111111111111111111111111111111111111111111111111111111".to_owned(),
        width: 777,
    };
    let mut extra = serde_json::Map::new();
    extra.insert("future_field".to_owned(), json!({"kept": true}));
    let bytes = write_voiceprints_npz(
        &embedding(1.0),
        &[r#"{"day":"d"}"#.to_owned()],
        &envelope(&identity, 1, extra.clone()),
        &identity,
    )
    .unwrap();
    let archive = read_voiceprints_npz(&bytes).unwrap();
    assert_eq!(archive.envelope.version, 1);
    assert_eq!(archive.envelope.encoder, Some(identity));
    assert_eq!(archive.envelope.extra, extra);
}

#[test]
fn absent_or_malformed_envelope_is_legacy_not_a_read_error() {
    let metadata = vec![r#"{"day":"d"}"#.to_owned()];
    let legacy = legacy_archive(&embedding(1.0), &metadata);
    assert_eq!(
        read_voiceprints_npz(&legacy).unwrap().envelope,
        crate::VoiceprintEnvelope::default()
    );

    let malformed = rewrite_member(&legacy, "envelope.npy", Some(b"not an npy file"));
    let archive = read_voiceprints_npz(&malformed).unwrap();
    assert_eq!(archive.envelope, crate::VoiceprintEnvelope::default());

    for envelope in [
        unicode_npy(&[], "(0,)"),
        unicode_npy(&["{}".to_owned(), "{}".to_owned(), "{}".to_owned()], "(3,)"),
        unicode_npy(&["{}".to_owned()], "(1, 1)"),
    ] {
        let malformed = rewrite_member(&legacy, "envelope.npy", Some(&envelope));
        assert_eq!(
            read_voiceprints_npz(&malformed).unwrap().envelope,
            crate::VoiceprintEnvelope::default()
        );
    }
}

#[test]
fn missing_required_voiceprint_member_is_still_an_error() {
    let metadata = vec![r#"{"day":"d"}"#.to_owned()];
    let legacy = legacy_archive(&embedding(1.0), &metadata);
    let missing = rewrite_member(&legacy, "metadata.npy", None);
    assert!(matches!(
        read_voiceprints_npz(&missing),
        Err(crate::VoiceprintNpzError::Invalid(message)) if message.contains("metadata.npy")
    ));

    let missing = rewrite_member(&legacy, "embeddings.npy", None);
    assert!(matches!(
        read_voiceprints_npz(&missing),
        Err(crate::VoiceprintNpzError::Invalid(message)) if message.contains("embeddings.npy")
    ));
}

#[test]
fn v0_file_with_zero_rows_round_trips() {
    let legacy = legacy_archive(&[], &[]);

    let archive = read_voiceprints_npz(&legacy).unwrap();

    assert_eq!(archive.rows, 0);
    assert!(archive.embeddings.is_empty());
    assert!(archive.metadata.is_empty());
    assert_eq!(archive.envelope, crate::VoiceprintEnvelope::default());
}

#[test]
fn unknown_member_is_observable_and_refuses_mutation_without_overwrite() {
    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    let bytes = fs::read(&path).unwrap();
    let bytes = rewrite_member(&bytes, "future.npy", Some(b"opaque"));
    fs::write(&path, &bytes).unwrap();
    let archive = read_voiceprints_npz(&bytes).unwrap();
    assert_eq!(archive.unrecognized_members, vec!["future.npy"]);

    let error = save_voiceprints_batch_with_encoder(
        temporary.path(),
        fixture_entity_id(),
        &[VoiceprintItem {
            embedding: embedding(9.0),
            metadata: json!({"day":"d"}),
        }],
        &test_encoder(),
    )
    .unwrap_err();
    assert!(
        matches!(error, VoiceprintOperationError::UnrecognizedNpzMember { member } if member == "future.npy")
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);

    fs::write(
        &path,
        legacy_archive(&embedding(1.0), &[r#"{"day":"d"}"#.to_owned()]),
    )
    .unwrap();
    assert_eq!(
        save_voiceprints_batch(
            temporary.path(),
            fixture_entity_id(),
            &[VoiceprintItem {
                embedding: embedding(2.0),
                metadata: json!({"day":"next"}),
            }]
        )
        .unwrap(),
        1
    );
}

#[test]
fn future_envelope_and_encoder_mismatch_refuse_but_same_encoder_stamps() {
    let temporary = fixture_journal();
    let path = voiceprint_path(temporary.path());
    let future = rewrite_member(
        &fs::read(&path).unwrap(),
        "envelope.npy",
        Some(&envelope_npy(&test_encoder(), 2, serde_json::Map::new())),
    );
    fs::write(&path, &future).unwrap();
    let error = rewrite_voiceprint_metadata_with_encoder(
        temporary.path(),
        fixture_entity_id(),
        &test_encoder(),
        |_| 1,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        VoiceprintOperationError::UnsupportedEnvelopeVersion {
            found: 2,
            max_supported: 1
        }
    ));

    let other = EncoderIdentity {
        id: "other".to_owned(),
        sha256: "2222222222222222222222222222222222222222222222222222222222222222".to_owned(),
        width: 256,
    };
    let stamped = write_voiceprints_npz(
        &embedding(1.0),
        &[r#"{"day":"d"}"#.to_owned()],
        &crate::VoiceprintEnvelope::default(),
        &other,
    )
    .unwrap();
    fs::write(&path, stamped).unwrap();
    let archive = load_entity_voiceprints_file(temporary.path(), fixture_entity_id()).unwrap();
    assert!(!archive.matches_running_encoder(&test_encoder()));
    assert!(archive.matches_running_encoder(&other));
    let before = fs::read(&path).unwrap();
    let error = save_voiceprints_batch_with_encoder(
        temporary.path(),
        fixture_entity_id(),
        &[VoiceprintItem {
            embedding: embedding(3.0),
            metadata: json!({"day":"new"}),
        }],
        &test_encoder(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        VoiceprintOperationError::EncoderIdentityMismatch { .. }
    ));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        save_voiceprints_batch_with_encoder(
            temporary.path(),
            fixture_entity_id(),
            &[VoiceprintItem {
                embedding: embedding(3.0),
                metadata: json!({"day":"new"})
            }],
            &other,
        )
        .unwrap(),
        1
    );

    let mut extra = serde_json::Map::new();
    extra.insert("future".to_owned(), json!(["preserve"]));
    let bytes = write_voiceprints_npz(
        &embedding(1.0),
        &[r#"{"day":"d"}"#.to_owned()],
        &envelope(&other, 1, extra.clone()),
        &other,
    )
    .unwrap();
    fs::write(&path, bytes).unwrap();
    save_voiceprints_batch_with_encoder(
        temporary.path(),
        fixture_entity_id(),
        &[VoiceprintItem {
            embedding: embedding(4.0),
            metadata: json!({"day":"again"}),
        }],
        &other,
    )
    .unwrap();
    assert_eq!(
        load_entity_voiceprints_file(temporary.path(), fixture_entity_id())
            .unwrap()
            .envelope
            .extra,
        extra
    );
}

#[test]
fn direct_mutations_stamp_legacy_with_the_caller_identity() {
    let temporary = fixture_journal();
    let caller = EncoderIdentity {
        id: "caller-supplied".to_owned(),
        sha256: "3333333333333333333333333333333333333333333333333333333333333333".to_owned(),
        width: 256,
    };
    assert_eq!(
        save_voiceprints_batch_with_encoder(
            temporary.path(),
            fixture_entity_id(),
            &[VoiceprintItem {
                embedding: embedding(8.0),
                metadata: json!({"day":"new"})
            }],
            &caller,
        )
        .unwrap(),
        1
    );
    let archive = load_entity_voiceprints_file(temporary.path(), fixture_entity_id()).unwrap();
    assert_eq!(archive.envelope.encoder, Some(caller));
    assert_eq!(archive.rows, 8);
}

#[test]
fn rewrite_and_remove_stamp_legacy_with_the_caller_identity() {
    let caller = EncoderIdentity {
        id: "rewrite-remove".to_owned(),
        sha256: "4444444444444444444444444444444444444444444444444444444444444444".to_owned(),
        width: 256,
    };
    let temporary = fixture_journal();
    assert_eq!(
        rewrite_voiceprint_metadata_with_encoder(
            temporary.path(),
            fixture_entity_id(),
            &caller,
            |rows| {
                rows[0]["rewritten"] = Value::Bool(true);
                1
            },
        )
        .unwrap(),
        1
    );
    assert_eq!(
        load_entity_voiceprints_file(temporary.path(), fixture_entity_id())
            .unwrap()
            .envelope
            .encoder,
        Some(caller.clone())
    );

    let temporary = fixture_journal();
    let first = fixture()["metadata"]["null_key"].clone();
    assert_eq!(
        remove_voiceprints_by_key_with_encoder(
            temporary.path(),
            fixture_entity_id(),
            &[VoiceprintRemoval {
                key: json!({
                    "day": first["day"],
                    "segment_key": first["segment_key"],
                    "sentence_id": first["sentence_id"],
                }),
                expected_metadata: Some(first),
            }],
            &caller,
        )
        .unwrap()
        .removed_count,
        1
    );
    assert_eq!(
        load_entity_voiceprints_file(temporary.path(), fixture_entity_id())
            .unwrap()
            .envelope
            .encoder,
        Some(caller)
    );
}

#[test]
fn non_256_embedding_width_has_typed_error() {
    let metadata = vec![r#"{"day":"d"}"#.to_owned()];
    let bytes = legacy_archive(&embedding(1.0), &metadata);
    let malformed = rewrite_member(
        &bytes,
        "embeddings.npy",
        Some(&npy("<f4", "(1, 255)", &[0; 255 * 4])),
    );
    assert!(matches!(
        read_voiceprints_npz(&malformed),
        Err(crate::VoiceprintNpzError::EmbeddingWidth { found: 255 })
    ));
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
    fs::write(path, legacy_archive(&embeddings, &metadata)).unwrap();
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
