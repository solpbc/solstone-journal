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
    list_scoped_facet_entities, publish_review_aliases, update_facet_entity_identity,
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

/// Write one entity directory carrying a full identity document.
fn write_identity(root: &std::path::Path, dir: &str, identity: serde_json::Value) {
    write_journal_entity(root, dir, identity.get("id").and_then(|v| v.as_str()));
    fs::write(
        root.join(format!("entities/{dir}/entity.json")),
        serde_json::to_vec(&identity).unwrap(),
    )
    .unwrap();
}

#[test]
fn promotion_resolves_a_punctuated_identity_through_the_id_namespace() {
    // The reported wedge: the stored display name carries punctuation that
    // `normalize_resolution_query` preserves and `entity_slug` collapses, so
    // the plainly-phrased promotion matched no name and minted an id that was
    // already taken -- unmatchable and uncreatable at once, for good.
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    // The premise, asserted rather than assumed: both spellings derive one id,
    // and only one of them matches by name.
    assert_eq!(
        solstone_core_entity_matching::entity_slug("`make ci`"),
        solstone_core_entity_matching::entity_slug("make ci")
    );
    assert_ne!(
        solstone_core_entity_matching::normalize_resolution_query("`make ci`"),
        solstone_core_entity_matching::normalize_resolution_query("make ci")
    );
    write_identity(
        temporary.path(),
        "make_ci",
        json!({"id":"make_ci","name":"`make ci`","type":"Thing"}),
    );
    let promotion = crate::prepare_review_promotion(
        temporary.path(),
        "scope",
        "Thing",
        "make ci",
        "The repository gate",
        &["the gate".to_owned()],
    )
    .unwrap();
    assert_eq!(promotion.attachment.entity_id, "make_ci");
    assert_eq!(promotion.attachment.relationship_dir, "make_ci");
    assert!(promotion.attachment.before.is_none());
    // Adoption, not creation: no identity is minted, and the alias the
    // promotion carries lands on the stored document with its name intact.
    assert!(promotion.identity.is_none(), "{:?}", promotion.identity);
    let aliases = promotion.aliases.expect("alias change");
    assert_eq!(aliases.entity_dir, "make_ci");
    assert_eq!(
        aliases
            .before
            .as_ref()
            .and_then(|before| before.get("name")),
        Some(&json!("`make ci`"))
    );
    assert_eq!(aliases.after["aka"], json!(["the gate"]));
}

#[test]
fn promotion_reuses_a_punctuated_link_already_attached_to_the_facet() {
    // Same mismatch one scope in: resolving the facet's own entities by name
    // alone missed the link and the promotion died on the relationship
    // directory it was about to need.
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_identity(
        temporary.path(),
        "make_ci",
        json!({"id":"make_ci","name":"`make ci`","type":"Thing"}),
    );
    write_facet_relationship(
        temporary.path(),
        "scope",
        "make_ci",
        json!({"entity_id":"make_ci","description":"before"}),
    );
    let promotion = crate::prepare_review_promotion(
        temporary.path(),
        "scope",
        "Thing",
        "make ci",
        "The repository gate",
        &[],
    )
    .unwrap();
    assert_eq!(promotion.attachment.entity_id, "make_ci");
    assert_eq!(
        promotion
            .attachment
            .before
            .as_ref()
            .and_then(|b| b.get("description")),
        Some(&json!("before"))
    );
}

#[test]
fn review_alias_publication_ignores_inherited_conflicts_but_rechecks_additions() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    for (dir, id, name, aliases) in [
        ("target", "target", "Target", vec!["Inherited"]),
        ("claimant", "claimant", "Inherited", vec![]),
    ] {
        write_identity(
            temporary.path(),
            dir,
            json!({"id":id,"name":name,"type":"Person","aka":aliases}),
        );
        write_facet_relationship(temporary.path(), "scope", dir, json!({"entity_id":id}));
    }

    let before = crate::read_facet_entity_link(temporary.path(), "scope", "target")
        .unwrap()
        .expect("target relationship");
    assert_eq!(before.value()["entity_id"], "target");
    let current = solstone_core_entity::read_entity_identity(temporary.path(), "target")
        .unwrap()
        .unwrap()
        .value()
        .clone();
    let mut after = current.clone();
    after["aka"] = json!(["Inherited", "One", "Two", "Three", "Four"]);
    let change = solstone_core_entity::prepare_identity_changes(
        temporary.path(),
        "target",
        &[current, after.clone()],
    )
    .unwrap()
    .into_iter()
    .find(|candidate| candidate.after == after)
    .expect("prepared alias change");

    let mut started = false;
    let mut committed = false;
    publish_review_aliases(
        temporary.path(),
        "scope",
        &change,
        true,
        || {
            started = true;
            Ok(())
        },
        || {
            committed = true;
            Ok(())
        },
    )
    .unwrap();
    assert!(started);
    assert!(committed);
    let stored = solstone_core_entity::read_entity_identity(temporary.path(), "target")
        .unwrap()
        .unwrap();
    assert_eq!(stored.value(), &after);

    let mut later_after = after.clone();
    later_after["aka"] = json!(["Inherited", "One", "Two", "Three", "Four", "Late Claim"]);
    let later = solstone_core_entity::prepare_identity_changes(
        temporary.path(),
        "target",
        &[after.clone(), later_after],
    )
    .unwrap()
    .into_iter()
    .next()
    .expect("prepared later alias change");
    write_identity(
        temporary.path(),
        "late_claimant",
        json!({"id":"late_claimant","name":"Other","type":"Person","aka":["Late Claim"]}),
    );
    write_facet_relationship(
        temporary.path(),
        "scope",
        "late_claimant",
        json!({"entity_id":"late_claimant"}),
    );
    let mut refused_start = false;
    let mut refused_receipt = false;
    let error = publish_review_aliases(
        temporary.path(),
        "scope",
        &later,
        true,
        || {
            refused_start = true;
            Ok(())
        },
        || {
            refused_receipt = true;
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(
        error,
        "conflict: promotion alias was claimed after preparation"
    );
    assert_eq!(
        error.kind(),
        Some(solstone_core_entity::ReviewOwnerConflictKind::AliasClaimed)
    );
    assert!(!refused_start);
    assert!(!refused_receipt);
    let unchanged = solstone_core_entity::read_entity_identity(temporary.path(), "target")
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.value(), &after);
}

#[test]
fn review_alias_delta_membership_uses_resolution_normalization() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    for (dir, id, name, aliases) in [
        ("target", "target", "Target", vec!["Straße"]),
        ("claimant", "claimant", "STRASSE", vec![]),
    ] {
        write_identity(
            temporary.path(),
            dir,
            json!({"id":id,"name":name,"type":"Person","aka":aliases}),
        );
        write_facet_relationship(temporary.path(), "scope", dir, json!({"entity_id":id}));
    }
    let before = json!({"id":"target","name":"Target","type":"Person","aka":["Straße"]});
    let after =
        json!({"id":"target","name":"Target","type":"Person","aka":["STRASSE","Safe Alias"]});
    let change = solstone_core_entity::prepare_identity_changes(
        temporary.path(),
        "target",
        &[before, after.clone()],
    )
    .unwrap()
    .into_iter()
    .find(|candidate| candidate.after == after)
    .expect("prepared normalized alias change");

    publish_review_aliases(
        temporary.path(),
        "scope",
        &change,
        true,
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    let stored = solstone_core_entity::read_entity_identity(temporary.path(), "target")
        .unwrap()
        .unwrap();
    assert_eq!(stored.value(), &after);
}

#[test]
fn promotion_prefers_a_name_match_over_an_id_match() {
    // Ordering is the guard against the id namespace swallowing a real name:
    // the display name decides, and the id is only consulted when it cannot.
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_identity(
        temporary.path(),
        "make_ci",
        json!({"id":"make_ci","name":"`make ci`","type":"Thing"}),
    );
    write_identity(
        temporary.path(),
        "other_dir",
        json!({"id":"other_id","name":"Make CI","type":"Thing"}),
    );
    let promotion = crate::prepare_review_promotion(
        temporary.path(),
        "scope",
        "Thing",
        "make ci",
        "The repository gate",
        &[],
    )
    .unwrap();
    assert_eq!(promotion.attachment.entity_id, "other_id");
}

#[test]
fn promotion_still_refuses_a_blocked_identity_reached_through_the_id_namespace() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_identity(
        temporary.path(),
        "make_ci",
        json!({"id":"make_ci","name":"`make ci`","type":"Thing","blocked":true}),
    );
    let error = crate::prepare_review_promotion(
        temporary.path(),
        "scope",
        "Thing",
        "make ci",
        "The repository gate",
        &[],
    )
    .unwrap_err();
    assert!(error.to_string().starts_with("conflict:"), "{error}");
    assert!(error.to_string().contains("blocked"), "{error}");
    assert_eq!(
        error.kind(),
        Some(solstone_core_entity::ReviewOwnerConflictKind::PromotedEntityBlocked)
    );
}

#[test]
fn promotion_still_mints_an_identity_no_namespace_holds() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_identity(
        temporary.path(),
        "make_ci",
        json!({"id":"make_ci","name":"`make ci`","type":"Thing"}),
    );
    let promotion = crate::prepare_review_promotion(
        temporary.path(),
        "scope",
        "Thing",
        "Grace Hopper",
        "Engineer",
        &[],
    )
    .unwrap();
    assert_eq!(promotion.attachment.entity_id, "grace_hopper");
    assert_eq!(
        promotion
            .identity
            .as_ref()
            .and_then(|change| change.before.clone()),
        None
    );
}

#[test]
fn promotion_snapshot_covers_the_id_namespace_the_promotion_resolves_against() {
    // A freeze that cannot see the namespace the resolution reads reports no
    // change over exactly the state that decides the outcome.
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    let before = crate::review_promotion_snapshot(temporary.path(), "scope", "make ci").unwrap();
    write_identity(
        temporary.path(),
        "make_ci",
        json!({"id":"make_ci","name":"`make ci`","type":"Thing"}),
    );
    let after = crate::review_promotion_snapshot(temporary.path(), "scope", "make ci").unwrap();
    assert_ne!(before, after);
    assert_eq!(after["identities"][0]["directory"], json!("make_ci"));
}

#[test]
fn promotion_disambiguates_a_duplicate_name_family_by_the_id_it_derives() {
    // The store minted `_2`/`_3` suffixes for same-named identities, so the
    // name alone counts several and the id names exactly one.
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    for dir in [
        "weekly_reflection_3",
        "weekly_reflection",
        "weekly_reflection_2",
    ] {
        write_identity(
            temporary.path(),
            dir,
            json!({"id":dir,"name":"Weekly Reflection","type":"Thing"}),
        );
    }
    let promotion = crate::prepare_review_promotion(
        temporary.path(),
        "scope",
        "Thing",
        "Weekly Reflection",
        "A recurring review",
        &[],
    )
    .unwrap();
    assert_eq!(promotion.attachment.entity_id, "weekly_reflection");
}

#[test]
fn promotion_still_refuses_a_duplicate_name_family_no_member_can_claim() {
    // Ambiguity stays refused where it is still the honest answer: no member
    // of the family holds the id the promoted name derives.
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    for dir in ["weekly_reflection_2", "weekly_reflection_3"] {
        write_identity(
            temporary.path(),
            dir,
            json!({"id":dir,"name":"Weekly Reflection","type":"Thing"}),
        );
    }
    let error = crate::prepare_review_promotion(
        temporary.path(),
        "scope",
        "Thing",
        "Weekly Reflection",
        "A recurring review",
        &[],
    )
    .unwrap_err();
    assert_eq!(
        error,
        "conflict: promotion \"Weekly Reflection\" matches 2 identities by name"
    );
    assert_eq!(
        error.kind(),
        Some(solstone_core_entity::ReviewOwnerConflictKind::NameMatchesMultiple)
    );
}

#[test]
fn promotion_still_refuses_a_shared_name_held_by_an_identity_map_collision_loser() {
    // The boundary the duplicate-family tie-break did NOT move, pinned because
    // narrowing the ambiguity refusal is what makes this one load-bearing: two
    // directories claiming one id are a collision, not a family, and the loser
    // carrying the same display name is refused before any tie-break runs.
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    for dir in ["alpha_dir", "beta_dir"] {
        write_identity(
            temporary.path(),
            dir,
            json!({"id":"weekly_reflection","name":"Weekly Reflection","type":"Thing"}),
        );
    }
    let error = crate::prepare_review_promotion(
        temporary.path(),
        "scope",
        "Thing",
        "Weekly Reflection",
        "A recurring review",
        &[],
    )
    .unwrap_err();
    assert_eq!(
        error,
        "conflict: promotion \"Weekly Reflection\" matches \"beta_dir\", which lost its identity-map group"
    );
    assert_eq!(
        error.kind(),
        Some(solstone_core_entity::ReviewOwnerConflictKind::IdentityMapGroupLost)
    );
}

fn read_identity(root: &std::path::Path, id: &str) -> serde_json::Value {
    serde_json::from_slice(&fs::read(root.join("entities").join(id).join("entity.json")).unwrap())
        .unwrap()
}

fn links_to(root: &std::path::Path, facet: &str, entity_id: &str) -> usize {
    fs::read_dir(root.join("facets").join(facet).join("entities"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    fs::read(entry.path().join("entity.json"))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                        .is_some_and(|link| link["entity_id"] == entity_id)
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn attaching_a_name_whose_id_is_live_never_rewrites_that_identity() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    let jane = json!({
        "id": "jane",
        "name": "Jane Doe",
        "type": "Person",
        "aka": ["JD"],
        "emails": ["jane@example.com"],
        "is_principal": true,
    });
    write_identity(temporary.path(), "jane", jane.clone());

    let result =
        attach_or_reactivate_entity(temporary.path(), "work", "Person", "Jane", "a note").unwrap();
    assert!(!result.reactivated);
    assert_eq!(result.relationship["entity_id"], "jane");
    assert_eq!(read_identity(temporary.path(), "jane"), jane);
    assert_eq!(links_to(temporary.path(), "work", "jane"), 1);

    // Attached already: exactly one link stays.
    assert!(matches!(
        attach_or_reactivate_entity(temporary.path(), "work", "Person", "Jane", ""),
        Err(FacetEntityWriteError::EntityExists { .. })
    ));
    assert_eq!(links_to(temporary.path(), "work", "jane"), 1);
}

#[test]
fn attaching_by_live_id_finds_an_existing_link_under_any_folder() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_identity(
        temporary.path(),
        "jane",
        json!({"id":"jane","name":"Jane Doe"}),
    );
    write_facet_relationship(
        temporary.path(),
        "work",
        "jane_doe",
        json!({"entity_id":"jane","detached":true}),
    );
    let result =
        attach_or_reactivate_entity(temporary.path(), "work", "Person", "Jane", "").unwrap();
    assert!(result.reactivated);
    assert_eq!(links_to(temporary.path(), "work", "jane"), 1);
    assert!(!temporary.path().join("facets/work/entities/jane").exists());
}

#[test]
fn attaching_by_live_id_refuses_a_blocked_entity_and_an_occupied_folder() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_identity(
        temporary.path(),
        "jane",
        json!({"id":"jane","name":"Jane Doe","blocked":true}),
    );
    assert!(matches!(
        attach_or_reactivate_entity(temporary.path(), "work", "Person", "Jane", ""),
        Err(FacetEntityWriteError::EntityBlocked { .. })
    ));
    assert_eq!(read_identity(temporary.path(), "jane")["blocked"], true);

    write_identity(
        temporary.path(),
        "jane",
        json!({"id":"jane","name":"Jane Doe"}),
    );
    write_facet_relationship(
        temporary.path(),
        "work",
        "jane",
        json!({"entity_id":"someone_else"}),
    );
    assert!(matches!(
        attach_or_reactivate_entity(temporary.path(), "work", "Person", "Jane", ""),
        Err(FacetEntityWriteError::RelationshipOccupied { .. })
    ));
    assert_eq!(
        relationship_value(temporary.path(), "work", "jane")["entity_id"],
        "someone_else"
    );
}

#[test]
fn a_merged_name_is_never_created_again_by_attach() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_identity(
        temporary.path(),
        "solstone",
        json!({"id":"solstone","name":"Solstone"}),
    );
    fs::write(
        temporary.path().join("entities/retired.json"),
        serde_json::to_vec(
            &json!({"ids":{"sunstone":{"state":"merged","dir":"sunstone","successor":"solstone"}}}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(matches!(
        attach_or_reactivate_entity(temporary.path(), "work", "Project", "Sunstone", ""),
        Err(FacetEntityWriteError::EntityWrite(
            solstone_core_entity::EntityWriteError::IdentityMerged { .. }
        ))
    ));
    assert!(!temporary.path().join("entities/sunstone").exists());
    assert!(
        !temporary
            .path()
            .join("facets/work/entities/sunstone")
            .exists()
    );
}

#[test]
fn a_merge_landing_between_prepare_and_publish_ends_the_promotion_as_a_conflict() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    let promotion = crate::prepare_review_promotion(
        temporary.path(),
        "scope",
        "Project",
        "Sunstone",
        "An earlier project name",
        &[],
    )
    .unwrap();
    let change = promotion.identity.expect("a new identity is prepared");
    assert_eq!(change.entity_id, "sunstone");
    // A merge lands and retires the id before the plan is published.
    write_identity(
        temporary.path(),
        "solstone",
        json!({"id":"solstone","name":"Solstone"}),
    );
    fs::write(
        temporary.path().join("entities/retired.json"),
        serde_json::to_vec(
            &json!({"ids":{"sunstone":{"state":"merged","dir":"sunstone","successor":"solstone"}}}),
        )
        .unwrap(),
    )
    .unwrap();
    let error = solstone_core_entity::publish_identity_change(
        temporary.path(),
        &change,
        false,
        || Ok(()),
        || Ok(()),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        solstone_core_entity::ReviewOwnerError::Conflict {
            kind: solstone_core_entity::ReviewOwnerConflictKind::IdentityMerged,
            ..
        }
    ));
    assert!(!temporary.path().join("entities/sunstone").exists());
}
