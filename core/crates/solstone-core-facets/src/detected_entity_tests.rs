// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;

use chrono::{FixedOffset, TimeZone};
use serde_json::{Value, json};

use crate::store::{cutoff_day, exclusion_tier};
use crate::store_tests::{
    TempDir, create_test_facet, write_facet_relationship, write_journal_entity,
};
use crate::{
    DetectedEntityInput, FacetEntityWriteError, delete_detected_entity,
    iter_detected_entity_names_since, iter_detected_entity_names_since_strict,
    load_detected_entities_recent, read_detected_entities, read_detected_entity_names_strict,
    save_detected_entity, update_detected_entity, upsert_detection_segment,
};

fn write_detected(root: &std::path::Path, facet: &str, day: &str, rows: &[Value]) {
    let path = root.join(format!("facets/{facet}/entities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        rows.iter()
            .map(|row| serde_json::to_string(row).unwrap() + "\n")
            .collect::<String>(),
    )
    .unwrap();
}

fn input(entity_type: &str, name: &str, description: &str) -> DetectedEntityInput {
    DetectedEntityInput {
        entity_type: entity_type.to_owned(),
        name: name.to_owned(),
        description: description.to_owned(),
    }
}

fn attach_entity(
    root: &std::path::Path,
    facet: &str,
    entity_id: &str,
    name: &str,
    blocked: bool,
    detached: bool,
    relationship_extra: Value,
) {
    write_journal_entity(root, entity_id, Some(entity_id));
    fs::write(
        root.join(format!("entities/{entity_id}/entity.json")),
        serde_json::to_vec(&json!({
            "id": entity_id,
            "name": name,
            "type": "Person",
            "blocked": blocked,
        }))
        .unwrap(),
    )
    .unwrap();
    let mut relationship = relationship_extra.as_object().cloned().unwrap_or_default();
    relationship.insert("entity_id".to_owned(), Value::String(entity_id.to_owned()));
    if detached {
        relationship.insert("detached".to_owned(), Value::Bool(true));
    }
    write_facet_relationship(root, facet, entity_id, Value::Object(relationship));
}

#[test]
fn detected_save_is_fold_insensitive_but_update_and_delete_are_exact() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    save_detected_entity(
        temporary.path(),
        "scope",
        "20260101",
        "kind",
        "Straße",
        "one",
    )
    .unwrap();
    assert!(matches!(
        save_detected_entity(
            temporary.path(),
            "scope",
            "20260101",
            "kind",
            "STRASSE",
            "two"
        ),
        Err(FacetEntityWriteError::EntityExists { .. })
    ));
    assert!(matches!(
        update_detected_entity(temporary.path(), "scope", "20260101", "STRASSE", "two"),
        Err(FacetEntityWriteError::EntityNotFound { .. })
    ));
    update_detected_entity(temporary.path(), "scope", "20260101", "Straße", "two").unwrap();
    assert!(
        delete_detected_entity(temporary.path(), "scope", "20260101", "STRASSE")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        delete_detected_entity(temporary.path(), "scope", "20260101", "Straße")
            .unwrap()
            .len(),
        1
    );
    assert!(
        read_detected_entities(temporary.path(), "scope", "20260101")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn detected_reader_treats_missing_and_directory_paths_as_empty() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    assert!(
        read_detected_entities(temporary.path(), "scope", "20260101")
            .unwrap()
            .is_empty()
    );
    fs::create_dir_all(
        temporary
            .path()
            .join("facets/scope/entities/20260102.jsonl"),
    )
    .unwrap();
    assert!(
        read_detected_entities(temporary.path(), "scope", "20260102")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn detected_reader_skips_malformed_rows_and_fills_missing_ids() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    let path = temporary
        .path()
        .join("facets/scope/entities/20260101.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "not json\n{\"type\":\"Person\",\"name\":\"Alice\",\"description\":\"ok\"}\n",
    )
    .unwrap();
    let rows = read_detected_entities(temporary.path(), "scope", "20260101").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "alice");
}

#[test]
fn strict_detected_reader_preserves_valid_row_order_and_rejects_bad_rows() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    let path = temporary
        .path()
        .join("facets/scope/entities/20260101.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "\n{\"type\":\"Person\",\"name\":\"Ada\"}\n{\"type\":\"Tool\",\"name\":\"Beacon\"}\n",
    )
    .unwrap();
    assert_eq!(
        read_detected_entity_names_strict(temporary.path(), "scope", "20260101").unwrap(),
        vec!["Ada", "Beacon"]
    );

    for contents in [
        "not json\n",
        "[]\n",
        "{\"type\":\"x\",\"name\":\"Ada\"}\n",
        "{\"type\":\"Person\"}\n",
        "{\"type\":\"Person\",\"name\":\"  \"}\n",
    ] {
        fs::write(&path, contents).unwrap();
        assert!(
            read_detected_entity_names_strict(temporary.path(), "scope", "20260101").is_err(),
            "{contents:?} must fail strictly"
        );
    }
}

#[test]
fn invalid_detected_rows_are_dropped_after_a_write_round_trip() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[
            json!({"id":"valid","type":"Person","name":"Valid","description":"before"}),
            json!({"id":"invalid","type":"x","name":"Invalid","description":"bad"}),
        ],
    );
    let rows = read_detected_entities(temporary.path(), "scope", "20260101").unwrap();
    assert_eq!(rows.len(), 1);
    update_detected_entity(temporary.path(), "scope", "20260101", "Valid", "after").unwrap();
    let raw = fs::read_to_string(
        temporary
            .path()
            .join("facets/scope/entities/20260101.jsonl"),
    )
    .unwrap();
    assert!(raw.contains("Valid"));
    assert!(!raw.contains("Invalid"));
}

#[test]
fn recent_detections_keep_the_newest_description_and_count_every_day() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"type":"Person","name":"Alice","description":"old"})],
    );
    write_detected(
        temporary.path(),
        "scope",
        "20260102",
        &[json!({"type":"Person","name":"Alice","description":"new"})],
    );
    let rows = load_detected_entities_recent(temporary.path(), "scope", 36500).unwrap();
    assert_eq!(
        rows,
        vec![
            json!({"type":"Person","name":"Alice","description":"new","count":2,"last_seen":"20260102"})
        ]
    );
}

#[test]
fn recent_detections_exclude_matched_attached_twins() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    attach_entity(
        temporary.path(),
        "scope",
        "alice",
        "Alice",
        false,
        false,
        json!({}),
    );
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"type":"Person","name":"Alice","description":"seen"})],
    );
    assert!(
        load_detected_entities_recent(temporary.path(), "scope", 36500)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn recent_detections_exclude_ambiguous_attached_name_and_keep_its_tier() {
    // Polarity guard — green before and after the wave; reddens if this caller is
    // moved to the public find_matching_entity wrapper instead of the detailed entry point.
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    attach_entity(
        temporary.path(),
        "scope",
        "sam-one",
        "Sam Person",
        false,
        false,
        json!({}),
    );
    attach_entity(
        temporary.path(),
        "scope",
        "sam-two",
        "Sam Person",
        false,
        false,
        json!({}),
    );
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"type":"Person","name":"Sam Person","description":"seen"})],
    );
    assert!(
        load_detected_entities_recent(temporary.path(), "scope", 36500)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        exclusion_tier(temporary.path(), "scope", "Sam Person").unwrap(),
        Some(solstone_core_entity_matching::MatchTier::Exact)
    );
}

#[test]
fn recent_detections_keep_unmatched_names() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    attach_entity(
        temporary.path(),
        "scope",
        "alice",
        "Alice",
        false,
        false,
        json!({}),
    );
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"type":"Person","name":"Bob","description":"seen"})],
    );
    assert_eq!(
        load_detected_entities_recent(temporary.path(), "scope", 36500)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn detached_attached_entities_do_not_exclude_detected_twins() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    attach_entity(
        temporary.path(),
        "scope",
        "alice",
        "Alice",
        false,
        true,
        json!({}),
    );
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"type":"Person","name":"Alice","description":"seen"})],
    );
    assert_eq!(
        load_detected_entities_recent(temporary.path(), "scope", 36500)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn blocked_attached_entities_do_not_exclude_detected_twins() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    attach_entity(
        temporary.path(),
        "scope",
        "alice",
        "Alice",
        true,
        false,
        json!({}),
    );
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"type":"Person","name":"Alice","description":"seen"})],
    );
    assert_eq!(
        load_detected_entities_recent(temporary.path(), "scope", 36500)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn exclusion_candidates_merge_identity_name_with_relationship_email() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    attach_entity(
        temporary.path(),
        "scope",
        "alice",
        "Authoritative Alice",
        false,
        false,
        json!({"name":"stale relationship name","emails":["alice@example.test"]}),
    );
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"type":"Person","name":"Authoritative Alice","description":"seen"})],
    );
    assert!(
        load_detected_entities_recent(temporary.path(), "scope", 36500)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn exclusion_candidates_preserve_relationship_emails() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    attach_entity(
        temporary.path(),
        "scope",
        "alice",
        "Authoritative Alice",
        false,
        false,
        json!({"emails":["alice@example.test"]}),
    );
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"type":"Person","name":"alice@example.test","description":"seen"})],
    );
    assert!(
        load_detected_entities_recent(temporary.path(), "scope", 36500)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cutoff_uses_the_supplied_local_date_not_its_utc_calendar_day() {
    let offset = FixedOffset::west_opt(8 * 60 * 60).unwrap();
    let local = offset
        .with_ymd_and_hms(2026, 1, 1, 23, 30, 0)
        .single()
        .unwrap();
    assert_eq!(
        local.with_timezone(&chrono::Utc).date_naive().to_string(),
        "2026-01-02"
    );
    assert_eq!(cutoff_day(local.date_naive(), 1), "20251231");
}

#[test]
fn upsert_updates_rows_in_place_and_sorts_deduplicated_segments() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[
            json!({"id":"alice","type":"Person","name":"Alice","description":"old","segments":["z","a","z"]}),
        ],
    );
    let report = upsert_detection_segment(
        temporary.path(),
        "scope",
        "20260101",
        "m",
        &[input("Person", "Alice", "new")],
    )
    .unwrap();
    assert_eq!(report.wrote, 1);
    let rows = read_detected_entities(temporary.path(), "scope", "20260101").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["description"], "new");
    assert_eq!(rows[0]["segments"], json!(["a", "m", "z"]));
}

#[test]
fn upsert_normalizes_legacy_object_segment_rows() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[
            json!({"id":"alice","type":"Person","name":"Alice","description":"old","segments":[{"segment":"z"},{"segment":"a"}]}),
        ],
    );
    upsert_detection_segment(
        temporary.path(),
        "scope",
        "20260101",
        "m",
        &[input("Person", "Alice", "new")],
    )
    .unwrap();
    assert_eq!(
        read_detected_entities(temporary.path(), "scope", "20260101").unwrap()[0]["segments"],
        json!(["a", "m", "z"])
    );
}

#[test]
fn upsert_appends_each_distinct_new_detection_once() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    upsert_detection_segment(
        temporary.path(),
        "scope",
        "20260101",
        "segment",
        &[
            input("Person", "Alice", "a"),
            input("Company", "Acme", "b"),
            input("Project", "Beacon", "c"),
        ],
    )
    .unwrap();
    assert_eq!(
        read_detected_entities(temporary.path(), "scope", "20260101")
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn upsert_uses_existing_id_before_deriving_a_slug_from_the_old_name() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"id":"renamed","type":"Person","name":"Legacy Name","description":"old"})],
    );
    upsert_detection_segment(
        temporary.path(),
        "scope",
        "20260101",
        "segment",
        &[input("Person", "Renamed", "new")],
    )
    .unwrap();
    let rows = read_detected_entities(temporary.path(), "scope", "20260101").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Renamed");
}

#[test]
fn names_since_keeps_one_tuple_per_day() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[json!({"type":"Person","name":"Alice"})],
    );
    write_detected(
        temporary.path(),
        "scope",
        "20260102",
        &[json!({"type":"Person","name":"Alice"})],
    );
    assert_eq!(
        iter_detected_entity_names_since(temporary.path(), "20260101").unwrap(),
        vec![
            (
                "Alice".to_owned(),
                "scope".to_owned(),
                "20260101".to_owned()
            ),
            (
                "Alice".to_owned(),
                "scope".to_owned(),
                "20260102".to_owned()
            ),
        ]
    );
    assert_eq!(
        iter_detected_entity_names_since_strict(temporary.path(), "20260101", None).unwrap(),
        vec![
            (
                "Alice".to_owned(),
                "scope".to_owned(),
                "20260101".to_owned()
            ),
            (
                "Alice".to_owned(),
                "scope".to_owned(),
                "20260102".to_owned()
            ),
        ]
    );

    create_test_facet(temporary.path(), "other");
    let malformed = temporary
        .path()
        .join("facets/other/entities/20260101.jsonl");
    fs::create_dir_all(malformed.parent().unwrap()).unwrap();
    fs::write(malformed, "not json\n").unwrap();
    assert_eq!(
        iter_detected_entity_names_since_strict(temporary.path(), "20260101", Some("scope"),)
            .unwrap(),
        vec![
            (
                "Alice".to_owned(),
                "scope".to_owned(),
                "20260101".to_owned()
            ),
            (
                "Alice".to_owned(),
                "scope".to_owned(),
                "20260102".to_owned()
            ),
        ]
    );
}

#[test]
fn names_since_does_not_deduplicate_rows_within_one_day() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_detected(
        temporary.path(),
        "scope",
        "20260101",
        &[
            json!({"type":"Person","name":"Alice"}),
            json!({"type":"Person","name":"Alice"}),
        ],
    );
    assert_eq!(
        iter_detected_entity_names_since(temporary.path(), "20260101").unwrap(),
        vec![
            (
                "Alice".to_owned(),
                "scope".to_owned(),
                "20260101".to_owned()
            ),
            (
                "Alice".to_owned(),
                "scope".to_owned(),
                "20260101".to_owned()
            ),
        ]
    );
}
