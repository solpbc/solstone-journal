// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;

use serde::Deserialize;
use serde_json::json;

use crate::load_detected_entities_recent;
use crate::store::exclusion_tier;
use crate::store_tests::{
    TempDir, create_test_facet, write_facet_relationship, write_journal_entity,
};

#[derive(Deserialize)]
struct Fixture {
    counts: Counts,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct Counts {
    agree: usize,
    now_hidden: usize,
    now_offered: usize,
    total: usize,
}

#[derive(Deserialize)]
struct Entry {
    fixture_index: usize,
    detected_name: String,
    detected_type: String,
    attached_setup: Setup,
    reference_outcome: Outcome,
    native_outcome: Outcome,
}

#[derive(Deserialize)]
struct Setup {
    kind: String,
    entity_id: String,
    identity_name: Option<String>,
}

#[derive(Deserialize, Debug, PartialEq)]
struct Outcome {
    excluded: bool,
    tier: Option<u8>,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!(
        "../../../fixtures/detected_entity_exclusion_divergences.json"
    ))
    .unwrap()
}

fn write_detection(root: &std::path::Path, name: &str, entity_type: &str) {
    let path = root.join("facets/scope/entities/20260101.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        serde_json::to_string(&json!({"type":entity_type,"name":name,"description":"seen"}))
            .unwrap()
            + "\n",
    )
    .unwrap();
}

fn setup(root: &std::path::Path, setup: &Setup) {
    match setup.kind.as_str() {
        "resolved" | "detached" | "blocked" => {
            write_journal_entity(root, &setup.entity_id, Some(&setup.entity_id));
            fs::write(
                root.join(format!("entities/{}/entity.json", setup.entity_id)),
                serde_json::to_vec(&json!({
                    "id": setup.entity_id,
                    "name": setup.identity_name,
                    "type": "Person",
                    "blocked": setup.kind == "blocked",
                }))
                .unwrap(),
            )
            .unwrap();
            let relationship = if setup.kind == "detached" {
                json!({"entity_id":setup.entity_id,"detached":true})
            } else {
                json!({"entity_id":setup.entity_id})
            };
            write_facet_relationship(root, "scope", &setup.entity_id, relationship);
        }
        "orphan" => write_facet_relationship(
            root,
            "scope",
            "orphan-link",
            json!({"entity_id":setup.entity_id}),
        ),
        other => panic!("unknown fixture setup: {other}"),
    }
}

fn native(entry: &Entry) -> Outcome {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "scope");
    setup(temporary.path(), &entry.attached_setup);
    write_detection(temporary.path(), &entry.detected_name, &entry.detected_type);
    let excluded = load_detected_entities_recent(temporary.path(), "scope", 36500)
        .unwrap()
        .is_empty();
    let tier = exclusion_tier(temporary.path(), "scope", &entry.detected_name)
        .unwrap()
        .map(|tier| tier as u8);
    Outcome { excluded, tier }
}

#[test]
fn detected_entity_exclusion_vectors_execute_native_answers_and_counts() {
    let fixture = fixture();
    assert_eq!(fixture.entries.len(), fixture.counts.total);
    let mut agree = 0;
    let mut now_hidden = 0;
    let mut now_offered = 0;
    for (index, entry) in fixture.entries.iter().enumerate() {
        assert_eq!(entry.fixture_index, index);
        let native_outcome = native(entry);
        assert_eq!(native_outcome, entry.native_outcome);
        match (entry.reference_outcome.excluded, native_outcome.excluded) {
            (same, same_native) if same == same_native => agree += 1,
            (false, true) => now_hidden += 1,
            (true, false) => now_offered += 1,
            _ => unreachable!(),
        }
    }
    assert_eq!(agree, fixture.counts.agree);
    assert_eq!(now_hidden, fixture.counts.now_hidden);
    assert_eq!(now_offered, fixture.counts.now_offered);
    assert_eq!(agree + now_hidden + now_offered, fixture.counts.total);
}
