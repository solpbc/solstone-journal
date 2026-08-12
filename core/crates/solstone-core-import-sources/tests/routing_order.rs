// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use serde_json::Value;
use solstone_core_import_sources::registry::first_claimed;

const GRAMMAR: &str = include_str!("../../../fixtures/import_reference_grammar.json");
const DETECTION: &str = include_str!("../../../fixtures/import_detection_corpus.json");
const RESOLVER: &str = include_str!("../../../fixtures/import_resolver_corpus.json");

#[test]
fn first_claimed_uses_fixture_order_and_fixture_zip_claimants() {
    let grammar: Value = serde_json::from_str(GRAMMAR).unwrap();
    let detection: Value = serde_json::from_str(DETECTION).unwrap();
    let resolver: Value = serde_json::from_str(RESOLVER).unwrap();
    let claims = strings(
        grammar["routing_contract"]["patterns_claimed_by_more_than_one"]["*.zip"]
            .as_array()
            .unwrap(),
    );
    let claim_set = claims.iter().copied().collect::<BTreeSet<_>>();
    let order = strings(
        detection["routing_contract"]["detection_order"]
            .as_array()
            .unwrap(),
    );

    // The live pass raises BodyNativeError with a native usage dump; it is a capture-environment
    // artifact, not a routing observation. native_detector_answers_no is authoritative here.
    for row in [
        "bare::zip_takeout_ics_AND_gemini",
        "bare::zip_claude_AND_ics",
        "bare::dir_vault_3md_AND_pdf",
    ] {
        let claim_set = if row == "bare::dir_vault_3md_AND_pdf" {
            ["obsidian", "document"].into_iter().collect()
        } else {
            claim_set.clone()
        };
        let expected = resolver["passes"]["native_detector_answers_no"][row]["result"]["importer"]
            .as_str()
            .unwrap();
        let selected = first_claimed(&order, |name| claim_set.contains(name)).unwrap();
        assert_eq!(selected, expected);

        let reversed = order.iter().rev().copied().collect::<Vec<_>>();
        let reversed_expected = reversed
            .iter()
            .copied()
            .find(|name| claim_set.contains(name))
            .unwrap();
        let reversed_selected = first_claimed(&reversed, |name| claim_set.contains(name)).unwrap();
        assert_eq!(reversed_selected, reversed_expected);
        assert_ne!(reversed_selected, expected);
    }
}

fn strings(values: &[Value]) -> Vec<&str> {
    values
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .unwrap()
}
