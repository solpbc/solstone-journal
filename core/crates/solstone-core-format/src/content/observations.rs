// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{JsonObject, ProducedChunks, display_value, json_truthy, recorded_chunk};

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    ProducedChunks {
        chunks: records.iter().map(render_record).collect(),
        agent_override: Some("observation".to_string()),
        header: Some(observation_header(rel, records.len())),
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

fn render_record(record: &JsonObject) -> super::IndexChunk {
    let content = record.get("content").map(display_value).unwrap_or_default();
    let mut markdown = format!("- {content}");
    if let Some(source_day) = record
        .get("source_day")
        .filter(|value| json_truthy(Some(value)))
    {
        markdown.push_str(" (observed: ");
        markdown.push_str(&display_value(source_day));
        markdown.push(')');
    }
    recorded_chunk(
        markdown,
        record
            .get("observed_at")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        record,
    )
}
