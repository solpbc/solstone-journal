// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use serde_json::Value;
use solstone_core_import::ImportPreview;
use solstone_core_import_sources::{chatgpt, claude, gemini, kindle};
use support::TempTree;

const ORACLE: &str = include_str!("../../../fixtures/import-sources-oracle.json");

#[test]
fn preview_oracle_honors_status_data_and_atomic_summary_units() {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    let tree = TempTree::new();
    for (name, case) in oracle["cases"].as_object().unwrap() {
        let preview = match name.as_str() {
            "claude" => claude::preview(&support::claude_archive(&tree)).unwrap(),
            "chatgpt" => chatgpt::preview(&support::chatgpt_archive(&tree)).unwrap(),
            "gemini" => gemini::preview(&support::gemini_archive(&tree)).unwrap(),
            "kindle" => kindle::preview(&support::kindle_clippings(&tree)).unwrap(),
            other => panic!("unexpected oracle source {other}"),
        };
        let expected = case
            .get("w7_expected")
            .unwrap_or(&case["captured"]["preview"]);
        if case.get("w7_expected").is_some() {
            assert_ne!(case["status"].as_str(), Some("expectation"));
        }
        assert_preview_matches(&preview, expected);
    }
}

fn assert_preview_matches(actual: &ImportPreview, expected: &Value) {
    assert_eq!(
        actual.date_range.0,
        expected["date_range"][0].as_str().unwrap()
    );
    assert_eq!(
        actual.date_range.1,
        expected["date_range"][1].as_str().unwrap()
    );
    assert_eq!(actual.item_count, expected["item_count"].as_u64().unwrap());
    assert_eq!(
        actual.entity_count,
        expected["entity_count"].as_u64().unwrap()
    );
    assert_eq!(actual.summary, expected["summary"].as_str().unwrap());
}
