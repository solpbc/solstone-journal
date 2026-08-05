// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation, EntityLifecycleError,
    delete_entity_directory, has_journal_principal, read_entity_identity, read_identity_map,
    read_journal_principal, read_visible_history, record_ambiguity_choice,
    record_ambiguity_observation, remove_entity_ambiguity_references,
    restore_journal_entity_version, save_entity_identity, unblock_journal_entity,
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);
const LIFECYCLE_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_lifecycle.json"
));

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-lifecycle-{}-{sequence}",
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
fn unblock_clears_blocked_and_refuses_missing_or_unblocked_without_writing() {
    let temporary = TempDir::new();
    save_entity_identity(
        temporary.path(),
        "blocked",
        &json!({"id": "blocked", "blocked": true, "name": "Blocked"}),
        None,
    )
    .unwrap();
    let event = unblock_journal_entity(temporary.path(), "blocked").unwrap();
    assert_eq!(event["kind"], "update");
    assert!(
        read_entity_identity(temporary.path(), "blocked")
            .unwrap()
            .unwrap()
            .value()
            .get("blocked")
            .is_none()
    );

    save_entity_identity(
        temporary.path(),
        "open",
        &json!({"id": "open", "name": "Open"}),
        None,
    )
    .unwrap();
    let open_before = fs::read(temporary.path().join("entities/open/entity.json")).unwrap();
    assert!(matches!(
        unblock_journal_entity(temporary.path(), "open"),
        Err(EntityLifecycleError::EntityNotBlocked { .. })
    ));
    assert_eq!(
        fs::read(temporary.path().join("entities/open/entity.json")).unwrap(),
        open_before
    );
    assert!(matches!(
        unblock_journal_entity(temporary.path(), "missing"),
        Err(EntityLifecycleError::EntityNotFound { .. })
    ));
    assert!(!temporary.path().join("entities/missing").exists());
}

#[test]
fn restore_refuses_target_merge_and_crossing_later_merge() {
    let target_merge = TempDir::new();
    let target_event = save_entity_identity(
        target_merge.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice"}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    rewrite_event_kind(
        target_merge.path(),
        "alice",
        &target_event["version_id"],
        "merge",
    );
    assert!(matches!(
        restore_journal_entity_version(
            target_merge.path(),
            "alice",
            target_event["version_id"].as_str().unwrap(),
            None,
        ),
        Err(EntityLifecycleError::RestoreTargetsRecordedMerge)
    ));

    let later_merge = TempDir::new();
    let first = save_entity_identity(
        later_merge.path(),
        "alice",
        &json!({"id": "alice", "name": "Before"}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    let second = save_entity_identity(
        later_merge.path(),
        "alice",
        &json!({"id": "alice", "name": "After"}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    rewrite_event_kind(later_merge.path(), "alice", &second["version_id"], "merge");
    assert!(matches!(
        restore_journal_entity_version(
            later_merge.path(),
            "alice",
            first["version_id"].as_str().unwrap(),
            None,
        ),
        Err(EntityLifecycleError::RestoreCrossesRecordedMerge)
    ));
}

#[test]
fn restore_validates_snapshot_identity_and_principal_uniqueness() {
    let mismatch = TempDir::new();
    let event = save_entity_identity(
        mismatch.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice"}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    rewrite_event_identity(mismatch.path(), "alice", &event["version_id"], "other");
    assert!(matches!(
        restore_journal_entity_version(
            mismatch.path(),
            "alice",
            event["version_id"].as_str().unwrap(),
            None,
        ),
        Err(EntityLifecycleError::RestoreSnapshotIdentityMismatch { .. })
    ));

    let principal = TempDir::new();
    let principal_version = save_entity_identity(
        principal.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice", "is_principal": true}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    save_entity_identity(
        principal.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice", "is_principal": false}),
        None,
    )
    .unwrap();
    save_entity_identity(
        principal.path(),
        "bob",
        &json!({"id": "bob", "name": "Bob", "is_principal": true}),
        None,
    )
    .unwrap();
    assert!(matches!(
        restore_journal_entity_version(
            principal.path(),
            "alice",
            principal_version["version_id"].as_str().unwrap(),
            None,
        ),
        Err(EntityLifecycleError::RestoreWouldCreateSecondPrincipal {
            existing_entity_id,
            ..
        }) if existing_entity_id == "bob"
    ));

    let self_principal = TempDir::new();
    let self_version = save_entity_identity(
        self_principal.path(),
        "alice",
        &json!({"id": "alice", "name": "Before", "is_principal": true}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    save_entity_identity(
        self_principal.path(),
        "alice",
        &json!({"id": "alice", "name": "After", "is_principal": true}),
        None,
    )
    .unwrap();
    assert!(
        restore_journal_entity_version(
            self_principal.path(),
            "alice",
            self_version["version_id"].as_str().unwrap(),
            None,
        )
        .is_ok()
    );
}

#[test]
fn restore_replays_snapshot_and_appends_one_restore_event() {
    let temporary = TempDir::new();
    let snapshot = json!({"id": "alice", "name": "Before", "tags": ["one"]});
    let first = save_entity_identity(temporary.path(), "alice", &snapshot, None)
        .unwrap()
        .event
        .unwrap();
    save_entity_identity(
        temporary.path(),
        "alice",
        &json!({"id": "alice", "name": "After", "new": true}),
        None,
    )
    .unwrap();
    let before_count = read_visible_history(temporary.path(), "alice")
        .unwrap()
        .len();

    let restored = restore_journal_entity_version(
        temporary.path(),
        "alice",
        first["version_id"].as_str().unwrap(),
        Some(json!({"source": "test"})),
    )
    .unwrap();
    assert_eq!(restored["kind"], "restore");
    assert_eq!(
        restored["operation"]["restored_version_id"],
        first["version_id"]
    );
    assert_eq!(
        read_entity_identity(temporary.path(), "alice")
            .unwrap()
            .unwrap()
            .value(),
        &snapshot
    );
    assert_eq!(
        read_visible_history(temporary.path(), "alice")
            .unwrap()
            .len(),
        before_count + 1
    );
}

#[test]
fn principal_reads_are_empty_or_return_the_principal() {
    let temporary = TempDir::new();
    assert_eq!(read_journal_principal(temporary.path()).unwrap(), None);
    assert!(!has_journal_principal(temporary.path()).unwrap());
    save_entity_identity(
        temporary.path(),
        "owner",
        &json!({"id": "owner", "is_principal": true}),
        None,
    )
    .unwrap();
    assert_eq!(
        read_journal_principal(temporary.path()).unwrap().unwrap()["id"],
        "owner"
    );
    assert!(has_journal_principal(temporary.path()).unwrap());
}

#[test]
fn removing_entity_ambiguity_references_rewrites_only_target_rows() {
    let temporary = TempDir::new();
    record_observation(temporary.path(), "target", vec!["target", "survivor"]);
    resolve_observation(
        temporary.path(),
        "target",
        "target",
        &["target", "survivor"],
    );
    record_observation(temporary.path(), "other", vec!["survivor"]);
    resolve_observation(temporary.path(), "other", "survivor", &["survivor"]);
    let before_other = ambiguity_line(temporary.path(), 1);

    let report = remove_entity_ambiguity_references(temporary.path(), "target").unwrap();
    assert_eq!(report.rewritten_ambiguity_ids.len(), 1);
    assert!(report.removed_ambiguity_ids.is_empty());
    let rows = crate::read_ambiguities(
        temporary.path(),
        solstone_core_journal_io::MalformedPolicy::Raise,
    )
    .unwrap();
    assert_eq!(rows[0]["status"], "open");
    assert!(rows[0].get("resolved_entity_id").is_none());
    assert_eq!(rows[0]["ranked_candidates"][0]["id"], "survivor");
    assert_eq!(ambiguity_line(temporary.path(), 1), before_other);
}

#[test]
fn removing_entity_ambiguity_references_drops_rows_without_candidates() {
    let temporary = TempDir::new();
    record_observation(temporary.path(), "only-target", vec!["target"]);
    let report = remove_entity_ambiguity_references(temporary.path(), "target").unwrap();
    assert_eq!(report.rewritten_ambiguity_ids, Vec::<String>::new());
    assert_eq!(report.removed_ambiguity_ids.len(), 1);
    assert!(
        crate::read_ambiguities(
            temporary.path(),
            solstone_core_journal_io::MalformedPolicy::Raise,
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn delete_entity_directory_removes_the_effective_directory_and_rebuilds_cache() {
    let temporary = TempDir::new();
    save_entity_identity(
        temporary.path(),
        "effective",
        &json!({"id": "effective", "name": "Entity"}),
        None,
    )
    .unwrap();
    assert!(
        read_identity_map(temporary.path())
            .unwrap()
            .resolved
            .contains_key("effective")
    );
    delete_entity_directory(temporary.path(), "effective").unwrap();
    assert!(!temporary.path().join("entities/effective").exists());
    assert!(
        !read_identity_map(temporary.path())
            .unwrap()
            .resolved
            .contains_key("effective")
    );
}

#[test]
fn lifecycle_fixture_declares_a_target_identity() {
    let fixture: Value = serde_json::from_str(LIFECYCLE_FIXTURE).unwrap();
    assert_eq!(fixture["target_entity_id"], "target");
}

fn rewrite_event_kind(root: &Path, entity: &str, version: &Value, kind: &str) {
    rewrite_event(root, entity, version, |event| event["kind"] = json!(kind));
}

fn rewrite_event_identity(root: &Path, entity: &str, version: &Value, id: &str) {
    rewrite_event(root, entity, version, |event| {
        event["identity_after"]["id"] = json!(id)
    });
}

fn rewrite_event(root: &Path, entity: &str, version: &Value, change: impl FnOnce(&mut Value)) {
    let path = fs::read_dir(root.join("entities").join(entity).join("history/events"))
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| entry.path())
        .find(|path| {
            fs::read_to_string(path)
                .unwrap()
                .contains(version.as_str().unwrap())
        })
        .unwrap();
    let mut event: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut event);
    fs::write(path, serde_json::to_vec(&event).unwrap()).unwrap();
}

fn record_observation(root: &Path, query: &str, candidates: Vec<&str>) {
    record_ambiguity_observation(
        root,
        &AmbiguityObservation {
            scope: json!({"kind": "journal"}),
            query: query.to_owned(),
            normalized_query: query.to_owned(),
            observed_tier: 5,
            ranked_candidates: candidates
                .into_iter()
                .map(|id| json!({"id": id, "name": id, "tier": 5, "score": 1.0}))
                .collect(),
            origin: json!({"lane": "segment", "day": "20260805", "segment_id": query}),
        },
    )
    .unwrap();
}

fn resolve_observation(root: &Path, query: &str, entity_id: &str, candidates: &[&str]) {
    record_ambiguity_choice(
        root,
        &AmbiguityChoiceRequest {
            scope: json!({"kind": "journal"}),
            query: query.to_owned(),
            entity_id: entity_id.to_owned(),
            origin: None,
        },
        &candidates
            .iter()
            .map(|id| AmbiguityChoiceEntity {
                id: (*id).to_owned(),
                blocked: false,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
}

fn ambiguity_line(root: &Path, index: usize) -> String {
    fs::read_to_string(root.join("entities/ambiguities.jsonl"))
        .unwrap()
        .lines()
        .nth(index)
        .unwrap()
        .to_owned()
}
