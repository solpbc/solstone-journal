// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;
use serde_json::Value;

use super::{
    IndexChunk, JsonObject, ProducedChunks, display_value, json_truthy, recorded_chunk, titleize,
};

const SKIP_FIELDS: &[&str] = &[
    "id",
    "type",
    "name",
    "description",
    "updated_at",
    "attached_at",
    "last_seen",
    "detached",
    "tags",
    "aka",
];

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let detected_day = detected_day(rel);
    let chunks = records
        .iter()
        .map(|record| render_record(record, detected_day))
        .collect();
    ProducedChunks {
        chunks,
        agent_override: Some(agent_for_rel(rel).to_string()),
        header: Some(entity_header(rel, records.len())),
        error: None,
        warnings: Vec::new(),
    }
}

fn entity_header(rel: &str, count: usize) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    let facet = parts
        .windows(3)
        .find_map(|parts| (parts[0] == "facets" && parts[2] == "entities").then_some(parts[1]))
        .unwrap_or("unknown");
    match detected_day(rel) {
        Some(day) => format!(
            "# Detected Entities: {facet} ({}-{}-{})\n\n{count} entities",
            &day[..4],
            &day[4..6],
            &day[6..]
        ),
        None => format!("# Attached Entities: {facet}\n\n{count} entities"),
    }
}

fn detected_day(rel: &str) -> Option<&str> {
    let stem = file_stem(rel);
    (stem.len() == 8 && stem.bytes().all(|byte| byte.is_ascii_digit())).then_some(stem)
}

fn agent_for_rel(rel: &str) -> &'static str {
    let stem = file_stem(rel);
    if !stem.is_empty() && stem.chars().all(|ch| ch.is_ascii_digit()) {
        "entity:detected"
    } else {
        "entity:attached"
    }
}

fn file_stem(rel: &str) -> &str {
    let filename = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    filename
        .rsplit_once('.')
        .map(|(stem, _extension)| stem)
        .unwrap_or(filename)
}

fn render_record(record: &JsonObject, detected_day: Option<&str>) -> IndexChunk {
    let entity_type = record
        .get("type")
        .map(display_value)
        .unwrap_or_else(|| "Unknown".to_string());
    let name = record
        .get("name")
        .map(display_value)
        .unwrap_or_else(|| "Unnamed".to_string());
    let mut lines = vec![format!("### {entity_type}: {name}"), String::new()];

    if let Some(description) = record
        .get("description")
        .filter(|value| json_truthy(Some(value)))
    {
        lines.push(display_value(description));
    } else {
        lines.push("*(No description available)*".to_string());
    }
    lines.push(String::new());

    append_array_field(record, "tags", "**Tags:**", &mut lines);
    append_array_field(record, "aka", "**Also known as:**", &mut lines);

    for (key, value) in record {
        if SKIP_FIELDS.contains(&key.as_str()) {
            continue;
        }
        let value_display = match value {
            Value::Array(items) => items
                .iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", "),
            _ => display_value(value),
        };
        lines.push(format!("**{}:** {value_display}", titleize(key)));
    }

    lines.push(String::new());
    recorded_chunk(
        lines.join("\n"),
        entity_timestamp(record, detected_day),
        record,
    )
}

fn entity_timestamp(record: &JsonObject, detected_day: Option<&str>) -> i64 {
    if let Some(day) = detected_day {
        return day_timestamp(day);
    }
    if let Some(day) = record.get("last_seen").and_then(Value::as_str) {
        let timestamp = day_timestamp(day);
        if timestamp != 0 {
            return timestamp;
        }
    }
    for field in ["updated_at", "attached_at"] {
        if let Some(timestamp) = record.get(field).and_then(Value::as_i64) {
            return timestamp;
        }
    }
    1_767_225_600_000
}

fn day_timestamp(day: &str) -> i64 {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .map(|day| {
            day.and_hms_opt(0, 0, 0)
                .expect("midnight is valid")
                .and_utc()
                .timestamp_millis()
        })
        .unwrap_or(0)
}

fn append_array_field(record: &JsonObject, key: &str, label: &str, lines: &mut Vec<String>) {
    let Some(value) = record.get(key).filter(|value| json_truthy(Some(value))) else {
        return;
    };
    let Value::Array(items) = value else {
        return;
    };
    let joined = items
        .iter()
        .map(display_value)
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!("{label} {joined}"));
}
