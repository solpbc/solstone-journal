// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use serde::Deserialize;
use serde_json::{Value, json};

use crate::attach_or_reactivate_entity;
use crate::store_tests::{TempDir, create_test_facet, write_journal_entity};

#[derive(Deserialize, Clone)]
struct Fixture {
    counts: Counts,
    entries: Vec<Entry>,
}
#[derive(Deserialize, Clone)]
struct Counts {
    total: usize,
}
#[derive(Deserialize, Clone)]
struct Entry {
    fixture_index: usize,
    query: String,
    candidate_ids: Vec<String>,
    candidate_name: String,
    reference_outcome: Value,
    native_outcome: Value,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!(
        "../../../fixtures/facet_entity_attach_collision_divergences.json"
    ))
    .unwrap()
}
fn native(entry: &Entry) -> Value {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    write_journal_entity(
        temporary.path(),
        "opaque_dir",
        Some(&entry.candidate_ids[0]),
    );
    std::fs::write(
        temporary.path().join("entities/opaque_dir/entity.json"),
        serde_json::to_vec(&json!({"id":entry.candidate_ids[0],"name":entry.candidate_name}))
            .unwrap(),
    )
    .unwrap();
    match attach_or_reactivate_entity(temporary.path(), "scope", "kind", &entry.query, "") {
        Ok(result) if !result.reactivated => json!({"kind":"attached_existing"}),
        Ok(_) => json!({"kind":"reactivated"}),
        Err(error) => json!({"kind":"error","detail":error.to_string()}),
    }
}
fn verify(fixture: Fixture) -> Result<(), &'static str> {
    if fixture.entries.len() != fixture.counts.total {
        return Err("count");
    }
    for entry in &fixture.entries {
        if entry.fixture_index >= fixture.counts.total {
            return Err("index");
        }
        if native(entry) != entry.native_outcome {
            return Err("native");
        }
    }
    Ok(())
}
#[test]
fn attach_collision_divergence_vectors_execute_native_answers() {
    assert!(verify(fixture()).is_ok());
}
#[test]
fn missing_attach_collision_divergence_fails_count_check() {
    let mut fixture = fixture();
    fixture.entries.pop();
    assert_eq!(verify(fixture), Err("count"));
}
#[test]
fn altered_attach_collision_native_value_fails_assertion() {
    let mut fixture = fixture();
    fixture.entries[0].native_outcome = json!({"kind":"created"});
    assert_eq!(verify(fixture), Err("native"));
}
#[test]
fn reference_answers_are_distinct_from_native_answers() {
    let fixture = fixture();
    assert!(
        fixture
            .entries
            .iter()
            .all(|entry| entry.reference_outcome != entry.native_outcome)
    );
}
