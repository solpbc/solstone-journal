// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use serde_json::Value;
use solstone_core_import_sources::registry::ORDERED_FILE_IMPORTER_NAMES;

const GRAMMAR: &str = include_str!("../../../fixtures/import_reference_grammar.json");
const DETECTION: &str = include_str!("../../../fixtures/import_detection_corpus.json");
const CAPTURE_REV: &str = "86fd678a6b3aec2eb4f33a4c934f0cf34a099542";

#[test]
fn registry_and_auxiliary_grammar_match_the_frozen_fixture_contract() {
    let grammar: Value = serde_json::from_str(GRAMMAR).unwrap();
    let detection: Value = serde_json::from_str(DETECTION).unwrap();
    let file_registry = grammar["file_importer_registry_ordered"]
        .as_array()
        .unwrap();
    let file_names = file_registry
        .iter()
        .map(|row| row.as_array().unwrap())
        .map(|row| {
            assert_eq!(row.len(), 2);
            row[0].as_str().unwrap()
        })
        .collect::<Vec<_>>();
    let detection_names = strings(
        detection["routing_contract"]["detection_order"]
            .as_array()
            .unwrap(),
    );

    assert_eq!(file_names.len(), detection_names.len());
    assert_eq!(file_names, detection_names);
    assert_eq!(file_names, ORDERED_FILE_IMPORTER_NAMES);
    assert!(file_names.contains(&"journal_archive"));

    let importer_rows = grammar["importers"].as_array().unwrap();
    assert_eq!(importer_rows.len(), file_registry.len());
    for row in importer_rows {
        assert_key_set(
            row,
            ["name", "display_name", "file_patterns", "description"],
        );
    }

    let syncable = grammar["syncable_registry_ordered"].as_array().unwrap();
    let syncable_names = syncable
        .iter()
        .map(|row| row.as_array().unwrap())
        .map(|row| {
            assert_eq!(row.len(), 2);
            row[0].as_str().unwrap()
        })
        .collect::<Vec<_>>();
    let syncable_backends = strings(
        grammar["syncable_backends_instantiated"]
            .as_array()
            .unwrap(),
    );
    assert_eq!(syncable_names, ["plaud", "obsidian", "audio"]);
    assert_eq!(syncable_names, syncable_backends);

    let native_sync_backends = strings(grammar["native_sync_backends"].as_array().unwrap());
    assert_eq!(native_sync_backends, ["oura"]);

    assert_key_set(
        &grammar["journal_source"],
        [
            "subcommands",
            "top_level_flag",
            "list_mode_choices",
            "parses_with_parse_known",
            "stale_prog_string",
            "note",
        ],
    );
    let journal_subcommands = strings(grammar["journal_source"]["subcommands"].as_array().unwrap());
    assert_eq!(journal_subcommands, ["create", "list", "status", "revoke"]);

    assert_key_set(
        &detection["routing_contract"],
        [
            "rule",
            "detection_order",
            "file_patterns_are_NOT_the_predicate",
            "zip_claimants",
        ],
    );

    // This worktree is ahead of the capture revision; the fixture is the identity, not HEAD.
    assert_eq!(grammar["provenance"]["captured_from_rev"], CAPTURE_REV);
}

fn strings(values: &[Value]) -> Vec<&str> {
    values
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .unwrap()
}

fn assert_key_set<const N: usize>(value: &Value, expected: [&str; N]) {
    let actual = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = expected.into_iter().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
}
