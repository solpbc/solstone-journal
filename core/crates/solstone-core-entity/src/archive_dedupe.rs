// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Narrow, non-destructive dedupe operations shared by archive import.

use std::collections::HashSet;

use serde_json::Value;

/// Case-insensitively deduplicate aliases, preserving the first spelling and
/// sorting by the case-folded spelling. This is the archive-safe subset of the
/// entity merge planner.
pub fn archive_dedupe_akas(target_values: &[String], source_values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for value in target_values.iter().chain(source_values) {
        if seen.insert(value.to_lowercase()) {
            output.push(value.clone());
        }
    }
    output.sort_by_key(|value| value.to_lowercase());
    output
}

/// Case-insensitively deduplicate email values in target-then-source order.
pub fn archive_dedupe_emails(target_values: &[String], source_values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    target_values
        .iter()
        .chain(source_values)
        .filter(|value| seen.insert(value.to_lowercase()))
        .cloned()
        .collect()
}

/// Deduplicate observation objects on `(content, observed_at)`, keeping target
/// rows before source rows.
pub fn archive_dedupe_observations(source: &[Value], target: &[Value]) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for value in target.iter().chain(source) {
        let key = (
            value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            value.get("observed_at").cloned().unwrap_or(Value::Null),
        );
        if seen.insert(key) {
            result.push(value.clone());
        }
    }
    result
}
