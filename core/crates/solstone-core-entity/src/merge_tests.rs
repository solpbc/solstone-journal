// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use super::store::merge::commit_entity_merge_with_injector as commit_entity_merge_with_injector_with_encoder;
use super::store::merge::merge_facets;
use super::store::merge::merge_voiceprints as merge_voiceprints_with_encoder;
use super::store::merge::{dedupe_akas, dedupe_emails, dedupe_observations};
use super::store::merge_payload::{list_entity_merge_payload_ids, load_entity_merge_payload};
use super::store::voiceprints::{read_voiceprints_npz, write_voiceprints_npz};
use crate::{
    EncoderIdentity, EntityLifecycleError, EntityMergeError, EntityMergeOptions,
    EntityTrustLockError, EntityWriteError, LockError,
    commit_entity_merge as commit_entity_merge_with_encoder, guard_restore_does_not_cross_merge,
    hold_entity_trust_lock, preview_entity_merge, read_entity_identity, read_visible_history,
    save_entity_identity,
};
use serde_json::json;
use solstone_core_journal_io::{LockOptions, hold_lock};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

static NEXT_VOICEPRINT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
type MergeInjector<'a> = &'a (dyn Fn(&str, usize) -> bool + 'static);

fn test_encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test-encoder".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

fn merge_voiceprints(
    journal: &std::path::Path,
    source_id: &str,
    target_id: &str,
) -> Result<super::store::merge::VoiceprintMergeStats, EntityMergeError> {
    merge_voiceprints_with_encoder(
        journal,
        source_id,
        target_id,
        &test_encoder(),
        LockOptions::default(),
    )
}

fn commit_entity_merge(
    journal: &std::path::Path,
    source_id: &str,
    target_id: &str,
    options: EntityMergeOptions,
) -> Result<crate::EntityMergeReport, EntityMergeError> {
    commit_entity_merge_with_encoder(journal, source_id, target_id, options, &test_encoder())
}

fn commit_entity_merge_with_injector(
    journal: &std::path::Path,
    source_id: &str,
    target_id: &str,
    options: EntityMergeOptions,
    injector: Option<MergeInjector<'_>>,
) -> Result<crate::EntityMergeReport, EntityMergeError> {
    commit_entity_merge_with_injector_with_encoder(
        journal,
        source_id,
        target_id,
        options,
        &test_encoder(),
        injector,
    )
}

fn voiceprint_journal() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "solstone-voiceprint-{}-{}",
        std::process::id(),
        NEXT_VOICEPRINT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    fs::canonicalize(path).unwrap()
}

#[cfg(unix)]
#[test]
fn merge_and_undo_accept_an_aliased_journal_root() {
    let journal = voiceprint_journal();
    let alias = journal.with_extension("alias");
    std::os::unix::fs::symlink(&journal, &alias).unwrap();
    for id in ["source", "target"] {
        save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
        let facet = journal.join(format!("facets/work/entities/{id}"));
        fs::create_dir_all(&facet).unwrap();
        fs::write(
            facet.join("entity.json"),
            json!({"entity_id":id}).to_string(),
        )
        .unwrap();
        fs::write(
            facet.join("observations.jsonl"),
            json!({"content":id}).to_string(),
        )
        .unwrap();
    }
    let merged =
        commit_entity_merge(&alias, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(!journal.join("entities/source").exists());
    crate::undo_entity_merge(&alias, &merged.merge_id, serde_json::Value::Null).unwrap();
    assert!(journal.join("entities/source/entity.json").is_file());
    fs::remove_file(alias).unwrap();
    fs::remove_dir_all(journal).unwrap();
}

#[cfg(unix)]
#[test]
fn source_namespace_sync_failure_keeps_uncommitted_recovery_and_retry_succeeds() {
    for relative in [".", "entities", "entities/target/history/private", "logs"] {
        let journal = voiceprint_journal();
        for id in ["source", "target"] {
            save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
        }
        let error = super::store::with_source_sync_failure(relative, || {
            commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("source directory sync failure"),
            "{relative}: {error}"
        );
        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(journal.join("health/entity-merge-recovery/state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["source_committed"], false);
        assert_ne!(state["finished"], true);
        assert!(
            journal
                .join("health/entity-merge-recovery/00000000.json")
                .is_file()
        );
        assert!(!journal.join("entities/source").exists());
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
        assert!(!journal.join("health/entity-merge-recovery").exists());
        fs::remove_dir_all(journal).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn restored_sources_keep_before_images_when_namespace_sync_fails() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
    }
    let before = solstone_core_journal_io::capture_snapshot(&journal, "entities/source").unwrap();
    let error = super::store::with_source_sync_failure("entities/source", || {
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, _| phase == "cleanup"),
        )
    })
    .unwrap_err();
    assert!(
        error.to_string().contains("source directory sync failure"),
        "{error}"
    );
    assert_eq!(
        solstone_core_journal_io::capture_snapshot(&journal, "entities/source").unwrap(),
        before
    );
    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(journal.join("health/entity-merge-recovery/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["source_committed"], false);
    assert_ne!(state["finished"], true);
    assert!(
        journal
            .join("health/entity-merge-recovery/00000000.json")
            .is_file()
    );
    fs::remove_dir_all(journal).unwrap();
}
fn row(value: f32) -> Vec<f32> {
    vec![value; 256]
}
fn metadata(key: &str) -> String {
    format!(r#"{{"day":"d","segment_key":"s","source":"x","sentence_id":"{key}"}}"#)
}
fn write_voiceprints(
    journal: &std::path::Path,
    id: &str,
    rows: Vec<Vec<f32>>,
    metadata: Vec<String>,
) {
    write_voiceprints_with_identity(journal, id, rows, metadata, &test_encoder(), true);
}

fn seed_self_resolving_identity(journal: &Path, id: &str) {
    let path = journal.join(format!("entities/{id}/entity.json"));
    if path.exists() {
        return;
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!(r#"{{"id":"{id}"}}"#)).unwrap();
}

fn write_voiceprints_at(
    journal: &Path,
    directory: &str,
    rows: Vec<Vec<f32>>,
    metadata: Vec<String>,
    identity: &EncoderIdentity,
) {
    let path = journal.join(format!("entities/{directory}/voiceprints.npz"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let embeddings = rows.into_iter().flatten().collect::<Vec<_>>();
    let bytes = write_voiceprints_npz(
        &embeddings,
        &metadata,
        &super::store::voiceprints::VoiceprintEnvelope::default(),
        identity,
    )
    .unwrap();
    fs::write(path, bytes).unwrap();
}

fn write_voiceprints_with_identity(
    journal: &std::path::Path,
    id: &str,
    rows: Vec<Vec<f32>>,
    metadata: Vec<String>,
    identity: &EncoderIdentity,
    legacy: bool,
) {
    seed_self_resolving_identity(journal, id);
    let path = journal.join(format!("entities/{id}/voiceprints.npz"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let embeddings = rows.into_iter().flatten().collect::<Vec<_>>();
    let bytes = write_voiceprints_npz(
        &embeddings,
        &metadata,
        &super::store::voiceprints::VoiceprintEnvelope::default(),
        identity,
    )
    .unwrap();
    fs::write(
        path,
        if legacy {
            rewrite_member(&bytes, "envelope.npy", None)
        } else {
            bytes
        },
    )
    .unwrap();
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
    if !found && let Some(replacement) = replacement {
        writer.start_file(name, options).unwrap();
        writer.write_all(replacement).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn envelope_npy(identity: &EncoderIdentity, version: u32) -> Vec<u8> {
    let serialized = json!({
        "format": "solstone-voiceprint-envelope",
        "version": version,
        "encoder": identity.id,
        "encoder_sha256": identity.sha256,
        "width": identity.width,
    })
    .to_string();
    let width = serialized.chars().count();
    let mut payload = Vec::new();
    for character in serialized.chars() {
        payload.extend_from_slice(&(character as u32).to_le_bytes());
    }
    let mut header = format!("{{'descr': '<U{width}', 'fortran_order': False, 'shape': (1,), }}");
    let padding = (64 - ((10 + header.len() + 1) % 64)) % 64;
    header.push_str(&" ".repeat(padding));
    header.push('\n');
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x93NUMPY");
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}
fn read_rows(journal: &std::path::Path, id: &str) -> super::store::voiceprints::VoiceprintArchive {
    read_voiceprints_npz(&fs::read(journal.join(format!("entities/{id}/voiceprints.npz"))).unwrap())
        .unwrap()
}

#[test]
fn voiceprints_copy_source_when_target_is_missing() {
    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("1")]);
    seed_self_resolving_identity(&journal, "target");
    assert_eq!(
        merge_voiceprints(&journal, "source", "target")
            .unwrap()
            .added,
        1
    );
    let target = read_rows(&journal, "target");
    assert_eq!(target.metadata, vec![metadata("1")]);
    assert_eq!(target.envelope.encoder, Some(test_encoder()));
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn voiceprint_merge_carries_matching_known_identity_and_refuses_mismatch() {
    let journal = voiceprint_journal();
    let source = EncoderIdentity {
        id: "source".to_owned(),
        sha256: "1111111111111111111111111111111111111111111111111111111111111111".to_owned(),
        width: 256,
    };
    let target = EncoderIdentity {
        id: "target".to_owned(),
        sha256: "2222222222222222222222222222222222222222222222222222222222222222".to_owned(),
        width: 256,
    };
    write_voiceprints_with_identity(
        &journal,
        "source",
        vec![row(2.0)],
        vec![metadata("1")],
        &source,
        false,
    );
    write_voiceprints_with_identity(
        &journal,
        "target",
        vec![row(3.0)],
        vec![metadata("2")],
        &target,
        false,
    );
    let error = merge_voiceprints(&journal, "source", "target").unwrap_err();
    assert!(matches!(
        error,
        EntityMergeError::VoiceprintEncoderMismatch {
            source_entity_id,
            target_entity_id,
            source_encoder_id,
            target_encoder_id,
        } if source_entity_id == "source"
            && target_entity_id == "target"
            && source_encoder_id == "source"
            && target_encoder_id == "target"
    ));
    fs::remove_dir_all(&journal).unwrap();

    let journal = voiceprint_journal();
    write_voiceprints_with_identity(
        &journal,
        "source",
        vec![row(2.0)],
        vec![metadata("1")],
        &source,
        false,
    );
    write_voiceprints_with_identity(
        &journal,
        "target",
        vec![row(3.0)],
        vec![metadata("2")],
        &source,
        false,
    );
    merge_voiceprints(&journal, "source", "target").unwrap();
    assert_eq!(read_rows(&journal, "target").envelope.encoder, Some(source));
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn commit_preflights_voiceprint_encoder_mismatch() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let source = EncoderIdentity {
        id: "source".to_owned(),
        sha256: "1111111111111111111111111111111111111111111111111111111111111111".to_owned(),
        width: 256,
    };
    let target = EncoderIdentity {
        id: "target".to_owned(),
        sha256: "2222222222222222222222222222222222222222222222222222222222222222".to_owned(),
        width: 256,
    };
    write_voiceprints_with_identity(
        &journal,
        "source",
        vec![row(2.0)],
        vec![metadata("1")],
        &source,
        false,
    );
    write_voiceprints_with_identity(
        &journal,
        "target",
        vec![row(3.0)],
        vec![metadata("2")],
        &target,
        false,
    );

    let error = commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
        .unwrap_err();

    assert!(matches!(
        error,
        EntityMergeError::VoiceprintEncoderMismatch {
            source_entity_id,
            target_entity_id,
            source_encoder_id,
            target_encoder_id,
        } if source_entity_id == "source"
            && target_entity_id == "target"
            && source_encoder_id == "source"
            && target_encoder_id == "target"
    ));
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn voiceprint_merge_refuses_unknown_or_future_members_and_merges_negative_twin() {
    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("1")]);
    seed_self_resolving_identity(&journal, "target");
    let source_path = journal.join("entities/source/voiceprints.npz");
    let unknown = rewrite_member(&fs::read(&source_path).unwrap(), "future.npy", Some(b"x"));
    fs::write(&source_path, unknown).unwrap();
    assert!(matches!(
        merge_voiceprints(&journal, "source", "target"),
        Err(EntityMergeError::Refused(message)) if message.contains("unrecognized member")
    ));
    fs::remove_dir_all(&journal).unwrap();

    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("1")]);
    seed_self_resolving_identity(&journal, "target");
    let source_path = journal.join("entities/source/voiceprints.npz");
    let future = rewrite_member(
        &fs::read(&source_path).unwrap(),
        "envelope.npy",
        Some(&envelope_npy(&test_encoder(), 2)),
    );
    fs::write(&source_path, future).unwrap();
    assert!(matches!(
        merge_voiceprints(&journal, "source", "target"),
        Err(EntityMergeError::Refused(message)) if message.contains("version 2")
    ));
    fs::remove_dir_all(&journal).unwrap();

    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("1")]);
    seed_self_resolving_identity(&journal, "target");
    assert_eq!(
        merge_voiceprints(&journal, "source", "target")
            .unwrap()
            .added,
        1
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn voiceprints_append_without_overlap() {
    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("2")]);
    write_voiceprints(&journal, "target", vec![row(3.0)], vec![metadata("1")]);
    assert_eq!(
        merge_voiceprints(&journal, "source", "target")
            .unwrap()
            .target_total,
        2
    );
    assert_eq!(
        read_rows(&journal, "target").metadata,
        vec![metadata("1"), metadata("2")]
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn voiceprints_skip_shared_target_key() {
    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("1")]);
    write_voiceprints(&journal, "target", vec![row(3.0)], vec![metadata("1")]);
    let stats = merge_voiceprints(&journal, "source", "target").unwrap();
    assert_eq!(
        (stats.added, stats.skipped_duplicate, stats.target_total),
        (0, 1, 1)
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn voiceprints_skip_internal_duplicate_key() {
    let journal = voiceprint_journal();
    seed_self_resolving_identity(&journal, "target");
    write_voiceprints(
        &journal,
        "source",
        vec![row(2.0), row(3.0)],
        vec![metadata("1"), metadata("1")],
    );
    let stats = merge_voiceprints(&journal, "source", "target").unwrap();
    assert_eq!((stats.added, stats.skipped_duplicate), (1, 1));
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn voiceprints_do_nothing_without_source_file() {
    let journal = voiceprint_journal();
    seed_self_resolving_identity(&journal, "source");
    write_voiceprints(&journal, "target", vec![row(3.0)], vec![metadata("1")]);
    assert_eq!(
        merge_voiceprints(&journal, "source", "target")
            .unwrap()
            .target_total,
        1
    );
    assert_eq!(read_rows(&journal, "target").metadata, vec![metadata("1")]);
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn voiceprints_skip_degenerate_rows_without_duplicate_count() {
    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(0.0)], vec![metadata("1")]);
    seed_self_resolving_identity(&journal, "target");
    let stats = merge_voiceprints(&journal, "source", "target").unwrap();
    assert_eq!(
        (stats.added, stats.skipped_duplicate, stats.target_total),
        (0, 0, 0)
    );
    assert!(!journal.join("entities/target/voiceprints.npz").exists());
    fs::remove_dir_all(journal).unwrap();
}

fn seed_identity_at(journal: &Path, directory: &str, effective_id: &str) {
    let path = journal.join(format!("entities/{directory}/entity.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!(r#"{{"id":"{effective_id}"}}"#)).unwrap();
}

fn short_lock_options(timeout: Duration) -> LockOptions {
    LockOptions {
        timeout,
        ..LockOptions::default()
    }
}

#[test]
fn merge_voiceprints_reads_remapped_source_archive() {
    let journal = voiceprint_journal();
    seed_identity_at(&journal, "dir-s", "source");
    write_voiceprints_at(
        &journal,
        "dir-s",
        vec![row(2.0)],
        vec![metadata("1")],
        &test_encoder(),
    );
    seed_self_resolving_identity(&journal, "target");
    assert_eq!(
        merge_voiceprints(&journal, "source", "target")
            .unwrap()
            .added,
        1
    );
    assert_eq!(read_rows(&journal, "target").metadata, vec![metadata("1")]);
    assert!(!journal.join("entities/source/voiceprints.npz").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_voiceprints_writes_remapped_target_archive() {
    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("S")]);
    seed_identity_at(&journal, "dir-t", "target");
    write_voiceprints_at(
        &journal,
        "dir-t",
        vec![row(3.0)],
        vec![metadata("U")],
        &test_encoder(),
    );
    let resolved = journal.join("entities/dir-t/voiceprints.npz");
    let unresolved = journal.join("entities/target/voiceprints.npz");
    assert!(!unresolved.exists());
    let stats = merge_voiceprints(&journal, "source", "target").unwrap();
    assert_eq!(stats.added, 1);
    let archive = read_voiceprints_npz(&fs::read(&resolved).unwrap()).unwrap();
    assert_eq!(archive.metadata, vec![metadata("U"), metadata("S")]);
    assert!(archive.embeddings[..256].iter().all(|value| *value == 3.0));
    assert!(!unresolved.exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_voiceprints_times_out_on_resolved_target_lock() {
    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("S")]);
    seed_identity_at(&journal, "dir-t", "target");
    write_voiceprints_at(
        &journal,
        "dir-t",
        vec![row(3.0)],
        vec![metadata("U")],
        &test_encoder(),
    );
    let resolved = journal.join("entities/dir-t/voiceprints.npz");
    let before = fs::read(&resolved).unwrap();
    let _lock = hold_lock(&resolved, short_lock_options(Duration::from_millis(100))).unwrap();
    let error = merge_voiceprints_with_encoder(
        &journal,
        "source",
        "target",
        &test_encoder(),
        short_lock_options(Duration::from_millis(200)),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        EntityMergeError::Write(EntityWriteError::TrustLock(EntityTrustLockError::Lock(
            LockError::Timeout(_)
        )))
    ));
    assert_eq!(fs::read(&resolved).unwrap(), before);
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_voiceprints_ignores_unresolved_target_lock() {
    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("S")]);
    seed_identity_at(&journal, "dir-t", "target");
    write_voiceprints_at(
        &journal,
        "dir-t",
        vec![row(3.0)],
        vec![metadata("U")],
        &test_encoder(),
    );
    let unresolved = journal.join("entities/target/voiceprints.npz");
    let _lock = hold_lock(&unresolved, LockOptions::default()).unwrap();
    assert_eq!(
        merge_voiceprints(&journal, "source", "target")
            .unwrap()
            .added,
        1
    );
    let archive =
        read_voiceprints_npz(&fs::read(journal.join("entities/dir-t/voiceprints.npz")).unwrap())
            .unwrap();
    assert_eq!(archive.metadata, vec![metadata("U"), metadata("S")]);
    assert!(!unresolved.exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_voiceprints_requires_identity_map_entry() {
    let journal = voiceprint_journal();
    write_voiceprints_at(
        &journal,
        "source",
        vec![row(2.0)],
        vec![metadata("1")],
        &test_encoder(),
    );
    let error = merge_voiceprints(&journal, "source", "target").unwrap_err();
    assert!(matches!(
        error,
        EntityMergeError::Lifecycle(EntityLifecycleError::EntityNotFound { entity_id, .. })
            if entity_id == "source"
    ));
    assert!(!journal.join("entities/target/voiceprints.npz").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_voiceprints_takes_lock_before_skipping_write() {
    let journal = voiceprint_journal();
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("1")]);
    write_voiceprints(&journal, "target", vec![row(3.0)], vec![metadata("1")]);
    let target_path = journal.join("entities/target/voiceprints.npz");
    let before = fs::read(&target_path).unwrap();
    let stats = merge_voiceprints(&journal, "source", "target").unwrap();
    assert_eq!(stats.added, 0);
    assert_eq!(fs::read(&target_path).unwrap(), before);
    assert!(
        journal
            .join("entities/target/voiceprints.npz.lock")
            .exists()
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn facets_move_relationship_and_observations() {
    let journal = voiceprint_journal();
    let dir = journal.join("facets/work/entities/source");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("entity.json"),
        br#"{"entity_id":"source","attached_at":"2026-01-01"}"#,
    )
    .unwrap();
    fs::write(
        dir.join("observations.jsonl"),
        b"{\"content\":\"note\",\"observed_at\":\"x\"}\n",
    )
    .unwrap();
    assert_eq!(
        merge_facets(&journal, "source", "target", None, None)
            .unwrap()
            .moved_count,
        1
    );
    let relationship: serde_json::Value = serde_json::from_slice(
        &fs::read(journal.join("facets/work/entities/source/entity.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(relationship["entity_id"], "target");
    assert_eq!(
        fs::read_to_string(journal.join("facets/work/entities/source/observations.jsonl")).unwrap(),
        "{\"content\":\"note\",\"observed_at\":\"x\"}\n"
    );
    assert!(!journal.join("facets/work/entities/target").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn committed_merge_payload_records_facet_inverse_entries() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let moved = journal.join("facets/moved/entities/source");
    fs::create_dir_all(&moved).unwrap();
    fs::write(
        moved.join("entity.json"),
        br#"{"entity_id":"source","description":"moved"}"#,
    )
    .unwrap();

    let source_merged = journal.join("facets/merged/entities/source");
    let target_merged = journal.join("facets/merged/entities/target");
    fs::create_dir_all(&source_merged).unwrap();
    fs::create_dir_all(&target_merged).unwrap();
    fs::write(
        source_merged.join("entity.json"),
        br#"{"entity_id":"source","attached_at":"2026-01-01"}"#,
    )
    .unwrap();
    let target_before = json!({
        "entity_id": "target",
        "attached_at": "2026-02-01",
        "description": "target description"
    });
    fs::write(
        target_merged.join("entity.json"),
        serde_json::to_vec(&target_before).unwrap(),
    )
    .unwrap();

    let report =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let payload = load_entity_merge_payload(&journal, "target", &report.merge_id).unwrap();
    let entries = payload["manifest"]["facets"]["entries"].as_array().unwrap();
    assert!(entries.iter().any(|entry| {
        entry["facet"] == "moved"
            && entry["kind"] == "relink"
            && entry["source_dir"] == "source"
            && entry["target_dir"] == "source"
    }));
    assert!(entries.iter().any(|entry| {
        entry["facet"] == "merged"
            && entry["kind"] == "merge"
            && entry["target_before"] == target_before
    }));
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn committed_merge_payload_records_identity_support() {
    let journal = voiceprint_journal();
    let source = json!({
        "id": "source",
        "name": "Source",
        "aka": ["New Alias", "Existing Alias"],
        "emails": ["new@example.test", "existing@example.test"],
        "title": "Engineer"
    });
    let target = json!({
        "id": "target",
        "name": "Target",
        "aka": ["Existing Alias"],
        "emails": ["existing@example.test"]
    });
    save_entity_identity(&journal, "source", &source, None).unwrap();
    save_entity_identity(&journal, "target", &target, None).unwrap();

    let report =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let payload = load_entity_merge_payload(&journal, "target", &report.merge_id).unwrap();
    let identity = &payload["manifest"]["identity"];
    assert!(
        identity["aka_support"]
            .as_array()
            .unwrap()
            .contains(&json!({"key":"new alias","target_preexisting":false}))
    );
    assert!(
        identity["aka_support"]
            .as_array()
            .unwrap()
            .contains(&json!({"key":"existing alias","target_preexisting":true}))
    );
    assert!(
        identity["email_support"]
            .as_array()
            .unwrap()
            .contains(&json!({"key":"new@example.test","target_preexisting":false}))
    );
    assert!(
        identity["email_support"]
            .as_array()
            .unwrap()
            .contains(&json!({"key":"existing@example.test","target_preexisting":true}))
    );
    assert!(
        identity["scalar_support"]
            .as_array()
            .unwrap()
            .contains(&json!({
                "key":"title",
                "target_prevalue":null,
                "target_prevalue_missing":true,
                "source_value":"Engineer"
            }))
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn email_dedup_keeps_first_seen_order() {
    assert_eq!(
        dedupe_emails(
            &["z@example.test".to_owned()],
            &["a@example.test".to_owned()]
        ),
        ["z@example.test", "a@example.test"]
    );
}

#[test]
fn alias_dedup_sorts_by_lowercase() {
    assert_eq!(
        dedupe_akas(&["Zulu".to_owned(), "alpha".to_owned()]),
        ["alpha", "Zulu"]
    );
}

#[test]
fn dedup_keeps_first_case_variant_and_does_not_normalize() {
    assert_eq!(
        dedupe_akas(&[
            "Jane".to_owned(),
            "jane".to_owned(),
            " Jane".to_owned(),
            "Straße".to_owned(),
            "STRASSE".to_owned(),
            "İ".to_owned(),
            "i".to_owned(),
            "ẞ".to_owned(),
            "ß".to_owned()
        ]),
        [" Jane", "i", "İ", "Jane", "STRASSE", "Straße", "ẞ"]
    );
}

#[test]
fn dedup_survives_strasse_case_variants() {
    assert_eq!(
        dedupe_akas(&["Straße".to_owned(), "STRASSE".to_owned()]),
        ["STRASSE", "Straße"]
    );
    assert_eq!(
        dedupe_emails(&["Straße".to_owned()], &["STRASSE".to_owned()]),
        ["Straße", "STRASSE"]
    );
}

#[test]
fn dedup_survives_dotted_i_variants() {
    assert_eq!(
        dedupe_akas(&["İstanbul".to_owned(), "istanbul".to_owned()]),
        ["istanbul", "İstanbul"]
    );
    assert_eq!(
        dedupe_emails(&["İstanbul".to_owned()], &["istanbul".to_owned()]),
        ["İstanbul", "istanbul"]
    );
}

#[test]
fn dedup_survives_nfc_nfd_variants() {
    assert_eq!(
        dedupe_akas(&["café".to_owned(), "cafe\u{301}".to_owned()]),
        ["cafe\u{301}", "café"]
    );
    assert_eq!(
        dedupe_emails(&["café".to_owned()], &["cafe\u{301}".to_owned()]),
        ["café", "cafe\u{301}"]
    );
}

#[test]
fn dedup_collapses_sharp_s_variants() {
    assert_eq!(dedupe_akas(&["ẞ".to_owned(), "ß".to_owned()]), ["ẞ"]);
    assert_eq!(dedupe_emails(&["ẞ".to_owned()], &["ß".to_owned()]), ["ẞ"]);
}

#[test]
fn dedup_does_not_collapse_whitespace_padded_variants() {
    assert_eq!(
        dedupe_akas(&[" Jane".to_owned(), "Jane".to_owned()]),
        [" Jane", "Jane"]
    );
    assert_eq!(
        dedupe_emails(&[" Jane".to_owned()], &["Jane".to_owned()]),
        [" Jane", "Jane"]
    );
}

#[test]
fn observation_dedup_prefers_target_entries() {
    assert_eq!(
        dedupe_observations(
            &[json!({"content":"one","observed_at":"x"})],
            &[json!({"content":"one","observed_at":"x"})]
        ),
        [json!({"content":"one","observed_at":"x"})]
    );
}

#[test]
fn commit_cleanup_removes_touched_source_facet_and_discovery_cache() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let source = journal.join("facets/work/entities/source");
    let target = journal.join("facets/work/entities/target");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(source.join("entity.json"), br#"{"entity_id":"source"}"#).unwrap();
    fs::write(target.join("entity.json"), br#"{"entity_id":"target"}"#).unwrap();
    let discovery = journal.join("awareness/discovery_clusters.json");
    fs::create_dir_all(discovery.parent().unwrap()).unwrap();
    fs::write(&discovery, b"{}").unwrap();
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(!source.exists());
    assert!(!discovery.exists());
    fs::remove_dir_all(journal).unwrap();
}

fn commit_segment_merge(journal: &std::path::Path) {
    for id in ["source", "target"] {
        save_entity_identity(
            journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    commit_entity_merge(journal, "source", "target", EntityMergeOptions::default()).unwrap();
}

#[test]
fn commit_rewrites_segment_speaker_labels() {
    let journal = voiceprint_journal();
    let path = journal.join("chronicle/20260102/080000_300/talents/speaker_labels.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, br#"{"labels":[{"speaker":"source"}]}"#).unwrap();
    commit_segment_merge(&journal);
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["labels"][0]["speaker"], "target");
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn commit_rewrites_segment_speaker_corrections() {
    let journal = voiceprint_journal();
    let path = journal.join("chronicle/20260102/080000_300/talents/speaker_corrections.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        br#"{"corrections":[{"original_speaker":"source","corrected_speaker":"other"}]}"#,
    )
    .unwrap();
    commit_segment_merge(&journal);
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["corrections"][0]["original_speaker"], "target");
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn commit_rewrites_activity_entity_references() {
    let journal = voiceprint_journal();
    let path = journal.join("facets/work/activities/20260102.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{\"id\":\"activity\",\"active_entities\":[\"source\"],\"commitments\":[{\"owner_entity_id\":\"source\",\"counterparty_entity_id\":\"someone-else\"}]}\n").unwrap();
    commit_segment_merge(&journal);
    let row: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(row["active_entities"][0], "target");
    assert_eq!(row["commitments"][0]["owner_entity_id"], "target");
    assert_eq!(
        row["commitments"][0]["counterparty_entity_id"],
        "someone-else"
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn commit_remaps_other_entity_observation_relation() {
    let journal = voiceprint_journal();
    let path = journal.join("facets/work/entities/other/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{\"relation\":{\"target_entity_id\":\"source\"}}\n").unwrap();
    commit_segment_merge(&journal);
    let row: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(row["relation"]["target_entity_id"], "target");
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn commit_payload_records_history_sequence() {
    let journal = voiceprint_journal();
    commit_segment_merge(&journal);
    let merge_id = list_entity_merge_payload_ids(&journal, "target")
        .unwrap()
        .pop()
        .unwrap();
    let payload = load_entity_merge_payload(&journal, "target", &merge_id).unwrap();
    let expected = read_visible_history(&journal, "target")
        .unwrap()
        .last()
        .unwrap()
        .sequence()
        .unwrap();
    assert_eq!(
        payload["commit_seq"].as_i64().map(i128::from),
        Some(expected)
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn commit_records_matching_payload_and_audit_counts() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("count")]);
    let facet = journal.join("facets/work/entities/source");
    fs::create_dir_all(&facet).unwrap();
    fs::write(facet.join("entity.json"), br#"{"entity_id":"source"}"#).unwrap();
    fs::write(
        facet.join("observations.jsonl"),
        b"{\"content\":\"source observation\",\"observed_at\":\"2026-01-01\"}\n",
    )
    .unwrap();
    let target_facet = journal.join("facets/work/entities/target");
    fs::create_dir_all(&target_facet).unwrap();
    fs::write(
        target_facet.join("entity.json"),
        br#"{"entity_id":"target"}"#,
    )
    .unwrap();
    fs::write(
        target_facet.join("observations.jsonl"),
        b"{\"content\":\"target observation\",\"observed_at\":\"2026-01-01\"}\n",
    )
    .unwrap();
    let activity = journal.join("facets/work/activities/20260102.jsonl");
    fs::create_dir_all(activity.parent().unwrap()).unwrap();
    fs::write(&activity, b"{\"active_entities\":[\"source\"]}\n").unwrap();
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let audit: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(journal.join("logs/entity-merges.jsonl")).unwrap(),
    )
    .unwrap();
    assert!(audit["ts"].as_u64().is_some());
    assert_eq!(audit["source_display_name"], "source");
    assert_eq!(audit["target_display_name"], "target");
    assert_eq!(audit["principal_transferred"], false);
    assert_eq!(audit["caller"], serde_json::Value::Null);
    assert!(audit.get("kind").is_none());
    assert!(audit["counts"]["facets"]["merged"].as_u64().unwrap() > 0);
    assert_eq!(audit["counts"]["facets"]["observations_appended"], 1);
    assert!(audit["counts"]["voiceprints"]["added"].as_u64().unwrap() > 0);
    assert!(
        audit["counts"]["activities"]["records_rewritten"]
            .as_u64()
            .unwrap()
            > 0
    );
    let merge_id = list_entity_merge_payload_ids(&journal, "target")
        .unwrap()
        .pop()
        .unwrap();
    let payload = load_entity_merge_payload(&journal, "target", &merge_id).unwrap();
    assert_eq!(payload["result_counts"], audit["counts"]);
    assert_eq!(
        payload["manifest"]["voiceprints"]["support"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        payload["manifest"]["voiceprints"]["support"][0]["key"]["sentence_id"],
        "count"
    );
    assert_eq!(
        payload["manifest"]["voiceprints"]["support"][0]["target_preexisting"],
        false
    );
    assert_eq!(
        payload["manifest"]["voiceprints"]["support"][0]["added"],
        true
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn facets_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let sources = [
        journal.join("facets/work/entities/source"),
        journal.join("facets/personal/entities/source"),
    ];
    for source in &sources {
        fs::create_dir_all(source).unwrap();
        fs::write(source.join("entity.json"), br#"{"entity_id":"source"}"#).unwrap();
    }
    let result = commit_entity_merge_with_injector(
        &journal,
        "source",
        "target",
        EntityMergeOptions::default(),
        Some(&|phase: &str, artifact_index| phase == "facets" && artifact_index == 0),
    );
    assert!(result.is_err());
    assert!(sources.iter().all(|source| source.exists()));
    assert!(journal.join("entities/source").exists());
    assert!(!journal.join("facets/work/entities/target").exists());
    assert!(!journal.join("facets/personal/entities/target").exists());
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(sources.iter().all(|source| source.exists()));
    assert!(!journal.join("facets/work/entities/target").exists());
    assert!(!journal.join("facets/personal/entities/target").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn voiceprints_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    write_voiceprints(&journal, "source", vec![row(2.0)], vec![metadata("inject")]);
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, artifact_index| phase == "voiceprints" && artifact_index == 0)
        )
        .is_err()
    );
    assert!(!journal.join("entities/target/voiceprints.npz").exists());
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert_eq!(
        read_rows(&journal, "target").metadata,
        vec![metadata("inject")]
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn segments_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let paths = [
        journal.join("chronicle/20260102/080000_300/talents/speaker_labels.json"),
        journal.join("chronicle/20260102/090000_300/talents/speaker_labels.json"),
    ];
    for path in &paths {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, br#"{"labels":[{"speaker":"source"}]}"#).unwrap();
    }
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, artifact_index| phase == "segments" && artifact_index == 0)
        )
        .is_err()
    );
    for path in &paths {
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap()["labels"]
                [0]["speaker"],
            "source"
        );
    }
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    for path in &paths {
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap()["labels"]
                [0]["speaker"],
            "target"
        );
    }
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn activities_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let paths = [
        journal.join("facets/work/activities/20260102.jsonl"),
        journal.join("facets/work/activities/20260103.jsonl"),
    ];
    for path in &paths {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"{\"active_entities\":[\"source\"]}\n").unwrap();
    }
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, artifact_index| phase == "activities" && artifact_index == 0)
        )
        .is_err()
    );
    for path in &paths {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(path).unwrap()).unwrap()
                ["active_entities"][0],
            "source"
        );
    }
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    for path in &paths {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(path).unwrap()).unwrap()
                ["active_entities"][0],
            "target"
        );
    }
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn observation_relations_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let paths = [
        journal.join("facets/work/entities/other-one/observations.jsonl"),
        journal.join("facets/work/entities/other-two/observations.jsonl"),
    ];
    for path in &paths {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"{\"relation\":{\"target_entity_id\":\"source\"}}\n").unwrap();
    }
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(
                &|phase, artifact_index| phase == "observation relation remap"
                    && artifact_index == 0
            )
        )
        .is_err()
    );
    for path in &paths {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(path).unwrap()).unwrap()
                ["relation"]["target_entity_id"],
            "source"
        );
    }
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    for path in &paths {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(path).unwrap()).unwrap()
                ["relation"]["target_entity_id"],
            "target"
        );
    }
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn private_payload_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, artifact_index| phase == "private_payload" && artifact_index == 0)
        )
        .is_err()
    );
    assert!(
        list_entity_merge_payload_ids(&journal, "target")
            .unwrap()
            .is_empty()
    );
    assert!(journal.join("entities/source").exists());
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(
        !list_entity_merge_payload_ids(&journal, "target")
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn lineage_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["grandparent", "source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    commit_entity_merge(
        &journal,
        "grandparent",
        "source",
        EntityMergeOptions::default(),
    )
    .unwrap();
    let descendant = list_entity_merge_payload_ids(&journal, "source")
        .unwrap()
        .pop()
        .unwrap();
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, artifact_index| phase == "lineage" && artifact_index == 0)
        )
        .is_err()
    );
    assert!(
        list_entity_merge_payload_ids(&journal, "source")
            .unwrap()
            .contains(&descendant)
    );
    assert!(journal.join("entities/source").exists());
    assert!(journal.join("entities/target").exists());
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(
        list_entity_merge_payload_ids(&journal, "target")
            .unwrap()
            .contains(&descendant)
    );
    assert!(
        list_entity_merge_payload_ids(&journal, "source")
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn cleanup_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let source = journal.join("facets/work/entities/source");
    let target = journal.join("facets/work/entities/target");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(source.join("entity.json"), br#"{"entity_id":"source"}"#).unwrap();
    fs::write(target.join("entity.json"), br#"{"entity_id":"target"}"#).unwrap();
    let discovery = journal.join("awareness/discovery_clusters.json");
    fs::create_dir_all(discovery.parent().unwrap()).unwrap();
    fs::write(&discovery, b"{}").unwrap();
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, artifact_index| phase == "cleanup" && artifact_index == 0)
        )
        .is_err()
    );
    assert!(source.exists());
    assert!(discovery.exists());
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(!source.exists());
    assert!(!discovery.exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn history_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let before = read_visible_history(&journal, "target").unwrap();
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, artifact_index| phase == "history" && artifact_index == 0)
        )
        .is_err()
    );
    let after = read_visible_history(&journal, "target").unwrap();
    assert_eq!(after.len(), before.len());
    assert!(after.iter().all(|event| event.value()["kind"] != "merge"));
    assert!(journal.join("entities/source").exists());
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(
        read_visible_history(&journal, "target")
            .unwrap()
            .iter()
            .any(|event| event.value()["kind"] == "merge")
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn index_failure_retains_committed_source_and_retry_repairs_without_remerging() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let connection = solstone_core_indexer_store::db::open_index(&journal).unwrap();
    connection
        .execute(
            "INSERT INTO edges(src, dst, kind, directed, source, path, weight) VALUES ('source', 'other', 'related', 1, 'manual', 'test', 1)",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, artifact_index| phase == "edges" && artifact_index == 0)
        )
        .is_err()
    );
    assert!(!journal.join("entities/source").exists());
    assert!(
        journal
            .join("health/entity-merge-recovery/state.json")
            .exists()
    );
    let connection = solstone_core_indexer_store::db::open_index(&journal).unwrap();
    let source_edges: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE src = 'source'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_edges, 1);
    drop(connection);
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let connection = solstone_core_indexer_store::db::open_index(&journal).unwrap();
    let target_edges: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE src = 'target'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target_edges, 1);
    drop(connection);
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn audit_phase_injection_rolls_back_and_retry_succeeds() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let error = commit_entity_merge_with_injector(
        &journal,
        "source",
        "target",
        EntityMergeOptions::default(),
        Some(&|phase, artifact_index| phase == "audit" && artifact_index == 0),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "entity merge failed during audit: injected failure after audit artifact 0"
    );
    let merge_id = match error {
        crate::EntityMergeError::Failed { report, .. } => report.merge_id,
        _ => panic!("expected injected merge failure"),
    };
    let audit = journal.join("logs/entity-merges.jsonl");
    assert!(
        !audit.exists()
            || !fs::read_to_string(&audit)
                .unwrap()
                .lines()
                .any(
                    |line| serde_json::from_str::<serde_json::Value>(line).unwrap()["merge_id"]
                        == merge_id
                )
    );
    assert!(journal.join("entities/source").exists());
    let report =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(fs::read_to_string(audit).unwrap().lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line).unwrap()["merge_id"] == report.merge_id
    }));
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_refuses_blocked_source() {
    let journal = voiceprint_journal();
    save_entity_identity(
        &journal,
        "source",
        &json!({"id":"source","name":"source","aka":[],"emails":[],"blocked":true}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "target",
        &json!({"id":"target","name":"target","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    assert_eq!(
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
            .unwrap_err()
            .to_string(),
        "Cannot merge blocked entity: source"
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_refuses_blocked_target() {
    let journal = voiceprint_journal();
    save_entity_identity(
        &journal,
        "source",
        &json!({"id":"source","name":"source","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "target",
        &json!({"id":"target","name":"target","aka":[],"emails":[],"blocked":true}),
        None,
    )
    .unwrap();
    assert_eq!(
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
            .unwrap_err()
            .to_string(),
        "Cannot merge blocked entity: target"
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_refuses_same_entity() {
    let journal = voiceprint_journal();
    save_entity_identity(
        &journal,
        "source",
        &json!({"id":"source","name":"source","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    assert_eq!(
        commit_entity_merge(&journal, "source", "source", EntityMergeOptions::default())
            .unwrap_err()
            .to_string(),
        "Source and target must be different entities."
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_refuses_two_principal_entities() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[],"is_principal":true}),
            None,
        )
        .unwrap();
    }
    assert_eq!(
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
            .unwrap_err()
            .to_string(),
        "Cannot merge two principal entities."
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn committed_merge_arms_the_restore_guard() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let events = read_visible_history(&journal, "target").unwrap();
    let merge = events
        .iter()
        .find(|event| event.value()["kind"] == "merge")
        .unwrap();
    assert_eq!(
        guard_restore_does_not_cross_merge(merge, &events)
            .unwrap_err()
            .to_string(),
        "generic identity restore cannot target a recorded merge event; use recorded-merge undo instead"
    );
    let earlier = events
        .iter()
        .find(|event| event.value()["kind"] != "merge")
        .unwrap();
    assert_eq!(
        guard_restore_does_not_cross_merge(earlier, &events)
            .unwrap_err()
            .to_string(),
        "generic identity restore cannot cross a recorded merge event; use recorded-merge undo instead"
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_transfers_principal_from_source_to_target() {
    let journal = voiceprint_journal();
    save_entity_identity(
        &journal,
        "source",
        &json!({"id":"source","name":"source","aka":[],"emails":[],"is_principal":true}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "target",
        &json!({"id":"target","name":"target","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert_eq!(
        read_entity_identity(&journal, "target")
            .unwrap()
            .unwrap()
            .value()["is_principal"],
        true
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_fills_missing_target_scalars_from_source() {
    let journal = voiceprint_journal();
    save_entity_identity(
        &journal,
        "source",
        &json!({"id":"source","name":"source","aka":[],"emails":[],"title":"Engineer"}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "target",
        &json!({"id":"target","name":"target","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert_eq!(
        read_entity_identity(&journal, "target")
            .unwrap()
            .unwrap()
            .value()["title"],
        "Engineer"
    );
    fs::remove_dir_all(journal).unwrap();

    let journal = voiceprint_journal();
    save_entity_identity(
        &journal,
        "source",
        &json!({"id":"source","name":"source","aka":[],"emails":[],"title":"Engineer"}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "target",
        &json!({"id":"target","name":"target","aka":[],"emails":[],"title":"Director"}),
        None,
    )
    .unwrap();
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert_eq!(
        read_entity_identity(&journal, "target")
            .unwrap()
            .unwrap()
            .value()["title"],
        "Director"
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_never_overwrites_target_name() {
    let journal = voiceprint_journal();
    save_entity_identity(
        &journal,
        "source",
        &json!({"id":"source","name":"Source Name","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "target",
        &json!({"id":"target","name":"Target Name","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    commit_entity_merge(
        &journal,
        "source",
        "target",
        EntityMergeOptions {
            keep_source_as_aka: true,
        },
    )
    .unwrap();
    let target = read_entity_identity(&journal, "target").unwrap().unwrap();
    assert_eq!(target.value()["name"], "Target Name");
    assert!(
        target.value()["aka"]
            .as_array()
            .unwrap()
            .contains(&json!("Source Name"))
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn merge_refuses_when_third_entity_names_source_in_aka() {
    let journal = voiceprint_journal();
    save_entity_identity(
        &journal,
        "source",
        &json!({"id":"source","name":"Source Name","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "target",
        &json!({"id":"target","name":"Target Name","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "third",
        &json!({"id":"third","name":"Third","aka":["source"],"emails":[]}),
        None,
    )
    .unwrap();
    assert_eq!(
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
            .unwrap_err()
            .to_string(),
        "Cannot merge 'source': referenced in aka lists of entity ids: third"
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn preview_does_not_take_trust_lock_or_touch_index() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let index = journal.join("indexer/journal.sqlite");
    assert!(!index.exists());
    preview_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(!index.exists());
    drop(hold_entity_trust_lock(&journal).unwrap());
    fs::remove_dir_all(journal).unwrap();
}

fn write_divergent_identity(journal: &Path, entity_dir: &str, entity_id: &str) {
    save_entity_identity(
        journal,
        entity_dir,
        &json!({"id": entity_id, "name": entity_id, "aka": [], "emails": []}),
        None,
    )
    .unwrap();
}

fn write_facet_link(
    journal: &Path,
    facet: &str,
    relationship_dir: &str,
    entity_id: &str,
    body: &str,
) {
    let dir = journal.join(format!("facets/{facet}/entities/{relationship_dir}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("entity.json"),
        format!(r#"{{"entity_id":"{entity_id}"}}"#),
    )
    .unwrap();
    fs::write(dir.join("observations.jsonl"), body).unwrap();
}

#[test]
fn merge_facets_follows_relationship_directories() {
    let journal = voiceprint_journal();
    write_divergent_identity(&journal, "src-dir", "src-id");
    write_divergent_identity(&journal, "tgt-dir", "tgt-id");
    write_facet_link(
        &journal,
        "work",
        "src-rel",
        "src-id",
        "{\"content\":\"source\"}\n",
    );
    write_facet_link(
        &journal,
        "work",
        "tgt-rel",
        "tgt-id",
        "{\"content\":\"target\"}\n",
    );
    write_divergent_identity(&journal, "c-dir", "c-id");
    write_facet_link(
        &journal,
        "work",
        "src-id",
        "c-id",
        "{\"content\":\"collision\"}\n",
    );

    let stats = merge_facets(&journal, "src-id", "tgt-id", None, None).unwrap();
    assert_eq!(stats.merged_count, 1);
    assert_eq!(
        stats.removed_source_dirs,
        vec!["facets/work/entities/src-rel".to_owned()]
    );
    let merged =
        fs::read_to_string(journal.join("facets/work/entities/tgt-rel/observations.jsonl"))
            .unwrap();
    assert!(merged.contains("source"));
    assert!(merged.contains("target"));
    assert_eq!(
        fs::read_to_string(journal.join("facets/work/entities/src-id/observations.jsonl")).unwrap(),
        "{\"content\":\"collision\"}\n"
    );
    assert!(!journal.join("facets/work/entities/tgt-id").exists());
    fs::remove_dir_all(journal).unwrap();
}

fn remapped_commit_journal() -> PathBuf {
    let journal = voiceprint_journal();
    seed_identity_at(&journal, "dir-s", "source");
    seed_identity_at(&journal, "dir-t", "target");
    write_voiceprints_at(
        &journal,
        "dir-s",
        vec![row(2.0)],
        vec![metadata("S")],
        &test_encoder(),
    );
    write_voiceprints_at(
        &journal,
        "dir-t",
        vec![row(3.0)],
        vec![metadata("U")],
        &test_encoder(),
    );
    journal
}

#[test]
fn commit_entity_merge_reads_remapped_source_archive() {
    let journal = remapped_commit_journal();
    let report =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert_eq!(report.source_id, "source");
    assert_eq!(report.target_id, "target");
    let archive =
        read_voiceprints_npz(&fs::read(journal.join("entities/dir-t/voiceprints.npz")).unwrap())
            .unwrap();
    assert_eq!(archive.metadata, vec![metadata("U"), metadata("S")]);
    assert!(!journal.join("entities/source/voiceprints.npz").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn commit_entity_merge_writes_remapped_target_archive_and_payload() {
    let journal = remapped_commit_journal();
    let report =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(
        journal
            .join(format!(
                "entities/dir-t/history/private/{}.json",
                report.merge_id
            ))
            .is_file()
    );
    assert!(
        !journal
            .join(format!(
                "entities/target/history/private/{}.json",
                report.merge_id
            ))
            .exists()
    );
    assert!(!journal.join("entities/target/voiceprints.npz").exists());
    let payload = load_entity_merge_payload(&journal, "dir-t", &report.merge_id).unwrap();
    assert_eq!(payload["source_id"], "source");
    assert_eq!(payload["target_id"], "target");
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn commit_entity_merge_cleans_up_the_resolved_source_directory() {
    let journal = remapped_commit_journal();
    commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(!journal.join("entities/dir-s").exists());
    assert!(!journal.join("entities/source").exists());
    assert!(journal.join("entities/dir-t/entity.json").is_file());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn remapped_voiceprints_phase_injection_rolls_back_resolved_directories() {
    let journal = remapped_commit_journal();
    let source_before = fs::read(journal.join("entities/dir-s/voiceprints.npz")).unwrap();
    let target_before = fs::read(journal.join("entities/dir-t/voiceprints.npz")).unwrap();
    assert!(
        commit_entity_merge_with_injector(
            &journal,
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&|phase, artifact_index| phase == "voiceprints" && artifact_index == 0)
        )
        .is_err()
    );
    assert_eq!(
        fs::read(journal.join("entities/dir-s/voiceprints.npz")).unwrap(),
        source_before
    );
    assert_eq!(
        fs::read(journal.join("entities/dir-t/voiceprints.npz")).unwrap(),
        target_before
    );
    assert!(journal.join("entities/dir-s").exists());
    assert!(!journal.join("entities/source").exists());
    assert!(!journal.join("entities/target").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn commit_entity_merge_records_resolved_snapshot_paths() {
    let journal = remapped_commit_journal();
    let report =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let payload = load_entity_merge_payload(&journal, "dir-t", &report.merge_id).unwrap();
    assert_eq!(
        payload["source_state"]["snapshots"][0]["rel"],
        "entities/dir-s"
    );
    assert_eq!(
        payload["manifest"]["voiceprints"]["target_before"]["path"],
        "entities/dir-t/voiceprints.npz"
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn entity_crate_does_not_depend_on_facets() {
    let manifest = include_str!("../Cargo.toml");
    assert!(
        !manifest.contains("solstone-core-facets"),
        "solstone-core-entity must not gain a solstone-core-facets dependency"
    );
}

#[test]
fn speakers_listing_joins_stay_byte_identical() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let shell = crate_root.join("../solstone-core-convey-shell/src");
    let expected = [
        (
            "speakers_discovery.rs",
            r#"root.join("entities").join(entity_id).join("voiceprints.npz")"#,
        ),
        (
            "speakers_media.rs",
            r#"root.0.join("entities").join(&entity_id).join("voiceprints.npz")"#,
        ),
        (
            "speakers_cli_reads.rs",
            r#".join(&id)
                .join("voiceprints.npz")"#,
        ),
        (
            "speakers_known.rs",
            "let entity_id = entry.file_name().to_string_lossy().into_owned();",
        ),
        (
            "speakers_owner.rs",
            r#".join(principal_id)
                .join("voiceprints.npz")"#,
        ),
        (
            "speakers_quality.rs",
            r#".join(principal_id)
            .join("voiceprints.npz")"#,
        ),
        (
            "speakers_attribution.rs",
            r#"format!("entities/{old}/voiceprints.npz")"#,
        ),
    ];
    for (name, needle) in expected {
        let source = fs::read_to_string(shell.join(name))
            .unwrap_or_else(|error| panic!("read {}: {error}", shell.join(name).display()));
        assert!(
            source.contains(needle),
            "{name} must keep the existing entities/id join"
        );
    }
}

#[test]
fn merge_facets_relinks_when_target_has_no_relationship_dir() {
    let journal = voiceprint_journal();
    write_divergent_identity(&journal, "src-dir", "src-id");
    write_divergent_identity(&journal, "tgt-dir", "tgt-id");
    write_facet_link(
        &journal,
        "work",
        "src-rel",
        "src-id",
        "{\"content\":\"kept\"}\n",
    );

    let stats = merge_facets(&journal, "src-id", "tgt-id", None, None).unwrap();
    assert_eq!(stats.moved_count, 1);
    let link: serde_json::Value = serde_json::from_slice(
        &fs::read(journal.join("facets/work/entities/src-rel/entity.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(link["entity_id"], "tgt-id");
    assert_eq!(
        fs::read_to_string(journal.join("facets/work/entities/src-rel/observations.jsonl"))
            .unwrap(),
        "{\"content\":\"kept\"}\n"
    );
    assert!(!journal.join("facets/work/entities/tgt-id").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn failed_merge_preserves_an_unrelated_committed_index_write() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
    }
    let connection = solstone_core_indexer_store::db::open_index(&journal).unwrap();
    connection
        .execute("CREATE TABLE independent_write(value TEXT)", [])
        .unwrap();
    let injected = connection;
    let result = commit_entity_merge_with_injector(
        &journal,
        "source",
        "target",
        EntityMergeOptions::default(),
        Some(&move |phase, _| {
            if phase == "history" {
                injected
                    .execute("INSERT INTO independent_write VALUES ('acknowledged')", [])
                    .unwrap();
                true
            } else {
                false
            }
        }),
    );
    assert!(result.is_err());
    assert!(journal.join("entities/source").exists());
    let connection = solstone_core_indexer_store::db::open_index(&journal).unwrap();
    let retained: String = connection
        .query_row("SELECT value FROM independent_write", [], |row| row.get(0))
        .unwrap();
    assert_eq!(retained, "acknowledged");
    drop(connection);
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn malformed_recovery_journal_leaves_source_untouched() {
    let journal = voiceprint_journal();
    for id in ["source", "target"] {
        save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
    }
    let root = journal.join("health/entity-merge-recovery");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("state.json"), "{broken").unwrap();
    let entities = solstone_core_journal_io::capture_snapshot(&journal, "entities").unwrap();
    assert!(
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).is_err()
    );
    assert_eq!(
        solstone_core_journal_io::capture_snapshot(&journal, "entities").unwrap(),
        entities
    );
    fs::remove_dir_all(journal).unwrap();
}
