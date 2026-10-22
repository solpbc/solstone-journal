// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use crate::{
    FacetEntityLifecycleError, block_journal_entity, block_journal_entity_with_hook, create_facet,
    detach_facet_entity, move_facet_entity, unblock_journal_entity,
    unblock_journal_entity_with_hook,
};
use serde_json::{Map, Value, json};
use solstone_core_entity::EntityLifecycleError;

fn create_test_facet(root: &Path, facet: &str) {
    create_facet(root, facet, facet, "Description", "blue", "💼", None).unwrap();
}

fn write_journal_entity(root: &Path, entity_dir: &str, written_id: Option<&str>) {
    let folder = root.join("entities").join(entity_dir);
    fs::create_dir_all(&folder).unwrap();
    let mut entity = Map::new();
    if let Some(written_id) = written_id {
        entity.insert("id".to_owned(), Value::String(written_id.to_owned()));
    }
    fs::write(
        folder.join("entity.json"),
        serde_json::to_vec_pretty(&Value::Object(entity)).unwrap(),
    )
    .unwrap();
}

fn write_journal_entity_with_fields(root: &Path, entity_dir: &str, fields: Value) {
    let folder = root.join("entities").join(entity_dir);
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("entity.json"),
        serde_json::to_vec_pretty(&fields).unwrap(),
    )
    .unwrap();
}

fn write_facet_relationship(root: &Path, facet: &str, entity_dir: &str, relationship: Value) {
    let folder = root
        .join("facets")
        .join(facet)
        .join("entities")
        .join(entity_dir);
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("entity.json"),
        serde_json::to_vec_pretty(&relationship).unwrap(),
    )
    .unwrap();
}

fn relationship_value(root: &Path, facet: &str, entity_dir: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(
            root.join("facets")
                .join(facet)
                .join("entities")
                .join(entity_dir)
                .join("entity.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn journal_entity_value(root: &Path, entity_dir: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(root.join("entities").join(entity_dir).join("entity.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn hide_of_two_attached_relationships_writes_both_keys_and_leaves_another_entity_file_byte_identical()
 {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity(temporary.path(), "target", Some("target"));
    write_journal_entity(temporary.path(), "other", Some("other"));
    create_test_facet(temporary.path(), "work");
    create_test_facet(temporary.path(), "home");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work",
        json!({"entity_id": "target", "role": "engineer"}),
    );
    write_facet_relationship(
        temporary.path(),
        "home",
        "rel_home",
        json!({"entity_id": "target", "role": "resident"}),
    );
    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_other",
        json!({"entity_id": "other", "role": "peer"}),
    );
    let other_file = temporary
        .path()
        .join("facets/work/entities/rel_other/entity.json");
    let other_bytes_before = fs::read(&other_file).unwrap();

    let report = block_journal_entity(temporary.path(), "target").unwrap();
    assert_eq!(report.facets_detached, vec!["home", "work"]);

    let rel_work = relationship_value(temporary.path(), "work", "rel_work");
    assert_eq!(rel_work["detached"], true);
    assert_eq!(rel_work["detached_by"], "hide");
    assert_eq!(rel_work["role"], "engineer");

    let rel_home = relationship_value(temporary.path(), "home", "rel_home");
    assert_eq!(rel_home["detached"], true);
    assert_eq!(rel_home["detached_by"], "hide");
    assert_eq!(rel_home["role"], "resident");

    let other_bytes_after = fs::read(&other_file).unwrap();
    assert_eq!(other_bytes_before, other_bytes_after);
}

#[test]
fn already_detached_file_stays_byte_identical() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity(temporary.path(), "target", Some("target"));
    create_test_facet(temporary.path(), "work");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work",
        json!({"entity_id": "target", "detached": true, "role": "engineer"}),
    );
    let file = temporary
        .path()
        .join("facets/work/entities/rel_work/entity.json");
    let bytes_before = fs::read(&file).unwrap();

    let report = block_journal_entity(temporary.path(), "target").unwrap();
    assert!(report.facets_detached.is_empty());

    let bytes_after = fs::read(&file).unwrap();
    assert_eq!(bytes_before, bytes_after);
}

#[test]
fn unhide_removes_detached_and_marker_only_where_marker_is_hide() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target", "blocked": true}),
    );
    write_journal_entity_with_fields(
        temporary.path(),
        "other",
        json!({"id": "other", "name": "Other"}),
    );
    create_test_facet(temporary.path(), "work");
    create_test_facet(temporary.path(), "home");
    create_test_facet(temporary.path(), "play");
    create_test_facet(temporary.path(), "other_facet");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work",
        json!({"entity_id": "target", "detached": true, "detached_by": "hide", "role": "w"}),
    );
    write_facet_relationship(
        temporary.path(),
        "home",
        "rel_home",
        json!({"entity_id": "target", "detached": true, "detached_by": "hide", "role": "h"}),
    );
    write_facet_relationship(
        temporary.path(),
        "play",
        "rel_play",
        json!({"entity_id": "target", "detached": true, "role": "p"}),
    );
    write_facet_relationship(
        temporary.path(),
        "other_facet",
        "rel_other_marker",
        json!({"entity_id": "target", "detached": true, "detached_by": "other", "role": "o"}),
    );
    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_other_entity",
        json!({"entity_id": "other", "detached": true, "detached_by": "hide", "role": "oe"}),
    );

    let play_file = temporary
        .path()
        .join("facets/play/entities/rel_play/entity.json");
    let other_marker_file = temporary
        .path()
        .join("facets/other_facet/entities/rel_other_marker/entity.json");
    let other_entity_file = temporary
        .path()
        .join("facets/work/entities/rel_other_entity/entity.json");

    let play_before = fs::read(&play_file).unwrap();
    let other_marker_before = fs::read(&other_marker_file).unwrap();
    let other_entity_before = fs::read(&other_entity_file).unwrap();

    let report = unblock_journal_entity(temporary.path(), "target").unwrap();
    assert_eq!(report.facets_reattached, vec!["home", "work"]);
    assert!(report.relationships_unreadable.is_empty());

    let target_identity = journal_entity_value(temporary.path(), "target");
    assert_eq!(target_identity.get("blocked"), None);

    let rel_work = relationship_value(temporary.path(), "work", "rel_work");
    assert_eq!(rel_work.get("detached"), None);
    assert_eq!(rel_work.get("detached_by"), None);
    assert_eq!(rel_work["role"], "w");

    let rel_home = relationship_value(temporary.path(), "home", "rel_home");
    assert_eq!(rel_home.get("detached"), None);
    assert_eq!(rel_home.get("detached_by"), None);
    assert_eq!(rel_home["role"], "h");

    assert_eq!(fs::read(&play_file).unwrap(), play_before);
    assert_eq!(fs::read(&other_marker_file).unwrap(), other_marker_before);
    assert_eq!(fs::read(&other_entity_file).unwrap(), other_entity_before);
}

#[test]
fn old_build_hide_unhide_clears_blocked_and_reports_empty() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target", "blocked": true}),
    );
    create_test_facet(temporary.path(), "work");
    create_test_facet(temporary.path(), "home");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work",
        json!({"entity_id": "target", "detached": true, "role": "w"}),
    );
    write_facet_relationship(
        temporary.path(),
        "home",
        "rel_home",
        json!({"entity_id": "target", "detached": true, "role": "h"}),
    );

    let work_file = temporary
        .path()
        .join("facets/work/entities/rel_work/entity.json");
    let home_file = temporary
        .path()
        .join("facets/home/entities/rel_home/entity.json");
    let work_before = fs::read(&work_file).unwrap();
    let home_before = fs::read(&home_file).unwrap();

    let report = unblock_journal_entity(temporary.path(), "target").unwrap();
    assert!(report.facets_reattached.is_empty());
    assert!(report.relationships_unreadable.is_empty());

    let target_identity = journal_entity_value(temporary.path(), "target");
    assert_eq!(target_identity.get("blocked"), None);

    assert_eq!(fs::read(&work_file).unwrap(), work_before);
    assert_eq!(fs::read(&home_file).unwrap(), home_before);
}

#[test]
fn already_hidden_fixture_marks_only_attached_ones() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target", "blocked": true}),
    );
    create_test_facet(temporary.path(), "work");
    create_test_facet(temporary.path(), "home");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_marked",
        json!({"entity_id": "target", "detached": true, "detached_by": "hide", "role": "marked"}),
    );
    write_facet_relationship(
        temporary.path(),
        "home",
        "rel_attached",
        json!({"entity_id": "target", "role": "attached"}),
    );

    let marked_file = temporary
        .path()
        .join("facets/work/entities/rel_marked/entity.json");
    let marked_before = fs::read(&marked_file).unwrap();

    let report = block_journal_entity(temporary.path(), "target").unwrap();
    assert_eq!(report.facets_detached, vec!["home"]);

    let rel_attached = relationship_value(temporary.path(), "home", "rel_attached");
    assert_eq!(rel_attached["detached"], true);
    assert_eq!(rel_attached["detached_by"], "hide");

    assert_eq!(fs::read(&marked_file).unwrap(), marked_before);
}

#[test]
fn unhide_hook_panic_leaves_entity_hidden_and_cleared_markers_and_rerun_completes() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target", "blocked": true}),
    );
    create_test_facet(temporary.path(), "work");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work",
        json!({"entity_id": "target", "detached": true, "detached_by": "hide"}),
    );

    let panic_result = catch_unwind(AssertUnwindSafe(|| {
        let _ = unblock_journal_entity_with_hook(temporary.path(), "target", || {
            panic!("unhide hook panic");
        });
    }));
    assert!(panic_result.is_err());

    let target_identity = journal_entity_value(temporary.path(), "target");
    assert_eq!(target_identity["blocked"], true);

    let rel_work = relationship_value(temporary.path(), "work", "rel_work");
    assert_eq!(rel_work.get("detached"), None);
    assert_eq!(rel_work.get("detached_by"), None);

    let report = unblock_journal_entity(temporary.path(), "target").unwrap();
    assert!(report.facets_reattached.is_empty());

    let final_identity = journal_entity_value(temporary.path(), "target");
    assert_eq!(final_identity.get("blocked"), None);
}

#[test]
fn partially_reattached_fixture_unhide_reattaches_only_marked_ones() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target", "blocked": true}),
    );
    create_test_facet(temporary.path(), "work");
    create_test_facet(temporary.path(), "home");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work",
        json!({"entity_id": "target", "role": "reattached"}),
    );
    write_facet_relationship(
        temporary.path(),
        "home",
        "rel_home",
        json!({"entity_id": "target", "detached": true, "detached_by": "hide", "role": "marked"}),
    );

    let work_file = temporary
        .path()
        .join("facets/work/entities/rel_work/entity.json");
    let work_before = fs::read(&work_file).unwrap();

    let report = unblock_journal_entity(temporary.path(), "target").unwrap();
    assert_eq!(report.facets_reattached, vec!["home"]);

    let rel_home = relationship_value(temporary.path(), "home", "rel_home");
    assert_eq!(rel_home.get("detached"), None);
    assert_eq!(rel_home.get("detached_by"), None);

    assert_eq!(fs::read(&work_file).unwrap(), work_before);

    let target_identity = journal_entity_value(temporary.path(), "target");
    assert_eq!(target_identity.get("blocked"), None);
}

#[test]
fn detach_facet_entity_on_marked_attached_relationship_clears_marker() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity(temporary.path(), "target", Some("target"));
    create_test_facet(temporary.path(), "work");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work",
        json!({"entity_id": "target", "detached_by": "hide", "role": "dev"}),
    );

    detach_facet_entity(temporary.path(), "work", "target").unwrap();

    let rel_work = relationship_value(temporary.path(), "work", "rel_work");
    assert_eq!(rel_work["detached"], true);
    assert_eq!(rel_work.get("detached_by"), None);

    let report = block_journal_entity(temporary.path(), "target").unwrap();
    assert!(report.facets_detached.is_empty());

    let unblock_report = unblock_journal_entity(temporary.path(), "target").unwrap();
    assert!(unblock_report.facets_reattached.is_empty());

    let final_rel = relationship_value(temporary.path(), "work", "rel_work");
    assert_eq!(final_rel["detached"], true);
    assert_eq!(final_rel.get("detached_by"), None);
}

#[test]
fn move_facet_entity_folds_marked_relationship_and_drops_marker() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Ada", "blocked": true}),
    );
    create_test_facet(temporary.path(), "from_facet");
    create_test_facet(temporary.path(), "to_facet");

    write_facet_relationship(
        temporary.path(),
        "from_facet",
        "rel_from",
        json!({"entity_id": "target", "name": "Ada", "detached": true, "detached_by": "hide"}),
    );
    write_facet_relationship(
        temporary.path(),
        "to_facet",
        "target",
        json!({"entity_id": "target", "name": "Ada", "detached": true}),
    );

    move_facet_entity(temporary.path(), "Ada", "from_facet", "to_facet", true).unwrap();

    let folded_rel = relationship_value(temporary.path(), "to_facet", "target");
    assert_eq!(folded_rel["detached"], true);
    assert_eq!(folded_rel.get("detached_by"), None);

    let report = unblock_journal_entity(temporary.path(), "target").unwrap();
    assert!(report.facets_reattached.is_empty());

    let final_rel = relationship_value(temporary.path(), "to_facet", "target");
    assert_eq!(final_rel["detached"], true);
    assert_eq!(final_rel.get("detached_by"), None);
}

#[test]
fn hide_with_unparseable_entity_json_fails_and_leaves_files_byte_identical() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target"}),
    );
    create_test_facet(temporary.path(), "work");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_target",
        json!({"entity_id": "target", "role": "engineer"}),
    );
    let target_identity_file = temporary.path().join("entities/target/entity.json");
    let target_rel_file = temporary
        .path()
        .join("facets/work/entities/rel_target/entity.json");

    let identity_before = fs::read(&target_identity_file).unwrap();
    let rel_before = fs::read(&target_rel_file).unwrap();

    let bad_file = temporary
        .path()
        .join("facets/work/entities/rel_bad/entity.json");
    fs::create_dir_all(bad_file.parent().unwrap()).unwrap();
    fs::write(&bad_file, b"not valid json").unwrap();

    let err = block_journal_entity(temporary.path(), "target").unwrap_err();
    assert!(matches!(
        err,
        FacetEntityLifecycleError::RelationshipsUnreadable { ref path }
            if path == "facets/work/entities/rel_bad/entity.json"
    ));

    assert_eq!(fs::read(&target_identity_file).unwrap(), identity_before);
    assert_eq!(fs::read(&target_rel_file).unwrap(), rel_before);
}

#[test]
fn unhide_with_unparseable_entity_json_skips_and_reattaches_readable() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target", "blocked": true}),
    );
    create_test_facet(temporary.path(), "work");
    create_test_facet(temporary.path(), "bad_facet");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_target",
        json!({"entity_id": "target", "detached": true, "detached_by": "hide"}),
    );

    let bad_file = temporary
        .path()
        .join("facets/bad_facet/entities/rel_bad/entity.json");
    fs::create_dir_all(bad_file.parent().unwrap()).unwrap();
    fs::write(&bad_file, b"corrupt json").unwrap();
    let bad_bytes_before = fs::read(&bad_file).unwrap();

    let report = unblock_journal_entity(temporary.path(), "target").unwrap();
    assert_eq!(report.facets_reattached, vec!["work"]);
    assert_eq!(
        report.relationships_unreadable,
        vec!["facets/bad_facet/entities/rel_bad/entity.json"]
    );

    let target_identity = journal_entity_value(temporary.path(), "target");
    assert_eq!(target_identity.get("blocked"), None);

    let rel_target = relationship_value(temporary.path(), "work", "rel_target");
    assert_eq!(rel_target.get("detached"), None);
    assert_eq!(rel_target.get("detached_by"), None);

    assert_eq!(fs::read(&bad_file).unwrap(), bad_bytes_before);
}

#[test]
fn hide_of_visible_entity_strips_detached_by_from_already_detached_relationship() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target"}),
    );
    create_test_facet(temporary.path(), "work");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work",
        json!({"entity_id": "target", "detached": true, "detached_by": "hide"}),
    );

    let report = block_journal_entity(temporary.path(), "target").unwrap();
    assert!(report.facets_detached.is_empty());

    let target_identity = journal_entity_value(temporary.path(), "target");
    assert_eq!(target_identity["blocked"], true);

    let rel_work = relationship_value(temporary.path(), "work", "rel_work");
    assert_eq!(rel_work["detached"], true);
    assert_eq!(rel_work.get("detached_by"), None);

    let unblock_report = unblock_journal_entity(temporary.path(), "target").unwrap();
    assert!(unblock_report.facets_reattached.is_empty());

    let final_rel = relationship_value(temporary.path(), "work", "rel_work");
    assert_eq!(final_rel["detached"], true);
    assert_eq!(final_rel.get("detached_by"), None);
}

#[test]
fn unhide_of_visible_entity_returns_entity_not_blocked_and_leaves_files_byte_identical() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target"}),
    );
    create_test_facet(temporary.path(), "work");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work",
        json!({"entity_id": "target", "detached": true, "detached_by": "hide"}),
    );

    let identity_file = temporary.path().join("entities/target/entity.json");
    let rel_file = temporary
        .path()
        .join("facets/work/entities/rel_work/entity.json");
    let identity_before = fs::read(&identity_file).unwrap();
    let rel_before = fs::read(&rel_file).unwrap();

    let err = unblock_journal_entity(temporary.path(), "target").unwrap_err();
    assert!(matches!(
        err,
        FacetEntityLifecycleError::Entity(EntityLifecycleError::EntityNotBlocked { entity_id })
            if entity_id == "target"
    ));

    assert_eq!(fs::read(&identity_file).unwrap(), identity_before);
    assert_eq!(fs::read(&rel_file).unwrap(), rel_before);
}

#[test]
fn hook_panic_during_hide_strips_marker_before_hook_and_rerun_completes() {
    let temporary = tempfile::tempdir().unwrap();
    write_journal_entity_with_fields(
        temporary.path(),
        "target",
        json!({"id": "target", "name": "Target"}),
    );
    create_test_facet(temporary.path(), "work");
    create_test_facet(temporary.path(), "home");

    write_facet_relationship(
        temporary.path(),
        "work",
        "rel_work_attached",
        json!({"entity_id": "target", "role": "attached"}),
    );
    write_facet_relationship(
        temporary.path(),
        "home",
        "rel_home_detached",
        json!({"entity_id": "target", "detached": true, "detached_by": "hide"}),
    );

    let panic_result = catch_unwind(AssertUnwindSafe(|| {
        let _ = block_journal_entity_with_hook(temporary.path(), "target", || {
            let identity = journal_entity_value(temporary.path(), "target");
            assert_eq!(identity["blocked"], true);

            let rel_home = relationship_value(temporary.path(), "home", "rel_home_detached");
            assert_eq!(rel_home["detached"], true);
            assert_eq!(rel_home.get("detached_by"), None);

            let rel_work = relationship_value(temporary.path(), "work", "rel_work_attached");
            assert_eq!(rel_work.get("detached"), None);

            panic!("hide hook panic");
        });
    }));
    assert!(panic_result.is_err());

    let report = block_journal_entity(temporary.path(), "target").unwrap();
    assert_eq!(report.facets_detached, vec!["work"]);

    let rel_work = relationship_value(temporary.path(), "work", "rel_work_attached");
    assert_eq!(rel_work["detached"], true);
    assert_eq!(rel_work["detached_by"], "hide");

    let rel_home = relationship_value(temporary.path(), "home", "rel_home_detached");
    assert_eq!(rel_home["detached"], true);
    assert_eq!(rel_home.get("detached_by"), None);

    let unblock_report = unblock_journal_entity(temporary.path(), "target").unwrap();
    assert_eq!(unblock_report.facets_reattached, vec!["work"]);

    let final_work = relationship_value(temporary.path(), "work", "rel_work_attached");
    assert_eq!(final_work.get("detached"), None);
    assert_eq!(final_work.get("detached_by"), None);

    let final_home = relationship_value(temporary.path(), "home", "rel_home_detached");
    assert_eq!(final_home["detached"], true);
    assert_eq!(final_home.get("detached_by"), None);
}
