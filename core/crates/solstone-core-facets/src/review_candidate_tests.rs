// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use serde_json::json;
use solstone_core_entity::{record_merge_candidate, save_entity_identity};

use crate::list_scoped_facet_entities;
use crate::store_tests::{TempDir, create_test_facet, write_facet_relationship};

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
