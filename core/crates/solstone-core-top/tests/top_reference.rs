// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use solstone_core_top::TopState;

const FIXTURE: &str = include_str!("../../../fixtures/top_reference.json");
const EXPECTED_SHA256: &str = "8b8c18d9cb122e99d40422f4355f9b5d60498af7d89a1e2bff5dd1b2d0531545";

#[test]
fn retained_top_reference_digest_schema_and_event_order_are_stable() {
    assert_eq!(
        format!("{:x}", Sha256::digest(FIXTURE.as_bytes())),
        EXPECTED_SHA256
    );
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture JSON");
    let root = fixture.as_object().expect("fixture object");
    for key in [
        "schema",
        "events",
        "malformed_events",
        "renders",
        "actions",
        "loop",
        "formatting",
        "process_matrix",
        "provenance",
    ] {
        assert!(root.contains_key(key), "missing {key}");
    }
    assert_eq!(fixture["events"].as_array().unwrap().len(), 21);
    assert_eq!(fixture["malformed_events"].as_array().unwrap().len(), 13);
    assert_eq!(fixture["renders"].as_array().unwrap().len(), 10);
    assert_eq!(fixture["actions"].as_array().unwrap().len(), 7);
    assert_eq!(fixture["loop"].as_array().unwrap().len(), 16);
    let actual: Vec<(&str, &str)> = fixture["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["event"]["tract"].as_str().unwrap(),
                entry["event"]["event"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        actual,
        vec![
            ("supervisor", "status"),
            ("supervisor", "status"),
            ("supervisor", "restarting"),
            ("supervisor", "started"),
            ("supervisor", "stopped"),
            ("supervisor", "queue"),
            ("supervisor", "queue"),
            ("logs", "line"),
            ("logs", "exec"),
            ("logs", "line"),
            ("logs", "exit"),
            ("logs", "exit"),
            ("observe", "status"),
            ("observe", "status"),
            ("observe", "observed"),
            ("observe", "observed"),
            ("observe", "observed"),
            ("observe", "observed"),
            ("think", "started"),
            ("think", "status"),
            ("think", "completed")
        ]
    );
}

#[test]
fn fixture_projection_round_trips_all_twenty_manager_keys() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let original = &fixture["actions"][0]["state"];
    let state = TopState::from_fixture_value(original).unwrap();
    assert_eq!(state.fixture_value(), *original);
    assert_eq!(state.fixture_value().as_object().unwrap().len(), 20);
}
