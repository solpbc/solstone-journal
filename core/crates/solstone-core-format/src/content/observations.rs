// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_window_catalog::{ENGLISH, strings};

use super::{JsonObject, ProducedChunks, display_value, json_truthy, recorded_chunk_for_locale};

#[allow(dead_code)]
pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    render_for_locale(rel, records, ENGLISH)
}

pub(super) fn render_for_locale(rel: &str, records: &[JsonObject], locale: &str) -> ProducedChunks {
    let mut live_records: Vec<_> = records
        .iter()
        .filter(|record| !json_truthy(record.get("retired")))
        .collect();
    live_records.sort_by(|a, b| {
        let key = |rec: &JsonObject| {
            let sd = rec
                .get("source_day")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .replace('-', "");
            let obs = rec.get("observed_at").and_then(Value::as_i64).unwrap_or(0);
            (sd, obs)
        };
        key(b).cmp(&key(a))
    });
    ProducedChunks {
        chunks: live_records
            .iter()
            .copied()
            .map(|record| render_record(record, locale))
            .collect(),
        agent_override: Some("observation".to_string()),
        header: Some(observation_header(rel, live_records.len())),
        error: None,
        warnings: Vec::new(),
    }
}

fn observation_header(rel: &str, count: usize) -> String {
    let slug = rel
        .rsplit_once('/')
        .and_then(|(parent, _)| parent.rsplit('/').next())
        .unwrap_or("unknown");
    let entity = slug
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ");
    format!("# Observations: {entity}\n\n{count} observations")
}

fn render_record(record: &JsonObject, locale: &str) -> super::IndexChunk {
    let content = record.get("content").map(display_value).unwrap_or_default();
    let markdown = if let Some(source_day) = record
        .get("source_day")
        .filter(|value| json_truthy(Some(value)))
    {
        let day = display_value(source_day);
        strings(
            locale,
            "journal.window.content.observation.observed",
            [("content", content.as_str()), ("day", day.as_str())],
        )
    } else {
        format!("- {content}")
    };
    recorded_chunk_for_locale(
        markdown,
        record
            .get("observed_at")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        record,
        locale,
    )
}
