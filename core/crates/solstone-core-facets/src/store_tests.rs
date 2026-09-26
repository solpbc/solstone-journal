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
    FacetEntityLinkRepairBranch, FacetEntityLinkRepairError, FacetStoreError, FacetWriteError,
    create_facet, delete_facet, list_facet_entity_directories, read_activity_file,
    read_facet_declaration, read_facet_entity_link, read_facet_entity_observations, read_log_file,
    read_news_file, repair_facet_entity_links, repair_facet_entity_links_journal_wide,
    retitle_facet, save_facet_entity_link, set_facet_muted, update_facet, write_activity_file,
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
    create_facet(temporary.path(), "personal", "Personal", "", "", "", None).unwrap();
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
fn news_and_log_writers_refuse_an_undeclared_destination_and_create_nothing() {
    let temporary = TempDir::new();
    let root = temporary.path();
    assert!(matches!(
        write_news_file(root, "undeclared", "20260101.md", "# Late\n"),
        Err(FacetWriteError::DeclarationMissing { .. })
    ));
    assert!(matches!(
        write_log_file(root, "undeclared", "20260101.jsonl", "{}\n"),
        Err(FacetWriteError::DeclarationMissing { .. })
    ));
    assert!(!root.join("facets/undeclared").exists());

    // A directory is not a declaration.
    fs::create_dir_all(root.join("facets/bare")).unwrap();
    assert!(matches!(
        write_news_file(root, "bare", "20260101.md", "# Late\n"),
        Err(FacetWriteError::DeclarationMissing { .. })
    ));
    assert!(matches!(
        write_log_file(root, "bare", "20260101.jsonl", "{}\n"),
        Err(FacetWriteError::DeclarationMissing { .. })
    ));
    assert!(!root.join("facets/bare/news").exists());
    assert!(!root.join("facets/bare/logs").exists());
}

#[test]
fn news_and_log_writers_leave_a_damaged_declaration_exactly_as_found() {
    let cases: [(&str, &[u8]); 3] = [
        ("malformed json", b"{not json"),
        ("scalar", b"42"),
        ("malformed id", br#"{"title":"Work","id":"nope"}"#),
    ];
    for (label, bytes) in cases {
        let temporary = TempDir::new();
        let root = temporary.path();
        let declaration = root.join("facets/work/facet.json");
        fs::create_dir_all(declaration.parent().unwrap()).unwrap();
        fs::write(&declaration, bytes).unwrap();
        for result in [
            write_news_file(root, "work", "20260101.md", "# Late\n"),
            write_log_file(root, "work", "20260101.jsonl", "{}\n"),
        ] {
            assert!(
                matches!(result, Err(FacetWriteError::DeclarationDamaged { .. })),
                "{label}: {result:?}"
            );
        }
        assert_eq!(fs::read(&declaration).unwrap(), bytes, "{label}");
        let siblings: Vec<String> = fs::read_dir(root.join("facets/work"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            siblings,
            vec!["facet.json".to_owned()],
            "{label}: no set-aside, no content"
        );
    }

    // A declaration that cannot be read as a file is refused and left in place too.
    let temporary = TempDir::new();
    let root = temporary.path();
    fs::create_dir_all(root.join("facets/work/facet.json")).unwrap();
    assert!(write_news_file(root, "work", "20260101.md", "# Late\n").is_err());
    assert!(root.join("facets/work/facet.json").is_dir());
    let siblings: Vec<String> = fs::read_dir(root.join("facets/work"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(siblings, vec!["facet.json".to_owned()]);
}

#[test]
fn news_and_log_writers_admit_declared_muted_and_legacy_id_less_facets() {
    let temporary = TempDir::new();
    let root = temporary.path();
    create_facet(root, "work", "Work", "", "", "", None).unwrap();
    create_facet(root, "personal", "Personal", "", "", "", None).unwrap();
    write_news_file(root, "work", "20260101.md", "# Declared\n").unwrap();
    write_log_file(root, "work", "20260101.jsonl", "{}\n").unwrap();

    set_facet_muted(root, "work", true).unwrap();
    write_news_file(root, "work", "20260102.md", "# Muted\n").unwrap();

    // The state `journal facet doctor --fix` and pre-identity journals leave behind.
    fs::create_dir_all(root.join("facets/legacy")).unwrap();
    fs::write(
        root.join("facets/legacy/facet.json"),
        r#"{"title":"Field Notes"}"#,
    )
    .unwrap();
    write_news_file(root, "legacy", "20260101.md", "# Legacy\n").unwrap();
    write_log_file(root, "legacy", "20260101.jsonl", "{}\n").unwrap();
    assert_eq!(
        read_news_file(root, "legacy", "20260101.md").unwrap(),
        Some("# Legacy\n".to_owned())
    );
    assert_eq!(
        fs::read_to_string(root.join("facets/legacy/facet.json")).unwrap(),
        r#"{"title":"Field Notes"}"#,
        "admission does not mint an id"
    );
}

#[test]
fn facet_content_files_round_trip_without_parsing() {
    let temporary = TempDir::new();
    create_facet(temporary.path(), "work", "Work", "", "", "", None).unwrap();
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
fn retitle_facet_keeps_its_name_id_references_and_choices() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "old-facet");
    create_test_facet(temporary.path(), "personal");
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

    let before = read_facet_declaration(temporary.path(), "old-facet")
        .unwrap()
        .unwrap()
        .value()
        .clone();

    retitle_facet(temporary.path(), "old-facet", "New Facet").unwrap();

    // The name, directory and id stay; only the title the owner sees changes.
    let after = read_facet_declaration(temporary.path(), "old-facet")
        .unwrap()
        .unwrap()
        .value()
        .clone();
    assert_eq!(
        after.get("title").and_then(Value::as_str),
        Some("New Facet")
    );
    assert_eq!(after.get("id"), before.get("id"));
    assert!(!temporary.path().join("facets/new-facet").exists());
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
    create_test_facet(temporary.path(), "personal");
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

#[test]
fn facet_creation_allocates_uuid_and_refuses_duplicate() {
    let temporary = TempDir::new();
    create_facet(
        temporary.path(),
        "work",
        "Work",
        "Work context",
        "#667eea",
        "💼",
        None,
    )
    .unwrap();

    let declaration = read_facet_declaration(temporary.path(), "work")
        .unwrap()
        .unwrap();
    let id = declaration
        .value()
        .get("id")
        .and_then(Value::as_str)
        .unwrap();
    assert!(crate::is_well_formed_facet_id(id));

    // Refuses duplicate creation
    let err = create_facet(
        temporary.path(),
        "work",
        "Work 2",
        "Different",
        "#ff0000",
        "🔥",
        None,
    )
    .unwrap_err();
    assert!(matches!(err, crate::FacetWriteError::AlreadyExists { .. }));
}

#[test]
fn facet_rename_and_update_preserves_id() {
    let temporary = TempDir::new();
    create_facet(
        temporary.path(),
        "work",
        "Work",
        "Work context",
        "#667eea",
        "💼",
        None,
    )
    .unwrap();

    let initial = read_facet_declaration(temporary.path(), "work")
        .unwrap()
        .unwrap();
    let id = initial
        .value()
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();

    update_facet(
        temporary.path(),
        "work",
        "Work Updated",
        "New description",
        "#112233",
        "📁",
        None,
    )
    .unwrap();

    let updated = read_facet_declaration(temporary.path(), "work")
        .unwrap()
        .unwrap();
    assert_eq!(
        updated.value().get("id").and_then(Value::as_str),
        Some(id.as_str())
    );

    retitle_facet(temporary.path(), "work", "Job").unwrap();

    let renamed = read_facet_declaration(temporary.path(), "work")
        .unwrap()
        .unwrap();
    assert_eq!(
        renamed.value().get("id").and_then(Value::as_str),
        Some(id.as_str())
    );
    assert_eq!(
        renamed.value().get("title").and_then(Value::as_str),
        Some("Job")
    );
    assert_eq!(
        crate::resolve_facet_id(temporary.path(), &id).unwrap(),
        "work"
    );
}

fn facet_id_of(root: &Path, name: &str) -> String {
    read_facet_declaration(root, name)
        .unwrap()
        .unwrap()
        .value()
        .get("id")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned()
}

fn plain_facet(root: &Path, name: &str) {
    create_facet(root, name, name, "", "", "", None).unwrap();
}

#[test]
fn deleting_a_facet_retires_its_name_and_the_name_is_never_given_out_again() {
    let temporary = TempDir::new();
    plain_facet(temporary.path(), "personal");
    plain_facet(temporary.path(), "work");
    let id = facet_id_of(temporary.path(), "work");
    assert!(crate::delete_facet(temporary.path(), "work").unwrap());

    let entry = crate::retired_facet_entry(temporary.path(), "work")
        .unwrap()
        .unwrap();
    assert_eq!(entry.state, crate::RetiredFacetState::Deleted);
    assert_eq!(entry.id.as_deref(), Some(id.as_str()));
    assert!(matches!(
        create_facet(temporary.path(), "work", "Work", "", "", "", None),
        Err(crate::FacetWriteError::NameRetired { name }) if name == "work"
    ));
    assert_eq!(
        crate::first_free_facet_name(temporary.path(), "work").unwrap(),
        "work-2"
    );
    // Any existing folder is skipped as well, declared or not.
    fs::create_dir_all(temporary.path().join("facets/work-2")).unwrap();
    assert_eq!(
        crate::first_free_facet_name(temporary.path(), "work").unwrap(),
        "work-3"
    );
}

#[test]
fn a_delete_interrupted_after_retiring_the_name_can_be_retried() {
    let temporary = TempDir::new();
    plain_facet(temporary.path(), "personal");
    plain_facet(temporary.path(), "work");
    let id = facet_id_of(temporary.path(), "work");
    // The state a crash between the two steps leaves: the facet is still
    // there and carries its own leftover entry, written with other details.
    let mut leftover = crate::RetiredFacet::deleted(Some(id.clone()), Some("Old".to_owned()));
    leftover.at = Some("2026-01-01T00:00:00Z".to_owned());
    crate::record_retired_facet(temporary.path(), "work", leftover).unwrap();

    assert!(crate::delete_facet(temporary.path(), "work").unwrap());
    let entry = crate::retired_facet_entry(temporary.path(), "work")
        .unwrap()
        .unwrap();
    assert_eq!(entry.id.as_deref(), Some(id.as_str()));
    assert!(!temporary.path().join("facets/work").exists());
}

#[test]
fn a_committed_retired_entry_is_never_replaced_by_another_facet() {
    let temporary = TempDir::new();
    plain_facet(temporary.path(), "personal");
    plain_facet(temporary.path(), "work");
    let work = facet_id_of(temporary.path(), "work");
    crate::delete_facet(temporary.path(), "work").unwrap();
    // A second live folder that happens to carry the deleted facet's id.
    fs::create_dir_all(temporary.path().join("facets/x")).unwrap();
    fs::write(
        temporary.path().join("facets/x/facet.json"),
        format!(r#"{{"id":"{work}","title":"X"}}"#),
    )
    .unwrap();
    let personal = facet_id_of(temporary.path(), "personal");
    assert!(matches!(
        crate::record_retired_facet(
            temporary.path(),
            "work",
            crate::RetiredFacet::merged(Some(work.clone()), personal, None),
        ),
        Err(crate::FacetWriteError::RetiredEntryConflict { .. })
    ));
    // Writing the same outcome again is a no-op.
    crate::record_retired_facet(
        temporary.path(),
        "work",
        crate::RetiredFacet::deleted(Some(work), None),
    )
    .unwrap();
}

#[test]
fn a_damaged_retired_record_is_never_overwritten_and_blocks_new_names() {
    let temporary = TempDir::new();
    plain_facet(temporary.path(), "personal");
    plain_facet(temporary.path(), "work");
    let path = temporary.path().join("facets/retired.json");
    fs::write(&path, b"{not json").unwrap();

    assert!(matches!(
        create_facet(temporary.path(), "fresh", "Fresh", "", "", "", None),
        Err(crate::FacetWriteError::RetiredFileDamaged { .. })
    ));
    assert!(matches!(
        crate::delete_facet(temporary.path(), "work"),
        Err(crate::FacetWriteError::RetiredFileDamaged { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), b"{not json");
    assert!(temporary.path().join("facets/work").exists());
}

#[test]
fn the_default_facet_skips_a_retired_personal() {
    let temporary = TempDir::new();
    plain_facet(temporary.path(), "personal");
    plain_facet(temporary.path(), "work");
    crate::delete_facet(temporary.path(), "personal").unwrap();
    crate::set_facet_muted(temporary.path(), "work", true).unwrap_err();
    // Mute by hand to reach "no enabled facet", as a hand edit or older build could.
    fs::write(
        temporary.path().join("facets/work/facet.json"),
        format!(
            r#"{{"id":"{}","title":"work","muted":true}}"#,
            facet_id_of(temporary.path(), "work")
        ),
    )
    .unwrap();

    assert!(crate::ensure_default_facet(temporary.path()).unwrap());
    assert!(!temporary.path().join("facets/personal").exists());
    let default = read_facet_declaration(temporary.path(), "personal-2")
        .unwrap()
        .unwrap();
    assert_eq!(
        default.value().get("title").and_then(Value::as_str),
        Some("Personal")
    );
}

#[test]
fn listing_facets_ignores_the_retired_record_and_dot_folders() {
    let temporary = TempDir::new();
    plain_facet(temporary.path(), "personal");
    plain_facet(temporary.path(), "work");
    let listed = |root: &Path| {
        (
            crate::list_declared_facet_names(root).unwrap(),
            crate::list_facet_directories(root).unwrap(),
            crate::observe_declared_facet_inventory(root)
                .unwrap()
                .enabled,
        )
    };
    let before = listed(temporary.path());
    crate::delete_facet(temporary.path(), "work").unwrap();
    plain_facet(temporary.path(), "work-2");
    let expected = listed(temporary.path());
    // A merge scratch copy holding a live facet's declaration changes nothing.
    fs::create_dir_all(temporary.path().join("facets/.facet-merge-x.dest")).unwrap();
    fs::copy(
        temporary.path().join("facets/personal/facet.json"),
        temporary
            .path()
            .join("facets/.facet-merge-x.dest/facet.json"),
    )
    .unwrap();
    assert!(temporary.path().join("facets/retired.json").exists());
    assert_eq!(listed(temporary.path()), expected);
    assert_ne!(before, expected);
}

#[test]
fn facet_id_resolution_and_backfill() {
    let temporary = TempDir::new();

    // Create legacy facet manually without an id
    let legacy_path = temporary.path().join("facets/legacy");
    fs::create_dir_all(&legacy_path).unwrap();
    fs::write(
        legacy_path.join("facet.json"),
        json!({
            "title": "Legacy",
            "description": "No ID",
            "color": "#667eea",
            "emoji": "📦"
        })
        .to_string(),
    )
    .unwrap();

    // Create a modern facet with id
    create_facet(
        temporary.path(),
        "modern",
        "Modern",
        "Has ID",
        "#667eea",
        "📦",
        None,
    )
    .unwrap();

    // Dry-run backfill
    let dry_run = crate::backfill_facet_ids(temporary.path(), false).unwrap();
    assert_eq!(dry_run.total_scanned, 2);
    assert_eq!(dry_run.backfilled_count, 1);
    assert_eq!(dry_run.unchanged_count, 1);
    assert!(!dry_run.committed);

    // Legacy still has no ID after dry run
    let legacy_decl = read_facet_declaration(temporary.path(), "legacy")
        .unwrap()
        .unwrap();
    assert!(legacy_decl.value().get("id").is_none());

    // Commit backfill
    let committed = crate::backfill_facet_ids(temporary.path(), true).unwrap();
    assert_eq!(committed.total_scanned, 2);
    assert_eq!(committed.backfilled_count, 1);
    assert_eq!(committed.unchanged_count, 1);
    assert!(committed.committed);

    // Legacy now has a well-formed ID
    let legacy_after = read_facet_declaration(temporary.path(), "legacy")
        .unwrap()
        .unwrap();
    let legacy_id = legacy_after
        .value()
        .get("id")
        .and_then(Value::as_str)
        .unwrap();
    assert!(crate::is_well_formed_facet_id(legacy_id));

    // Resolution works
    assert_eq!(
        crate::resolve_facet_id(temporary.path(), legacy_id).unwrap(),
        "legacy"
    );

    // Resolution errors
    assert_eq!(
        crate::resolve_facet_id(temporary.path(), "nonexistent-0000-4000-8000-000000000000"),
        Err(crate::FacetIdResolveError::Malformed)
    );
    assert_eq!(
        crate::resolve_facet_id(temporary.path(), "00000000-0000-4000-8000-000000000000"),
        Err(crate::FacetIdResolveError::Missing)
    );
}
