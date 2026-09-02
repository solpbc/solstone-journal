// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;

use serde_json::json;

use crate::store_tests::{
    TempDir, create_test_facet, relationship_value, write_facet_relationship, write_journal_entity,
};
use crate::{
    FacetEntityWriteError, add_entity_aka, attach_or_reactivate_entity, detach_facet_entity,
    list_scoped_facet_entities, update_facet_entity_identity,
};

#[test]
fn attach_reactivates_full_case_fold_name_match() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_journal_entity(temporary.path(), "opaque_dir", Some("opaque_identity"));
    let identity = temporary.path().join("entities/opaque_dir/entity.json");
    fs::write(
        &identity,
        serde_json::to_vec(&json!({"id":"opaque_identity","name":"STRASSE HANDEL","type":"old"}))
            .unwrap(),
    )
    .unwrap();
    write_facet_relationship(
        temporary.path(),
        "scope",
        "memory",
        json!({"entity_id":"opaque_identity","detached":true,"description":"before"}),
    );
    let result =
        attach_or_reactivate_entity(temporary.path(), "scope", "new", "Straße Handel", "after")
            .unwrap();
    assert!(result.reactivated);
    assert_eq!(
        relationship_value(temporary.path(), "scope", "memory")["description"],
        "after"
    );
    assert!(
        relationship_value(temporary.path(), "scope", "memory")
            .get("detached")
            .is_none()
    );
    let identity: serde_json::Value =
        serde_json::from_slice(&fs::read(&identity).unwrap()).unwrap();
    assert_eq!(identity["type"], "new");
}

#[test]
fn scoped_list_joins_stored_link_identity_and_filters_independently() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_journal_entity(temporary.path(), "real_dir", Some("written"));
    fs::write(
        temporary.path().join("entities/real_dir/entity.json"),
        serde_json::to_vec(&json!({"id":"written","name":"real","blocked":true})).unwrap(),
    )
    .unwrap();
    write_facet_relationship(
        temporary.path(),
        "scope",
        "other_dir",
        json!({"entity_id":"written","detached":false}),
    );
    assert!(
        list_scoped_facet_entities(temporary.path(), "scope", false, false)
            .unwrap()
            .is_empty()
    );
    let entities = list_scoped_facet_entities(temporary.path(), "scope", false, true).unwrap();
    assert_eq!(entities[0].identity["name"], "real");
    assert!(entities[0].blocked);
    assert!(entities[0].relationship.get("blocked").is_none());
}

#[test]
fn scoped_list_filters_all_detached_and_blocked_combinations_independently() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    for (index, detached, blocked) in [
        (0, false, false),
        (1, true, false),
        (2, false, true),
        (3, true, true),
    ] {
        let id = format!("id{index}");
        let dir = format!("dir{index}");
        let link = format!("link{index}");
        write_journal_entity(temporary.path(), &dir, Some(&id));
        fs::write(
            temporary.path().join(format!("entities/{dir}/entity.json")),
            serde_json::to_vec(&json!({"id":id,"name":format!("name{index}"),"blocked":blocked}))
                .unwrap(),
        )
        .unwrap();
        write_facet_relationship(
            temporary.path(),
            "scope",
            &link,
            json!({"entity_id":id,"detached":detached}),
        );
    }
    assert_eq!(
        list_scoped_facet_entities(temporary.path(), "scope", false, false)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        list_scoped_facet_entities(temporary.path(), "scope", true, false)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        list_scoped_facet_entities(temporary.path(), "scope", false, true)
            .unwrap()
            .len(),
        2
    );
    let all = list_scoped_facet_entities(temporary.path(), "scope", true, true).unwrap();
    assert_eq!(all.len(), 4);
    assert!(all.iter().any(|entity| entity.detached && entity.blocked));
}

#[test]
fn attach_map_loser_refuses_and_winner_attaches() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_journal_entity(temporary.path(), "a_winner", Some("shared"));
    write_journal_entity(temporary.path(), "z_loser", Some("shared"));
    fs::write(
        temporary.path().join("entities/a_winner/entity.json"),
        serde_json::to_vec(&json!({"id":"shared","name":"winner"})).unwrap(),
    )
    .unwrap();
    fs::write(
        temporary.path().join("entities/z_loser/entity.json"),
        serde_json::to_vec(&json!({"id":"shared","name":"loser"})).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        attach_or_reactivate_entity(temporary.path(), "scope", "kind", "loser", ""),
        Err(FacetEntityWriteError::IdentityMapLoser { .. })
    ));
    assert!(
        !temporary
            .path()
            .join("facets/scope/entities/loser")
            .exists()
    );
    assert!(
        !attach_or_reactivate_entity(temporary.path(), "scope", "kind", "winner", "")
            .unwrap()
            .reactivated
    );
}

#[test]
fn detach_does_not_change_journal_identity() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_journal_entity(temporary.path(), "entity", Some("id"));
    write_facet_relationship(temporary.path(), "scope", "link", json!({"entity_id":"id"}));
    let before = fs::read(temporary.path().join("entities/entity/entity.json")).unwrap();
    detach_facet_entity(temporary.path(), "scope", "id").unwrap();
    assert_eq!(
        relationship_value(temporary.path(), "scope", "link")["detached"],
        true
    );
    assert_eq!(
        fs::read(temporary.path().join("entities/entity/entity.json")).unwrap(),
        before
    );
}

#[test]
fn attach_covers_blocked_active_detached_and_existing_journal_outcomes() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    for (dir, id, name, blocked) in [
        ("active", "active", "active", false),
        ("blocked_link", "blocked_link", "blocked link", true),
        (
            "blocked_journal",
            "blocked_journal",
            "blocked journal",
            true,
        ),
        ("elsewhere", "opaque", "elsewhere", false),
    ] {
        write_journal_entity(temporary.path(), dir, Some(id));
        fs::write(
            temporary.path().join(format!("entities/{dir}/entity.json")),
            serde_json::to_vec(&json!({"id":id,"name":name,"type":"kind","blocked":blocked}))
                .unwrap(),
        )
        .unwrap();
    }
    write_facet_relationship(
        temporary.path(),
        "scope",
        "active",
        json!({"entity_id":"active"}),
    );
    write_facet_relationship(
        temporary.path(),
        "scope",
        "blocked_link",
        json!({"entity_id":"blocked_link"}),
    );
    assert!(matches!(
        attach_or_reactivate_entity(temporary.path(), "scope", "kind", "active", ""),
        Err(FacetEntityWriteError::EntityExists { .. })
    ));
    assert!(matches!(
        attach_or_reactivate_entity(temporary.path(), "scope", "kind", "blocked link", ""),
        Err(FacetEntityWriteError::EntityBlocked { .. })
    ));
    assert!(matches!(
        attach_or_reactivate_entity(temporary.path(), "scope", "kind", "blocked journal", ""),
        Err(FacetEntityWriteError::EntityBlocked { .. })
    ));
    let link_count_before = fs::read_dir(temporary.path().join("facets/scope/entities"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count();
    let entity_count_before = fs::read_dir(temporary.path().join("entities"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count();
    let before = fs::read(temporary.path().join("entities/elsewhere/entity.json")).unwrap();
    let result =
        attach_or_reactivate_entity(temporary.path(), "scope", "kind", "elsewhere", "").unwrap();
    assert!(!result.reactivated);
    assert_eq!(
        relationship_value(temporary.path(), "scope", "elsewhere")["entity_id"],
        "opaque"
    );
    assert_eq!(
        fs::read(temporary.path().join("entities/elsewhere/entity.json")).unwrap(),
        before
    );
    assert_eq!(
        fs::read_dir(temporary.path().join("facets/scope/entities"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .count(),
        link_count_before + 1
    );
    assert_eq!(
        fs::read_dir(temporary.path().join("entities"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .count(),
        entity_count_before
    );
}

#[test]
fn reactivation_keeps_empty_description_and_matching_type_identity_bytes() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_journal_entity(temporary.path(), "entity", Some("id"));
    let identity = temporary.path().join("entities/entity/entity.json");
    fs::write(
        &identity,
        serde_json::to_vec(&json!({"id":"id","name":"subject","type":"kind"})).unwrap(),
    )
    .unwrap();
    write_facet_relationship(
        temporary.path(),
        "scope",
        "subject",
        json!({"entity_id":"id","detached":true,"description":"kept"}),
    );
    let before = fs::read(&identity).unwrap();
    attach_or_reactivate_entity(temporary.path(), "scope", "kind", "subject", "").unwrap();
    assert_eq!(
        relationship_value(temporary.path(), "scope", "subject")["description"],
        "kept"
    );
    assert_eq!(fs::read(&identity).unwrap(), before);
}

#[test]
fn fresh_attach_creates_only_when_no_written_name_matches() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    assert!(!temporary.path().join("entities").exists());
    attach_or_reactivate_entity(temporary.path(), "scope", "kind", "fresh subject", "").unwrap();
    let directories = fs::read_dir(temporary.path().join("entities"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count();
    assert_eq!(directories, 1);
    assert!(matches!(
        attach_or_reactivate_entity(temporary.path(), "scope", "kind", "FRESH SUBJECT", ""),
        Err(FacetEntityWriteError::EntityExists { .. })
    ));
    assert_eq!(
        fs::read_dir(temporary.path().join("entities"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .count(),
        1
    );
}

#[test]
fn email_case_normalization_is_not_full_case_folding() {
    let email = "Straße@example.test";
    assert_eq!(email.to_lowercase(), "straße@example.test");
    assert_ne!(
        solstone_core_entity_matching::normalize_resolution_query(email),
        email.to_lowercase()
    );
}

#[test]
fn aka_does_not_conflict_with_a_blocked_entitys_name() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    for (dir, id, name, blocked) in [
        ("target", "target", "target", false),
        ("blocked", "blocked", "reserved", true),
    ] {
        write_journal_entity(temporary.path(), dir, Some(id));
        fs::write(
            temporary.path().join(format!("entities/{dir}/entity.json")),
            serde_json::to_vec(&json!({"id":id,"name":name,"blocked":blocked})).unwrap(),
        )
        .unwrap();
        write_facet_relationship(temporary.path(), "scope", dir, json!({"entity_id":id}));
    }
    // Blocking is what frees a name for reuse: the reference's guard filters
    // blocked candidates out before comparing, so an alias matching a blocked
    // entity's name is accepted. Verified against the reference by execution.
    assert!(add_entity_aka(temporary.path(), "scope", "target", "reserved").is_ok());
    update_facet_entity_identity(
        temporary.path(),
        "scope",
        "target",
        "target two",
        "",
        &["reserved".to_owned()],
    )
    .unwrap();
}
