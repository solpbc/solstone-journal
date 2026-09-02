// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;

use serde_json::{Value, json};
use solstone_core_entity::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation, EntityOperationContext,
    EntityOperationKind, EntityResolutionEntity, EntityResolutionOutcome, read_identity_map,
    record_ambiguity_choice, record_ambiguity_observation, record_entity_resolution,
    record_merge_candidate, remove_entity_ambiguity_references, save_entity_identity,
};
use solstone_core_indexer_store::db::open_index;

use crate::store_tests::{
    TempDir, create_test_facet, relationship_value, write_facet_relationship, write_journal_entity,
};
use crate::{
    EntityDeleteGuardOutcome, EntityHistoryReference, FacetEntityLifecycleError,
    block_journal_entity, delete_created_entity_if_unreferenced, delete_facet_entity_link,
    delete_journal_entity, read_facet_entity_link, set_facet_entity_link_detached,
};

#[test]
fn block_detaches_links_by_stored_entity_id_and_reports_only_new_detaches() {
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "target_dir", Some("target"));
    for facet in ["work", "home"] {
        create_test_facet(temporary.path(), facet);
    }
    write_facet_relationship(
        temporary.path(),
        "work",
        "legacy_relationship_directory",
        json!({"entity_id": "target", "role": "owner"}),
    );
    write_facet_relationship(
        temporary.path(),
        "work",
        "already_detached",
        json!({"entity_id": "target", "detached": true, "role": "observer"}),
    );
    write_facet_relationship(
        temporary.path(),
        "home",
        "target_elsewhere",
        json!({"entity_id": "target"}),
    );
    let already_path = temporary
        .path()
        .join("facets/work/entities/already_detached/entity.json");
    let already_before = fs::read(&already_path).unwrap();

    let report = block_journal_entity(temporary.path(), "target").unwrap();
    assert_eq!(report.facets_detached, vec!["home", "work"]);
    assert_eq!(
        relationship_value(temporary.path(), "work", "legacy_relationship_directory")["detached"],
        true
    );
    assert_eq!(
        relationship_value(temporary.path(), "home", "target_elsewhere")["detached"],
        true
    );
    assert_eq!(fs::read(&already_path).unwrap(), already_before);

    // An interrupted block has no unique on-disk signature: detached relationships
    // are also valid after a completed block. Retrying is deliberately idempotent.
    assert!(
        block_journal_entity(temporary.path(), "target")
            .unwrap()
            .facets_detached
            .is_empty()
    );
}

#[test]
fn block_refuses_principal_before_mutating_identity_or_relationships() {
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "owner", Some("owner"));
    let owner = temporary.path().join("entities/owner/entity.json");
    let mut owner_value: Value = serde_json::from_slice(&fs::read(&owner).unwrap()).unwrap();
    owner_value["is_principal"] = json!(true);
    fs::write(&owner, serde_json::to_vec(&owner_value).unwrap()).unwrap();
    create_test_facet(temporary.path(), "work");
    write_facet_relationship(
        temporary.path(),
        "work",
        "legacy",
        json!({"entity_id": "owner", "role": "owner"}),
    );
    let relationship = temporary
        .path()
        .join("facets/work/entities/legacy/entity.json");
    let identity_before = fs::read(&owner).unwrap();
    let relationship_before = fs::read(&relationship).unwrap();

    assert!(matches!(
        block_journal_entity(temporary.path(), "owner"),
        Err(FacetEntityLifecycleError::PrincipalEntityProtected { .. })
    ));
    assert_eq!(fs::read(&owner).unwrap(), identity_before);
    assert_eq!(fs::read(&relationship).unwrap(), relationship_before);
}

#[test]
fn relationship_detach_and_delete_preserve_or_remove_the_expected_tree() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_facet_relationship(
        temporary.path(),
        "work",
        "legacy",
        json!({"entity_id": "target", "role": "owner"}),
    );
    assert!(set_facet_entity_link_detached(temporary.path(), "work", "legacy", true).unwrap());
    let changed = relationship_value(temporary.path(), "work", "legacy");
    assert_eq!(changed["entity_id"], "target");
    assert_eq!(changed["role"], "owner");
    assert_eq!(changed["detached"], true);
    let bytes = fs::read(
        temporary
            .path()
            .join("facets/work/entities/legacy/entity.json"),
    )
    .unwrap();
    assert!(!set_facet_entity_link_detached(temporary.path(), "work", "legacy", true).unwrap());
    assert_eq!(
        fs::read(
            temporary
                .path()
                .join("facets/work/entities/legacy/entity.json")
        )
        .unwrap(),
        bytes
    );
    assert!(!set_facet_entity_link_detached(temporary.path(), "work", "missing", true).unwrap());

    let relationship_dir = temporary.path().join("facets/work/entities/legacy");
    fs::write(relationship_dir.join("observations.jsonl"), b"{}\n").unwrap();
    assert!(delete_facet_entity_link(temporary.path(), "work", "legacy").unwrap());
    assert!(!relationship_dir.exists());
    assert!(
        read_facet_entity_link(temporary.path(), "work", "legacy")
            .unwrap()
            .is_none()
    );
    assert!(!delete_facet_entity_link(temporary.path(), "work", "missing").unwrap());
}

#[test]
fn delete_removes_divergent_links_entity_and_identity_cache_entry() {
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "target_dir", Some("target"));
    for facet in ["work", "home"] {
        create_test_facet(temporary.path(), facet);
    }
    write_facet_relationship(
        temporary.path(),
        "work",
        "legacy_directory",
        json!({"entity_id": "target"}),
    );
    write_facet_relationship(
        temporary.path(),
        "home",
        "another_legacy_directory",
        json!({"entity_id": "target"}),
    );

    let report = delete_journal_entity(temporary.path(), "target").unwrap();
    assert_eq!(report.facets_deleted, vec!["home", "work"]);
    assert!(!temporary.path().join("entities/target_dir").exists());
    assert!(
        !temporary
            .path()
            .join("facets/work/entities/legacy_directory")
            .exists()
    );
    assert!(
        !temporary
            .path()
            .join("facets/home/entities/another_legacy_directory")
            .exists()
    );
    assert!(
        !read_identity_map(temporary.path())
            .unwrap()
            .resolved
            .contains_key("target")
    );
}

#[test]
fn guarded_delete_removes_matching_unreferenced_entity() {
    let temporary = TempDir::new();
    let (identity, history) = create_identify_entity(temporary.path(), "target", "op-1");
    open_index(temporary.path()).unwrap();

    let outcome = delete_created_entity_if_unreferenced(
        temporary.path(),
        "target",
        "op-1",
        &identity,
        &[history],
    )
    .unwrap();

    assert_eq!(outcome, deleted_outcome());
    assert!(!temporary.path().join("entities/target").exists());
    assert!(
        !read_identity_map(temporary.path())
            .unwrap()
            .resolved
            .contains_key("target")
    );
}

#[test]
fn guarded_delete_reports_identity_change_without_skipping_other_checks() {
    let temporary = TempDir::new();
    let (mut identity, history) = create_identify_entity(temporary.path(), "target", "op-1");
    open_index(temporary.path()).unwrap();
    identity["name"] = json!("previous name");

    let outcome = delete_created_entity_if_unreferenced(
        temporary.path(),
        "target",
        "op-1",
        &identity,
        &[history],
    )
    .unwrap();

    assert!(!outcome.deleted);
    assert!(outcome.identity_changed);
    assert!(!outcome.history_changed);
    assert_eq!(outcome.references, Default::default());
    assert!(temporary.path().join("entities/target").exists());
}

#[test]
fn guarded_delete_excludes_its_own_correction_and_identify_operation_rows() {
    let temporary = TempDir::new();
    let (identity, history) = create_identify_entity(temporary.path(), "target", "op-1");
    open_index(temporary.path()).unwrap();
    write_text(
        temporary.path(),
        "chronicle/20260805/stream/seg/talents/speaker_corrections.json",
        "{\"corrections\":[{\"original_speaker\":\"target\",\"operation_id\":\"op-1\"}]}",
    );
    write_text(
        temporary.path(),
        "speakers/identify-operations.jsonl",
        "{\"target_entity_id\":\"target\",\"operation_id\":\"op-1\"}\n",
    );

    let outcome = delete_created_entity_if_unreferenced(
        temporary.path(),
        "target",
        "op-1",
        &identity,
        &[history],
    )
    .unwrap();

    assert_eq!(outcome, deleted_outcome());
}

#[test]
fn guarded_delete_refuses_an_entity_review_candidate_reference() {
    let temporary = TempDir::new();
    let (identity, history) = create_identify_entity(temporary.path(), "target", "op-1");
    create_test_facet(temporary.path(), "scope");
    write_facet_relationship(
        temporary.path(),
        "scope",
        "target-link",
        json!({"entity_id":"target"}),
    );
    let entity_id = crate::list_scoped_facet_entities(temporary.path(), "scope", false, false)
        .unwrap()
        .pop()
        .unwrap()
        .entity_id;
    open_index(temporary.path()).unwrap();
    record_merge_candidate(
        temporary.path(),
        "scope",
        "20260101",
        "Target",
        &entity_id,
        "Other",
        "other",
        "review this",
        None,
        None,
        None,
    )
    .unwrap();

    let outcome = delete_created_entity_if_unreferenced(
        temporary.path(),
        &entity_id,
        "op-1",
        &identity,
        &[history],
    )
    .unwrap();
    assert!(!outcome.deleted);
    assert!(!outcome.identity_changed);
    assert!(!outcome.history_changed);
    assert_eq!(outcome.references.entity_review_candidate, 1);
}

#[test]
fn guarded_delete_counts_divergent_reference_surfaces_like_aligned_entities() {
    let divergent = TempDir::new();
    let aligned = TempDir::new();
    write_reference_shape(divergent.path(), "target-directory", "target");
    write_reference_shape(aligned.path(), "target", "target");
    open_index(divergent.path()).unwrap();
    open_index(aligned.path()).unwrap();

    let divergent_counts = crate::store::reference_scan::scan_entity_references(
        divergent.path(),
        "target",
        "target-directory",
        Some("op-1"),
    )
    .unwrap();
    let aligned_counts = crate::store::reference_scan::scan_entity_references(
        aligned.path(),
        "target",
        "target",
        Some("op-1"),
    )
    .unwrap();

    assert_eq!(divergent_counts, aligned_counts);
    assert_eq!(divergent_counts.facet_relationship, 1);
    assert_eq!(divergent_counts.observation, 1);
    assert_eq!(divergent_counts.unrecognized_file, 1);
}

#[test]
fn guarded_delete_missing_is_an_already_gone_success() {
    let temporary = TempDir::new();
    let outcome =
        delete_created_entity_if_unreferenced(temporary.path(), "missing", "op-1", &json!({}), &[])
            .unwrap();

    assert!(outcome.deleted);
    assert!(outcome.already_gone);
    assert!(!outcome.identity_changed);
    assert!(!outcome.history_changed);
    assert_eq!(outcome.references, Default::default());
}

#[test]
fn guarded_delete_refuses_history_mismatch_and_unrecognized_contents() {
    let history_mismatch = TempDir::new();
    let (identity, history) = create_identify_entity(history_mismatch.path(), "target", "op-1");
    open_index(history_mismatch.path()).unwrap();
    let outcome = delete_created_entity_if_unreferenced(
        history_mismatch.path(),
        "target",
        "different-operation",
        &identity,
        &[history],
    )
    .unwrap();
    assert!(outcome.history_changed);
    assert_eq!(outcome.references, Default::default());

    let unrecognized = TempDir::new();
    let (identity, history) = create_identify_entity(unrecognized.path(), "target", "op-1");
    open_index(unrecognized.path()).unwrap();
    write_text(unrecognized.path(), "entities/target/extra.bin", "x");
    let outcome = delete_created_entity_if_unreferenced(
        unrecognized.path(),
        "target",
        "op-1",
        &identity,
        &[history],
    )
    .unwrap();
    assert!(!outcome.deleted);
    assert_eq!(outcome.references.unrecognized_file, 1);
}

#[test]
fn guarded_delete_fails_closed_for_missing_index_without_changing_owner_delete_report() {
    let guarded = TempDir::new();
    let (identity, history) = create_identify_entity(guarded.path(), "target", "op-1");
    let outcome = delete_created_entity_if_unreferenced(
        guarded.path(),
        "target",
        "op-1",
        &identity,
        &[history],
    )
    .unwrap();
    assert!(!outcome.deleted);
    assert!(outcome.references.unreadable >= 1);
    assert!(guarded.path().join("entities/target").exists());

    let owner_delete = TempDir::new();
    write_journal_entity(owner_delete.path(), "target", Some("target"));
    let report = delete_journal_entity(owner_delete.path(), "target").unwrap();
    assert_eq!(report.references, Default::default());
}

#[test]
fn guarded_delete_blocks_target_ambiguity_but_not_other_entity_ambiguity() {
    let blocked = TempDir::new();
    let (identity, history) = create_identify_entity(blocked.path(), "target", "op-1");
    open_index(blocked.path()).unwrap();
    write_text(
        blocked.path(),
        "entities/ambiguities.jsonl",
        "{\"resolved_entity_id\":\"target\"}\n",
    );
    let blocked_outcome = delete_created_entity_if_unreferenced(
        blocked.path(),
        "target",
        "op-1",
        &identity,
        &[history],
    )
    .unwrap();
    assert_eq!(blocked_outcome.references.ambiguity, 1);
    assert!(!blocked_outcome.deleted);

    let clear = TempDir::new();
    let (_identity, _history) = create_identify_entity(clear.path(), "target", "op-1");
    open_index(clear.path()).unwrap();
    write_text(
        clear.path(),
        "entities/ambiguities.jsonl",
        "{\"resolved_entity_id\":\"other\"}\n",
    );
    let clear_counts = crate::store::reference_scan::scan_entity_references(
        clear.path(),
        "target",
        "target",
        Some("op-1"),
    )
    .unwrap();
    assert_eq!(clear_counts.ambiguity, 0);
}

#[test]
fn delete_refusals_leave_a_populated_store_byte_identical() {
    // Python's owner-facing delete raises on a missing entity; this port returns Err,
    // rather than treating a missing id as an already-complete deletion.
    let temporary = TempDir::new();
    for id in ["owner", "other", "target"] {
        write_journal_entity(temporary.path(), id, Some(id));
    }
    let owner = temporary.path().join("entities/owner/entity.json");
    fs::write(&owner, b"{\"id\":\"owner\",\"is_principal\":true}").unwrap();
    create_test_facet(temporary.path(), "work");
    for id in ["owner", "other", "target"] {
        write_facet_relationship(
            temporary.path(),
            "work",
            id,
            json!({"entity_id": id, "role": "member"}),
        );
    }
    let before = tree_bytes(temporary.path());
    assert!(matches!(
        delete_journal_entity(temporary.path(), "owner"),
        Err(FacetEntityLifecycleError::PrincipalEntityProtected { .. })
    ));
    assert_eq!(tree_bytes(temporary.path()), before);
    assert!(matches!(
        delete_journal_entity(temporary.path(), "missing"),
        Err(FacetEntityLifecycleError::Entity(
            solstone_core_entity::EntityLifecycleError::EntityNotFound { .. }
        ))
    ));
    assert_eq!(tree_bytes(temporary.path()), before);
}

#[test]
fn reference_breakdown_reports_each_surface_independently() {
    // The fixed struct makes a single-total regression unrepresentable.
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "target", Some("target"));
    write_journal_entity(temporary.path(), "other", Some("other"));
    fs::write(
        temporary.path().join("entities/other/entity.json"),
        b"{\"id\":\"other\",\"aka\":[\"target\"]}",
    )
    .unwrap();
    write_text(temporary.path(), "entities/target/unrecognized.bin", "x");
    create_test_facet(temporary.path(), "work");
    write_facet_relationship(
        temporary.path(),
        "work",
        "legacy",
        json!({"entity_id":"target"}),
    );
    write_text(
        temporary.path(),
        "facets/work/entities/legacy/observations.jsonl",
        "{\"target_entity_id\":\"target\"}\n",
    );
    write_text(
        temporary.path(),
        "facets/work/activities/day.jsonl",
        "{\"active_entities\":[\"target\"]}\nnot json\n",
    );
    write_text(
        temporary.path(),
        "facets/work/activities/archive/day.jsonl",
        "{\"active_entities\":[\"target\"]}\n",
    );
    write_text(
        temporary.path(),
        "chronicle/20260805/stream/seg/talents/speaker_labels.json",
        "{\"labels\":[{\"speaker\":\"target\"}]}",
    );
    write_text(
        temporary.path(),
        "chronicle/20260805/stream/seg/talents/speaker_corrections.json",
        "{\"corrections\":[{\"original_speaker\":\"target\"}]}",
    );
    write_text(
        temporary.path(),
        "chronicle/other/speaker_labels.json",
        "{\"labels\":[{\"speaker\":\"target\"}]}",
    );
    write_text(
        temporary.path(),
        "awareness/speaker_candidates.json",
        "{\"candidates\":[{\"confirmed_entity\":\"target\"}]}",
    );
    write_text(
        temporary.path(),
        "speakers/keep-separate.jsonl",
        "{\"entity_id_a\":\"target\",\"entity_id_b\":\"other\"}\n",
    );
    write_text(
        temporary.path(),
        "speakers/identify-operations.jsonl",
        "{\"target_entity_id\":\"target\"}\n",
    );
    write_text(
        temporary.path(),
        "entities/ambiguities.jsonl",
        "{\"resolved_entity_id\":\"target\"}\n",
    );
    write_text(
        temporary.path(),
        "entities/review-candidates.jsonl",
        "{\"source_slug\":\"target\"}\n",
    );
    write_text(
        temporary.path(),
        "speakers/review-candidates.jsonl",
        "{\"source_id\":\"target\"}\n",
    );
    let counts = crate::store::reference_scan::scan_entity_references(
        temporary.path(),
        "target",
        "target",
        None,
    )
    .unwrap();
    assert_eq!(counts.unrecognized_file, 1);
    assert_eq!(counts.facet_relationship, 1);
    assert_eq!(counts.observation, 1);
    assert_eq!(counts.activity, 1);
    assert_eq!(counts.segment_label, 1);
    assert_eq!(counts.segment_correction, 1);
    assert_eq!(counts.aka_crossref, 1);
    assert_eq!(counts.speaker_candidate, 1);
    assert_eq!(counts.keep_separate, 1);
    assert_eq!(counts.identify_operation, 1);
    assert_eq!(counts.ambiguity, 1);
    assert_eq!(counts.entity_review_candidate, 1);
    assert_eq!(counts.speaker_review_candidate, 1);
    assert_eq!(counts.candidate_pair, 0);
    assert_eq!(counts.dismissal, 0);
    assert_eq!(counts.unreadable, 1);
}

#[test]
fn scan_counts_synthetic_candidate_pair_entity_reference() {
    let counts = scan_synthetic_entity_reference(
        "speakers/candidate-pair-review-candidates.jsonl",
        "{\"entity_id\":\"target\"}\n",
    );

    assert_eq!(counts.candidate_pair, 1);
    assert_eq!(counts.dismissal, 0);
}

#[test]
fn scan_counts_synthetic_dismissal_entity_reference() {
    let counts = scan_synthetic_entity_reference(
        "speakers/cluster-dismissals.jsonl",
        "{\"entity_id\":\"target\"}\n",
    );

    assert_eq!(counts.candidate_pair, 0);
    assert_eq!(counts.dismissal, 1);
}

#[test]
fn partial_delete_steps_are_distinguishable_from_complete_or_untouched() {
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "target", Some("target"));
    create_test_facet(temporary.path(), "work");
    write_facet_relationship(
        temporary.path(),
        "work",
        "legacy",
        json!({"entity_id":"target"}),
    );
    delete_facet_entity_link(temporary.path(), "work", "legacy").unwrap();
    remove_entity_ambiguity_references(temporary.path(), "target").unwrap();
    assert!(temporary.path().join("entities/target").is_dir());
    assert!(
        !temporary
            .path()
            .join("facets/work/entities/legacy")
            .exists()
    );
}

#[test]
fn delete_repairs_ambiguities_for_real_resolution_without_disturbing_survivor_choice() {
    let temporary = TempDir::new();
    seed_twin_resolved_ambiguities(temporary.path());

    delete_journal_entity(temporary.path(), "target").unwrap();

    let first = resolve(temporary.path(), "first", &[entity("survivor")]).unwrap();
    assert_ne!(first.outcome, EntityResolutionOutcome::Resolved);
    let second = resolve(temporary.path(), "second", &[entity("survivor")]).unwrap();
    assert_eq!(second.outcome, EntityResolutionOutcome::Resolved);
    assert_eq!(second.entity_index, Some(0));
}

#[test]
fn block_preserves_twin_ambiguity_choices_for_real_resolution() {
    let temporary = TempDir::new();
    seed_twin_resolved_ambiguities(temporary.path());

    block_journal_entity(temporary.path(), "target").unwrap();

    // Resolution receives its current caller-owned entity slice. Supplying target
    // here confirms the persisted choice survived block unchanged.
    let first = resolve(
        temporary.path(),
        "first",
        &[entity("target"), entity("survivor")],
    )
    .unwrap();
    assert_eq!(first.outcome, EntityResolutionOutcome::Resolved);
    assert_eq!(first.entity_index, Some(0));
    let second = resolve(
        temporary.path(),
        "second",
        &[entity("target"), entity("survivor")],
    )
    .unwrap();
    assert_eq!(second.outcome, EntityResolutionOutcome::Resolved);
    assert_eq!(second.entity_index, Some(1));
}

fn seed_twin_resolved_ambiguities(root: &std::path::Path) {
    write_journal_entity(root, "target", Some("target"));
    write_journal_entity(root, "survivor", Some("survivor"));
    for (query, candidates, selected) in [
        ("first", vec!["target", "survivor"], "target"),
        ("second", vec!["survivor"], "survivor"),
    ] {
        let scope = json!({"kind": "journal"});
        record_ambiguity_observation(
            root,
            &AmbiguityObservation {
                scope: scope.clone(),
                query: query.to_owned(),
                normalized_query: query.to_owned(),
                observed_tier: 5,
                ranked_candidates: candidates
                    .iter()
                    .map(|id| json!({"id": id, "name": id, "tier": 5, "score": 1.0}))
                    .collect(),
                origin: json!({"lane": "segment", "day": "20260805", "segment_id": query}),
            },
        )
        .unwrap();
        record_ambiguity_choice(
            root,
            &AmbiguityChoiceRequest {
                scope,
                query: query.to_owned(),
                entity_id: selected.to_owned(),
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
}

fn resolve(
    root: &std::path::Path,
    query: &str,
    entities: &[EntityResolutionEntity],
) -> Result<solstone_core_entity::EntityResolution, solstone_core_entity::EntityResolutionError> {
    record_entity_resolution(
        root,
        query,
        entities,
        json!({"kind": "journal"}),
        json!({"lane": "test"}),
        80.0,
        false,
    )
}

fn entity(id: &str) -> EntityResolutionEntity {
    EntityResolutionEntity {
        id: Some(id.to_owned()),
        name: id.to_owned(),
        aka: Vec::new(),
        emails: Vec::new(),
        blocked: false,
    }
}

fn write_text(root: &std::path::Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn create_identify_entity(
    root: &std::path::Path,
    entity_id: &str,
    operation_id: &str,
) -> (Value, EntityHistoryReference) {
    let identity = json!({"id": entity_id, "name": "Target", "type": "Person"});
    let operation = EntityOperationContext {
        kind: EntityOperationKind::Create,
        caller: Value::Null,
        actor: Value::Null,
        metadata: json!({
            "operation_kind": "speaker_identify",
            "operation_id": operation_id,
        }),
    };
    let event = save_entity_identity(root, entity_id, &identity, Some(&operation))
        .unwrap()
        .event
        .unwrap();
    (
        identity,
        EntityHistoryReference {
            version_id: event["version_id"].as_str().unwrap().to_owned(),
            sequence: event["seq"].as_i64().unwrap().into(),
        },
    )
}

fn deleted_outcome() -> EntityDeleteGuardOutcome {
    EntityDeleteGuardOutcome {
        deleted: true,
        already_gone: false,
        identity_changed: false,
        history_changed: false,
        references: Default::default(),
    }
}

fn write_reference_shape(root: &std::path::Path, entity_dir: &str, entity_id: &str) {
    write_journal_entity(root, entity_dir, Some(entity_id));
    write_text(root, &format!("entities/{entity_dir}/unexpected.bin"), "x");
    create_test_facet(root, "work");
    write_facet_relationship(root, "work", "legacy", json!({"entity_id": entity_id}));
    write_text(
        root,
        "facets/work/entities/legacy/observations.jsonl",
        &format!("{{\"target_entity_id\":\"{entity_id}\"}}\n"),
    );
}

fn scan_synthetic_entity_reference(
    relative: &str,
    contents: &str,
) -> crate::store::reference_scan::EntityReferenceBreakdown {
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "target", Some("target"));
    write_text(temporary.path(), relative, contents);
    crate::store::reference_scan::scan_entity_references(temporary.path(), "target", "target", None)
        .unwrap()
}

fn tree_bytes(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut result = Vec::new();
    collect_tree(root, root, &mut result);
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

fn collect_tree(
    root: &std::path::Path,
    path: &std::path::Path,
    output: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
) {
    for entry in fs::read_dir(path).unwrap().map(Result::unwrap) {
        let entry_path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            collect_tree(root, &entry_path, output);
        } else if entry_path
            .strip_prefix(root)
            .is_ok_and(|relative| relative.starts_with("health/locks"))
        {
            continue;
        } else {
            output.push((
                entry_path.strip_prefix(root).unwrap().to_owned(),
                fs::read(entry_path).unwrap(),
            ));
        }
    }
}
