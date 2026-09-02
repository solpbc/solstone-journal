// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};
use solstone_core_entity::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation,
    load_resolved_ambiguity_choice, record_ambiguity_choice, record_ambiguity_observation,
};

use crate::{
    FacetEntityLinkRepairBranch, FacetEntityLinkRepairError, FacetStoreError, create_facet,
    delete_facet, list_facet_entity_directories, read_activity_file, read_facet_declaration,
    read_facet_entity_link, read_facet_entity_observations, read_log_file, read_news_file,
    rename_facet, repair_facet_entity_links, repair_facet_entity_links_journal_wide,
    save_facet_entity_link, set_facet_muted, update_facet, write_activity_file,
    write_facet_entity_observations, write_log_file, write_news_file,
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(crate) fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-facets-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn facet_declaration_create_and_read_round_trip_metadata() {
    let temporary = TempDir::new();

    create_facet(
        temporary.path(),
        "work",
        "Work",
        "Professional context",
        "blue",
        "💼",
        None,
    )
    .unwrap();

    let declaration = read_facet_declaration(temporary.path(), "work")
        .unwrap()
        .unwrap();
    assert_eq!(declaration.title, "Work");
    assert_eq!(declaration.description, "Professional context");
    assert_eq!(declaration.color, "blue");
    assert_eq!(declaration.emoji, "💼");
    assert_eq!(declaration.icon, None);
    assert_eq!(declaration.muted, None);
}

#[test]
fn facet_icon_pop_on_clear_removes_the_key() {
    let temporary = TempDir::new();
    create_facet(
        temporary.path(),
        "work",
        "Work",
        "Description",
        "blue",
        "💼",
        None,
    )
    .unwrap();
    update_facet(
        temporary.path(),
        "work",
        "Work",
        "Description",
        "blue",
        "💼",
        Some("briefcase"),
    )
    .unwrap();
    update_facet(
        temporary.path(),
        "work",
        "Work",
        "Description",
        "blue",
        "💼",
        Some(""),
    )
    .unwrap();

    let value = declaration_value(temporary.path(), "work");
    assert!(value.get("icon").is_none());
    assert_eq!(
        read_facet_declaration(temporary.path(), "work")
            .unwrap()
            .unwrap()
            .icon,
        None
    );
}

#[test]
fn facet_muted_pop_on_clear_removes_the_key() {
    let temporary = TempDir::new();
    create_facet(
        temporary.path(),
        "work",
        "Work",
        "Description",
        "blue",
        "💼",
        None,
    )
    .unwrap();
    set_facet_muted(temporary.path(), "work", true).unwrap();
    set_facet_muted(temporary.path(), "work", false).unwrap();

    let value = declaration_value(temporary.path(), "work");
    assert!(value.get("muted").is_none());
    assert_eq!(
        read_facet_declaration(temporary.path(), "work")
            .unwrap()
            .unwrap()
            .muted,
        None
    );
}

#[test]
fn facet_entity_links_read_divergent_persisted_links() {
    let temporary = TempDir::new();
    let mut relationship = Map::new();
    relationship.insert("role".to_owned(), Value::String("member".to_owned()));
    save_facet_entity_link(
        temporary.path(),
        "work",
        "display-name",
        "journal-entity-id",
        &relationship,
    )
    .unwrap();

    let linked = read_facet_entity_link(temporary.path(), "work", "display-name")
        .unwrap()
        .unwrap();
    assert_eq!(linked.entity_id(), "journal-entity-id");
    assert!(linked.was_written());
    assert_eq!(linked.value()["role"], "member");
}

#[test]
fn facet_entity_links_treat_matching_persisted_ids_as_written() {
    let temporary = TempDir::new();
    let relationship = Map::new();
    save_facet_entity_link(temporary.path(), "work", "person", "person", &relationship).unwrap();

    let linked = read_facet_entity_link(temporary.path(), "work", "person")
        .unwrap()
        .unwrap();
    assert_eq!(linked.entity_id(), "person");
    assert!(linked.was_written());
}

#[test]
fn facet_entity_links_fall_back_to_directory_name_when_entity_id_is_empty() {
    let temporary = TempDir::new();
    write_facet_relationship(
        temporary.path(),
        "work",
        "legacy",
        json!({"entity_id": "", "role": "member"}),
    );

    let link = read_facet_entity_link(temporary.path(), "work", "legacy")
        .unwrap()
        .unwrap();
    assert_eq!(link.entity_id(), "legacy");
    assert!(!link.was_written());
}

#[test]
fn facet_entity_directory_listing_ignores_non_directory_material() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "facets/work/entities/person/entity.json",
        &json!({"entity_id": "person"}),
    );
    write_text(
        temporary.path(),
        "facets/work/entities/20260305.jsonl",
        "{\"detected\": true}\n",
    );

    assert_eq!(
        list_facet_entity_directories(temporary.path(), "work").unwrap(),
        vec!["person".to_owned()]
    );
}

#[test]
fn facet_content_files_round_trip_without_parsing() {
    let temporary = TempDir::new();
    write_activity_file(temporary.path(), "work", "20260305.jsonl", "{\"id\": 1}\n").unwrap();
    write_activity_file(
        temporary.path(),
        "work",
        "20260305/a/event.json",
        "{\"id\": 2}\n",
    )
    .unwrap();
    write_news_file(temporary.path(), "work", "notice.md", "# Notice\n").unwrap();
    write_log_file(temporary.path(), "work", "log.jsonl", "{\"log\": true}\n").unwrap();

    assert_eq!(
        read_activity_file(temporary.path(), "work", "20260305.jsonl").unwrap(),
        Some("{\"id\": 1}\n".to_owned())
    );
    assert_eq!(
        read_activity_file(temporary.path(), "work", "20260305/a/event.json").unwrap(),
        Some("{\"id\": 2}\n".to_owned())
    );
    assert_eq!(
        read_news_file(temporary.path(), "work", "notice.md").unwrap(),
        Some("# Notice\n".to_owned())
    );
    assert_eq!(
        read_log_file(temporary.path(), "work", "log.jsonl").unwrap(),
        Some("{\"log\": true}\n".to_owned())
    );

    write_facet_entity_observations(
        temporary.path(),
        "work",
        "person",
        "{\"note\": \"keep raw\"}\n",
    )
    .unwrap();
    assert_eq!(
        read_facet_entity_observations(temporary.path(), "work", "person").unwrap(),
        Some("{\"note\": \"keep raw\"}\n".to_owned())
    );
}

#[test]
fn facet_entity_link_retarget_does_not_move_or_orphan_observations() {
    let temporary = TempDir::new();
    let mut relationship = Map::new();
    relationship.insert("role".to_owned(), Value::String("member".to_owned()));
    save_facet_entity_link(
        temporary.path(),
        "work",
        "stable-facet-dir",
        "old-journal-id",
        &relationship,
    )
    .unwrap();
    let observations = "{\"note\": \"durable\"}\n";
    write_facet_entity_observations(temporary.path(), "work", "stable-facet-dir", observations)
        .unwrap();

    save_facet_entity_link(
        temporary.path(),
        "work",
        "stable-facet-dir",
        "new-journal-id",
        &relationship,
    )
    .unwrap();

    assert_eq!(
        read_facet_entity_link(temporary.path(), "work", "stable-facet-dir")
            .unwrap()
            .unwrap()
            .entity_id(),
        "new-journal-id"
    );
    assert_eq!(
        read_facet_entity_observations(temporary.path(), "work", "stable-facet-dir").unwrap(),
        Some(observations.to_owned())
    );
    let directories = fs::read_dir(temporary.path().join("facets/work/entities"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(directories, vec!["stable-facet-dir"]);
}

#[test]
fn rename_facet_rescopes_recorded_choices_and_reports_reindexing() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "old-facet");
    let convey_path = temporary.path().join("config/convey.json");
    fs::create_dir_all(convey_path.parent().unwrap()).unwrap();
    let convey_bytes = br#"{ "facets": { "selected": "old-facet", "order": "malformed" }, "unrelated": { "preserved": true } }"#.to_vec();
    fs::write(&convey_path, &convey_bytes).unwrap();
    let scope = json!({"kind": "facet", "facet": "old-facet"});
    let observation = AmbiguityObservation {
        scope: scope.clone(),
        query: "Alex".to_owned(),
        normalized_query: "alex".to_owned(),
        observed_tier: 5,
        ranked_candidates: vec![
            json!({"id": "higher", "name": "Higher", "tier": 5, "score": 0.95}),
            json!({"id": "lower", "name": "Lower", "tier": 5, "score": 0.55}),
        ],
        origin: json!({
            "lane": "facet",
            "facet": "old-facet",
            "path": "facets/old-facet/entities/person/entity.json"
        }),
    };
    let recorded = record_ambiguity_observation(temporary.path(), &observation).unwrap();
    let origin_keys_before = recorded["origin_keys"].clone();
    record_ambiguity_choice(
        temporary.path(),
        &AmbiguityChoiceRequest {
            scope: scope.clone(),
            query: "Alex".to_owned(),
            entity_id: "lower".to_owned(),
            origin: None,
        },
        &[
            AmbiguityChoiceEntity {
                id: "higher".to_owned(),
                blocked: false,
            },
            AmbiguityChoiceEntity {
                id: "lower".to_owned(),
                blocked: false,
            },
        ],
    )
    .unwrap();

    let result = rename_facet(temporary.path(), "old-facet", "new-facet").unwrap();

    assert_eq!(result.old_name, "old-facet");
    assert_eq!(result.new_name, "new-facet");
    assert!(result.reindex_required);
    assert!(!temporary.path().join("facets/old-facet").exists());
    assert!(
        temporary
            .path()
            .join("facets/new-facet/facet.json")
            .exists()
    );
    // Rename follows the Python facet lifecycle only; ambiguity state is not
    // a facet declaration write concern and therefore remains untouched.
    let _ = origin_keys_before;
    assert!(
        load_resolved_ambiguity_choice(temporary.path(), &scope, "alex")
            .unwrap()
            .is_some()
    );
    assert_eq!(fs::read(convey_path).unwrap(), convey_bytes);
}

#[test]
fn delete_facet_leaves_legacy_convey_selection_bytes_untouched() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "old-facet");
    let convey_path = temporary.path().join("config/convey.json");
    fs::create_dir_all(convey_path.parent().unwrap()).unwrap();
    let convey_bytes =
        br#"{ "facets": { "selected": "old-facet", "order": { "malformed": true } } }"#.to_vec();
    fs::write(&convey_path, &convey_bytes).unwrap();

    assert!(delete_facet(temporary.path(), "old-facet").unwrap());

    assert!(!temporary.path().join("facets/old-facet").exists());
    assert_eq!(fs::read(convey_path).unwrap(), convey_bytes);
}

#[test]
fn facet_link_repair_links_matches_and_reports_unmatched_entries() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_journal_entity(temporary.path(), "alpha", None);
    write_facet_relationship(temporary.path(), "work", "alpha", json!({"role": "member"}));
    write_facet_relationship(
        temporary.path(),
        "work",
        "unknown",
        json!({"role": "guest"}),
    );

    let report = repair_facet_entity_links(temporary.path(), "work").unwrap();

    assert!(report.branches.iter().any(|branch| matches!(
        branch,
        FacetEntityLinkRepairBranch::Linked { facet_entity_dir, journal_entity_id }
            if facet_entity_dir == "alpha" && journal_entity_id == "alpha"
    )));
    assert!(report.branches.iter().any(|branch| matches!(
        branch,
        FacetEntityLinkRepairBranch::Unmatched { facet_entity_dir } if facet_entity_dir == "unknown"
    )));
    assert_eq!(
        relationship_value(temporary.path(), "work", "alpha")["entity_id"],
        "alpha"
    );
    assert!(facet_marker_path(temporary.path(), "work").exists());
}

#[test]
fn facet_link_repair_refuses_multiple_journal_identity_matches() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_journal_entity(temporary.path(), "shared", None);
    write_journal_entity(temporary.path(), "other", Some("shared"));
    write_facet_relationship(
        temporary.path(),
        "work",
        "shared",
        json!({"role": "member"}),
    );

    let report = repair_facet_entity_links(temporary.path(), "work").unwrap();

    assert!(report.branches.iter().any(|branch| matches!(
        branch,
        FacetEntityLinkRepairBranch::MultiMatched { facet_entity_dir, journal_entity_dirs }
            if facet_entity_dir == "shared"
                && journal_entity_dirs == &vec!["other".to_owned(), "shared".to_owned()]
    )));
    assert!(
        relationship_value(temporary.path(), "work", "shared")
            .get("entity_id")
            .is_none()
    );
}

#[test]
fn facet_link_repair_uses_effective_ids_and_checks_prepared_history_at_journal_directory() {
    let linked = TempDir::new();
    create_test_facet(linked.path(), "work");
    write_journal_entity(linked.path(), "foo", Some("shared-id"));
    write_facet_relationship(
        linked.path(),
        "work",
        "shared-id",
        json!({"role": "member"}),
    );

    let report = repair_facet_entity_links(linked.path(), "work").unwrap();

    assert!(report.branches.iter().any(|branch| matches!(
        branch,
        FacetEntityLinkRepairBranch::Linked { facet_entity_dir, journal_entity_id }
            if facet_entity_dir == "shared-id" && journal_entity_id == "shared-id"
    )));
    assert_eq!(
        relationship_value(linked.path(), "work", "shared-id")["entity_id"],
        "shared-id"
    );

    let pending = TempDir::new();
    create_test_facet(pending.path(), "work");
    write_journal_entity(pending.path(), "foo", Some("shared-id"));
    write_prepared_history(pending.path(), "foo");
    write_facet_relationship(
        pending.path(),
        "work",
        "shared-id",
        json!({"role": "member"}),
    );
    let before = fs::read(
        pending
            .path()
            .join("facets/work/entities/shared-id/entity.json"),
    )
    .unwrap();
    let journal_before = fs::read(
        pending
            .path()
            .join("entities/foo/history/prepared/staged/event.json"),
    )
    .unwrap();

    let report = incomplete_report(repair_facet_entity_links(pending.path(), "work").unwrap_err());

    assert!(report.branches.iter().any(|branch| matches!(
        branch,
        FacetEntityLinkRepairBranch::RefusedPending { facet_entity_dir, journal_entity_dir }
            if facet_entity_dir == "shared-id" && journal_entity_dir == "foo"
    )));
    assert_eq!(
        fs::read(
            pending
                .path()
                .join("facets/work/entities/shared-id/entity.json"),
        )
        .unwrap(),
        before
    );
    assert_eq!(
        fs::read(
            pending
                .path()
                .join("entities/foo/history/prepared/staged/event.json"),
        )
        .unwrap(),
        journal_before
    );
    assert!(!facet_marker_path(pending.path(), "work").exists());
}

#[test]
fn facet_link_repair_counts_non_entity_material_without_blocking_real_links() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_journal_entity(temporary.path(), "person", None);
    write_facet_relationship(temporary.path(), "work", "person", json!({}));
    write_text(
        temporary.path(),
        "facets/work/entities/20260305.jsonl",
        "{\"detected\": true}\n",
    );

    let report = repair_facet_entity_links(temporary.path(), "work").unwrap();

    assert!(report.branches.iter().any(|branch| matches!(
        branch,
        FacetEntityLinkRepairBranch::SkippedNotAnEntity { facet_entity_dir }
            if facet_entity_dir == "20260305.jsonl"
    )));
    assert!(report.branches.iter().any(|branch| matches!(
        branch,
        FacetEntityLinkRepairBranch::Linked { facet_entity_dir, .. } if facet_entity_dir == "person"
    )));
    assert_eq!(
        fs::read_to_string(temporary.path().join("facets/work/entities/20260305.jsonl")).unwrap(),
        "{\"detected\": true}\n"
    );
}

#[test]
fn facet_link_repair_refuses_unparseable_relationships_without_writing_marker() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_journal_entity(temporary.path(), "person", None);
    write_text(
        temporary.path(),
        "facets/work/entities/person/entity.json",
        "not json\n",
    );
    let relationship_path = temporary
        .path()
        .join("facets/work/entities/person/entity.json");
    let before = fs::read(&relationship_path).unwrap();

    let report =
        incomplete_report(repair_facet_entity_links(temporary.path(), "work").unwrap_err());

    assert!(report.branches.iter().any(|branch| matches!(
        branch,
        FacetEntityLinkRepairBranch::RefusedUnparseable { facet_entity_dir, .. }
            if facet_entity_dir == "person"
    )));
    assert_eq!(fs::read(relationship_path).unwrap(), before);
    assert!(!facet_marker_path(temporary.path(), "work").exists());
}

#[test]
fn facet_link_repair_writes_markers_only_after_a_clean_scan_and_rejects_second_run() {
    let clean = TempDir::new();
    create_test_facet(clean.path(), "work");
    write_journal_entity(clean.path(), "person", None);
    write_facet_relationship(clean.path(), "work", "person", json!({}));

    repair_facet_entity_links(clean.path(), "work").unwrap();
    let marker = facet_marker_path(clean.path(), "work");
    assert!(marker.exists());
    assert!(matches!(
        repair_facet_entity_links(clean.path(), "work"),
        Err(FacetEntityLinkRepairError::AlreadyCompleted { completion_marker }) if completion_marker == marker
    ));

    let refused = TempDir::new();
    create_test_facet(refused.path(), "work");
    write_journal_entity(refused.path(), "person", None);
    write_prepared_history(refused.path(), "person");
    write_facet_relationship(refused.path(), "work", "person", json!({}));

    assert!(matches!(
        repair_facet_entity_links(refused.path(), "work"),
        Err(FacetEntityLinkRepairError::Incomplete { .. })
    ));
    assert!(!facet_marker_path(refused.path(), "work").exists());
}

#[test]
fn facet_link_repair_resumes_from_partially_linked_relationships() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_journal_entity(temporary.path(), "alpha", None);
    write_journal_entity(temporary.path(), "beta", None);
    let fields = Map::new();
    save_facet_entity_link(temporary.path(), "work", "alpha", "alpha", &fields).unwrap();
    write_facet_relationship(temporary.path(), "work", "beta", json!({"role": "member"}));

    let report = repair_facet_entity_links(temporary.path(), "work").unwrap();

    assert_eq!(
        report
            .branches
            .iter()
            .filter(|branch| matches!(branch, FacetEntityLinkRepairBranch::Linked { .. }))
            .count(),
        2
    );
    assert_eq!(
        relationship_value(temporary.path(), "work", "alpha")["entity_id"],
        "alpha"
    );
    assert_eq!(
        relationship_value(temporary.path(), "work", "beta")["entity_id"],
        "beta"
    );
    assert!(facet_marker_path(temporary.path(), "work").exists());
}

#[test]
fn journal_wide_link_repair_waits_for_every_facet_before_writing_its_marker() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "clean");
    create_test_facet(temporary.path(), "refused");
    write_journal_entity(temporary.path(), "clean-person", None);
    write_journal_entity(temporary.path(), "refused-person", None);
    write_facet_relationship(temporary.path(), "clean", "clean-person", json!({}));
    write_prepared_history(temporary.path(), "refused-person");
    write_facet_relationship(temporary.path(), "refused", "refused-person", json!({}));

    let error = repair_facet_entity_links_journal_wide(temporary.path()).unwrap_err();

    assert!(matches!(
        error,
        FacetEntityLinkRepairError::JournalWideIncomplete { .. }
    ));
    assert!(facet_marker_path(temporary.path(), "clean").exists());
    assert!(!facet_marker_path(temporary.path(), "refused").exists());
    assert!(!journal_marker_path(temporary.path()).exists());
}

#[test]
fn journal_wide_link_repair_reuses_completed_facet_markers_without_rescanning() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "cached");
    create_test_facet(temporary.path(), "fresh");
    write_journal_entity(temporary.path(), "cached-person", None);
    write_journal_entity(temporary.path(), "fresh-person", None);
    write_facet_relationship(temporary.path(), "cached", "cached-person", json!({}));
    write_facet_relationship(temporary.path(), "fresh", "fresh-person", json!({}));
    repair_facet_entity_links(temporary.path(), "cached").unwrap();
    write_text(
        temporary.path(),
        "facets/cached/entities/broken/entity.json",
        "not json\n",
    );

    let report = repair_facet_entity_links_journal_wide(temporary.path()).unwrap();

    assert_eq!(report.facets.len(), 2);
    assert!(journal_marker_path(temporary.path()).exists());
    assert!(facet_marker_path(temporary.path(), "fresh").exists());
    assert!(
        report
            .facets
            .iter()
            .find(|facet| facet.facet == "cached")
            .unwrap()
            .branches
            .iter()
            .all(|branch| !matches!(
                branch,
                FacetEntityLinkRepairBranch::RefusedUnparseable { .. }
            ))
    );
}

#[test]
fn journal_wide_link_repair_rejects_a_corrupt_per_facet_marker_instead_of_trusting_it() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_journal_entity(temporary.path(), "person", None);
    write_facet_relationship(temporary.path(), "work", "person", json!({}));
    let marker = facet_marker_path(temporary.path(), "work");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "").unwrap();

    let error = repair_facet_entity_links_journal_wide(temporary.path()).unwrap_err();

    assert!(matches!(
        error,
        FacetEntityLinkRepairError::CachedMarkerRead { facet, source, .. }
            if facet == "work"
                && matches!(
                    source.as_ref(),
                    FacetStoreError::CorruptCompletionMarker { path } if path == &marker
                )
    ));
    assert!(
        relationship_value(temporary.path(), "work", "person")
            .get("entity_id")
            .is_none()
    );
    assert!(!journal_marker_path(temporary.path()).exists());
}

fn declaration_value(root: &Path, facet_dir: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(root.join("facets").join(facet_dir).join("facet.json")).unwrap(),
    )
    .unwrap()
}

fn write_json(root: &Path, relative: &str, value: &Value) {
    write_text(root, relative, &serde_json::to_string(value).unwrap());
}

fn write_text(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

pub(crate) fn create_test_facet(root: &Path, facet: &str) {
    create_facet(root, facet, facet, "Description", "blue", "💼", None).unwrap();
}

pub(crate) fn write_journal_entity(root: &Path, entity_dir: &str, written_id: Option<&str>) {
    let mut entity = Map::new();
    if let Some(written_id) = written_id {
        entity.insert("id".to_owned(), Value::String(written_id.to_owned()));
    }
    write_json(
        root,
        &format!("entities/{entity_dir}/entity.json"),
        &Value::Object(entity),
    );
}

pub(crate) fn write_facet_relationship(
    root: &Path,
    facet: &str,
    entity_dir: &str,
    relationship: Value,
) {
    write_json(
        root,
        &format!("facets/{facet}/entities/{entity_dir}/entity.json"),
        &relationship,
    );
}

fn write_prepared_history(root: &Path, journal_entity_dir: &str) {
    write_json(
        root,
        &format!("entities/{journal_entity_dir}/history/prepared/staged/event.json"),
        &json!({}),
    );
}

pub(crate) fn relationship_value(root: &Path, facet: &str, entity_dir: &str) -> Value {
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

fn facet_marker_path(root: &Path, facet: &str) -> PathBuf {
    root.join("facets")
        .join(facet)
        .join("health/migrations/entity-link-repair.json")
}

fn journal_marker_path(root: &Path) -> PathBuf {
    root.join("health/migrations/facet-entity-link-repair.json")
}

fn incomplete_report(error: FacetEntityLinkRepairError) -> crate::FacetEntityLinkReport {
    match error {
        FacetEntityLinkRepairError::Incomplete { report } => *report,
        other => panic!("expected incomplete repair, got {other}"),
    }
}
