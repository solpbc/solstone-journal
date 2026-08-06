// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use serde_json::json;
use solstone_core_entity::{record_merge_candidate, save_entity_identity};

use crate::store_tests::{TempDir, create_test_facet, write_facet_relationship};
use crate::{
    accept_candidate, dismiss_candidate, facet_slug, list_scoped_facet_entities, load_candidates,
};

fn resolved_entity_id(root: &std::path::Path) -> String {
    create_test_facet(root, "scope");
    save_entity_identity(
        root,
        "target",
        &json!({"id":"target","name":"Target","type":"Person"}),
        None,
    )
    .unwrap();
    write_facet_relationship(root, "scope", "link", json!({"entity_id":"target"}));
    list_scoped_facet_entities(root, "scope", false, false)
        .unwrap()
        .pop()
        .unwrap()
        .entity_id
}

#[test]
fn record_merge_candidate_updates_one_keyed_row_without_replacing_first_metadata() {
    let temporary = TempDir::new();
    let source_slug = resolved_entity_id(temporary.path());
    let (first, created) = record_merge_candidate(
        temporary.path(),
        "scope",
        "20260101",
        "Target variant",
        &source_slug,
        "Target",
        "canonical",
        "first evidence",
        None,
        Some(2),
        Some(3),
    )
    .unwrap();
    assert!(created);
    let path = temporary.path().join("entities/review-candidates.jsonl");
    let mut rows: Vec<serde_json::Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    rows[0]["updated_at"] = json!("1970-01-01T00:00:00Z");
    fs::write(
        &path,
        rows.iter()
            .map(|row| serde_json::to_string(row).unwrap() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    let (second, created) = record_merge_candidate(
        temporary.path(),
        "scope",
        "20260102",
        "Target variant",
        &source_slug,
        "Target",
        "canonical",
        "second evidence",
        Some("manual"),
        Some(4),
        Some(5),
    )
    .unwrap();
    assert!(!created);
    let raw = fs::read_to_string(&path).unwrap();
    let rows: Vec<serde_json::Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(second["evidence"]["summary"], "second evidence");
    assert_eq!(second["evidence"]["basis"], "manual");
    assert_eq!(second["evidence"]["detection_count"], 4);
    assert_eq!(second["last_surfaced"], "20260102");
    assert_eq!(second["first_surfaced"], first["first_surfaced"]);
    assert_eq!(second["created_at"], first["created_at"]);
    assert_ne!(second["updated_at"], "1970-01-01T00:00:00Z");
    assert_eq!(rows[0], second);
}

#[test]
fn facet_review_candidates_skip_bad_rows_and_update_matching_keys() {
    let temporary = TempDir::new();
    let path = temporary.path().join("facets/review-candidates.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        concat!(
            "{\"name_key\":\"work\",\"name\":\"Work\",\"status\":\"open\",\"count\":4,\"updated_at\":\"1970-01-01T00:00:00Z\"}\n",
            "not json\n",
            "[\"not an object\"]\n",
            "{\"name_key\":\"home\",\"status\":\"open\"}\n",
        ),
    )
    .unwrap();

    assert_eq!(load_candidates(temporary.path()).unwrap().len(), 2);
    assert!(
        accept_candidate(temporary.path(), "missing")
            .unwrap()
            .is_none()
    );
    let accepted = accept_candidate(temporary.path(), "home").unwrap().unwrap();
    assert_eq!(accepted["status"], "accepted");
    let dismissed = dismiss_candidate(temporary.path(), "work")
        .unwrap()
        .unwrap();
    assert_eq!(dismissed["status"], "dismissed");
    assert_eq!(dismissed["dismissed_count"], 4);
    assert_ne!(dismissed["updated_at"], "1970-01-01T00:00:00Z");
    assert!(dismissed["updated_at"].as_str().unwrap().ends_with('Z'));

    let saved = load_candidates(temporary.path()).unwrap();
    assert_eq!(saved.len(), 2);
    assert!(
        saved
            .iter()
            .any(|row| row["name_key"] == "work" && row["status"] == "dismissed")
    );
}

#[test]
fn facet_slug_matches_python_create_facet_normalization() {
    assert_eq!(facet_slug("  Work & Home!  "), "work-home");
    assert_eq!(facet_slug("123 start"), "123-start");
    assert_eq!(facet_slug("équipe"), "quipe");
}
