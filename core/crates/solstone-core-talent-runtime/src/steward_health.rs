// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use chrono::{Duration, NaiveDate, Utc};
use serde_json::{Map, Value, json};

pub const STATUS: &str = "## Status";
pub const ATTENTION: &str = "## Needs your attention";
pub const REPAIRS: &str = "## Auto-repairs (last 7d)";

/// Deliberately partial: pipeline-day, recipe-outcome, and source-error facts
/// from Python's `gather_health_facts` are not yet native in this wave.
pub fn gather_health_facts(_journal: &Path, _today: &str) -> Map<String, Value> {
    Map::from_iter([
        (
            "generated_at".to_owned(),
            Value::String(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        ),
        ("data_source_errors".to_owned(), json!([])),
        ("recipe_outcomes_7d".to_owned(), json!([])),
    ])
}

pub fn read_steward_summary(journal: &Path, day: &str) -> Option<Map<String, Value>> {
    let base = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    for offset in 0..=7 {
        let probe = (base - Duration::days(offset)).format("%Y%m%d").to_string();
        let path = journal
            .join("chronicle")
            .join(probe)
            .join("talents/steward.jsonl");
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let mut rows = text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|value| value.as_object().cloned())
                    .map(|row| {
                        (
                            row.get("ts").and_then(Value::as_i64).unwrap_or(0),
                            index,
                            row,
                        )
                    })
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|(ts, index, _)| (*ts, *index));
        if let Some((_, _, row)) = rows.pop() {
            return Some(row);
        };
    }
    None
}
pub fn load_previous_summary(journal: &Path, today: &str) -> Option<Map<String, Value>> {
    read_steward_summary(journal, today)
}

pub fn render_health_body(facts: &Map<String, Value>) -> String {
    let generated_at = facts
        .get("generated_at")
        .and_then(Value::as_str)
        .unwrap_or("1970-01-01T00:00:00Z");
    format!(
        "{STATUS}\n<!-- generated_at: {generated_at} -->\nyour journal is well.\n\n{ATTENTION}\n\n{REPAIRS}\n"
    )
}

pub fn validate_steward_health(body: &str) -> Option<String> {
    let headings = body
        .lines()
        .filter(|line| line.starts_with("## "))
        .collect::<Vec<_>>();
    if headings != [STATUS, ATTENTION, REPAIRS] {
        return Some("sections out of order".to_owned());
    }
    let stamp = body.lines().nth(1).unwrap_or_default();
    if !stamp.starts_with("<!-- generated_at: ") || !stamp.ends_with(" -->") {
        return Some("missing or invalid generated_at".to_owned());
    }
    if body.lines().nth(2).is_none_or(str::is_empty) {
        return Some("empty status section".to_owned());
    }
    None
}

pub fn default_summary_from_body(body: &str) -> Map<String, Value> {
    let status = body.lines().nth(2).unwrap_or("your journal is well.");
    if status.starts_with("your journal is well.") {
        Map::from_iter([
            ("headline".to_owned(), json!("All clear")),
            ("summary_sentence".to_owned(), json!(status)),
            ("suggested_action".to_owned(), json!("none")),
        ])
    } else {
        Map::from_iter([
            ("headline".to_owned(), json!("Needs attention")),
            ("summary_sentence".to_owned(), json!(status)),
            ("suggested_action".to_owned(), json!("open_health_detail")),
        ])
    }
}

pub fn normalize_summary(raw: &str, default: &Map<String, Value>) -> Map<String, Value> {
    let Ok(Value::Object(mut summary)) = serde_json::from_str(raw) else {
        return default.clone();
    };
    let valid = summary
        .get("headline")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
        && summary
            .get("summary_sentence")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
    if !valid {
        return default.clone();
    }
    for (key, limit) in [("headline", 80), ("summary_sentence", 280)] {
        if let Some(Value::String(value)) = summary.get_mut(key) {
            *value = value.chars().take(limit).collect();
        }
    }
    if !matches!(
        summary.get("suggested_action").and_then(Value::as_str),
        Some("none" | "open_health_detail" | "open_support")
    ) {
        summary.insert("suggested_action".to_owned(), json!("none"));
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_body_validates() {
        let body = render_health_body(&gather_health_facts(Path::new("."), "20260101"));
        assert_eq!(validate_steward_health(&body), None);
    }
}
