// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::time::Duration;

use serde_json::json;
use solstone_core_entity::{record_merge_candidate, save_entity_identity};
use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::store_tests::{TempDir, create_test_facet, write_facet_relationship};
use crate::{
    SpeculativeFacetCandidate, SpeculativeFacetSample, accept_candidate, dismiss_candidate,
    facet_slug, list_scoped_facet_entities, load_candidates, record_facet_candidates,
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

fn candidate(name: &str, name_key: &str, count: usize) -> SpeculativeFacetCandidate {
    SpeculativeFacetCandidate {
        name: name.to_owned(),
        name_key: name_key.to_owned(),
        count,
        window_days: 14,
        samples: vec![SpeculativeFacetSample {
            day: "20260810".to_owned(),
            stream: "archon".to_owned(),
            segment: "090000_300".to_owned(),
            unrepresentable: false,
        }],
    }
}

#[test]
fn record_facet_candidates_preserves_owner_decisions_and_unknown_fields() {
    let temporary = TempDir::new();
    let path = temporary.path().join("facets/review-candidates.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        concat!(
            "{\"name\":\"Dismissed\",\"name_key\":\"dismissed\",\"status\":\"dismissed\",\"count\":1,\"window_days\":1,\"evidence\":{\"samples\":[],\"review_note\":\"keep\"},\"first_surfaced\":\"20200101\",\"last_surfaced\":\"20200101\",\"created_at\":\"2020-01-01T00:00:00Z\",\"updated_at\":\"2020-01-01T00:00:00Z\",\"dismissed_count\":1,\"custom_flag\":true}\n",
            "{\"name\":\"Accepted\",\"name_key\":\"accepted\",\"status\":\"accepted\",\"count\":1,\"window_days\":1,\"evidence\":{\"samples\":[]},\"first_surfaced\":\"20200102\",\"last_surfaced\":\"20200101\",\"created_at\":\"2020-01-02T00:00:00Z\",\"updated_at\":\"2020-01-01T00:00:00Z\"}\n",
        ),
    )
    .unwrap();
    let before = load_candidates(temporary.path()).unwrap();

    assert_eq!(
        record_facet_candidates(
            temporary.path(),
            "20260810",
            &[
                candidate("dismissed", "dismissed", 4),
                candidate("accepted", "accepted", 5)
            ],
        )
        .unwrap(),
        2
    );

    let rows = load_candidates(temporary.path()).unwrap();
    for name_key in ["dismissed", "accepted"] {
        let old = before
            .iter()
            .find(|row| row["name_key"] == name_key)
            .unwrap();
        let updated = rows.iter().find(|row| row["name_key"] == name_key).unwrap();
        assert_eq!(updated["status"], old["status"]);
        assert_eq!(updated["first_surfaced"], old["first_surfaced"]);
        assert_eq!(updated["created_at"], old["created_at"]);
        assert_ne!(updated["count"], old["count"]);
        assert_ne!(updated["window_days"], old["window_days"]);
        assert_ne!(updated["last_surfaced"], old["last_surfaced"]);
        assert_ne!(updated["updated_at"], old["updated_at"]);
        assert_eq!(updated["evidence"]["samples"][0]["segment"], "090000_300");
    }
    let dismissed = rows
        .iter()
        .find(|row| row["name_key"] == "dismissed")
        .unwrap();
    let old_dismissed = before
        .iter()
        .find(|row| row["name_key"] == "dismissed")
        .unwrap();
    assert_eq!(
        dismissed["dismissed_count"],
        old_dismissed["dismissed_count"]
    );
    assert_eq!(dismissed["custom_flag"], old_dismissed["custom_flag"]);
    assert_eq!(
        dismissed["evidence"]["review_note"],
        old_dismissed["evidence"]["review_note"]
    );
}

#[test]
fn record_facet_candidates_tolerates_bad_rows_without_duplicate_upserts() {
    let temporary = TempDir::new();
    let path = temporary.path().join("facets/review-candidates.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        concat!(
            "\n",
            "[\"not\",\"an\",\"object\"]\n",
            "{\"name\":\"Home Reno\",\"name_key\":\"home reno\",\"status\":\"open\",\"count\":3}\n",
        ),
    )
    .unwrap();

    assert_eq!(
        record_facet_candidates(
            temporary.path(),
            "20260810",
            &[candidate("Home Reno", "home reno", 4)]
        )
        .unwrap(),
        1
    );

    // [check] The tolerant reader drops blank and non-object rows; this checks
    // that the surviving keyed row is updated rather than duplicated.
    let rows = load_candidates(temporary.path()).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row["name_key"] == "home reno")
            .count(),
        1
    );
    assert_eq!(rows[0]["count"], 4);
}

#[test]
fn record_facet_candidates_empty_batch_does_not_touch_the_store() {
    let temporary = TempDir::new();
    assert_eq!(
        record_facet_candidates(temporary.path(), "20260810", &[]).unwrap(),
        0
    );
    assert!(!temporary.path().join("facets").exists());

    let path = temporary.path().join("facets/review-candidates.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{\"name_key\":\"kept\"}\n").unwrap();
    let before = fs::read(&path).unwrap();

    assert_eq!(
        record_facet_candidates(temporary.path(), "20260810", &[]).unwrap(),
        0
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn record_facet_candidates_propagates_lock_contention_without_writing() {
    let temporary = TempDir::new();
    let path = temporary.path().join("facets/review-candidates.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{\"name_key\":\"kept\"}\n").unwrap();
    let before = fs::read(&path).unwrap();
    let _lock = hold_lock(
        &path,
        LockOptions {
            timeout: Duration::from_millis(50),
            ..LockOptions::default()
        },
    )
    .unwrap();

    assert!(
        record_facet_candidates(
            temporary.path(),
            "20260810",
            &[candidate("Kept", "kept", 4)]
        )
        .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), before);
}
