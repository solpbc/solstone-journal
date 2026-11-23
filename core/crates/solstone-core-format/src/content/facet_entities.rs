// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jiff::tz::Offset;
use serde_json::Value;
use solstone_core_window_catalog::{ENGLISH, strings, text};

use super::{
    IndexChunk, JsonObject, OccurrenceTimeMs, ProducedChunks, display_value, json_truthy, titleize,
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

#[allow(dead_code)]
pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    render_for_locale(rel, records, ENGLISH)
}

pub(super) fn render_for_locale(rel: &str, records: &[JsonObject], locale: &str) -> ProducedChunks {
    let detected_day = detected_day(rel);
    let chunks = records
        .iter()
        .map(|record| render_record(record, detected_day, locale))
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

fn render_record(record: &JsonObject, detected_day: Option<&str>, locale: &str) -> IndexChunk {
    let entity_type = record
        .get("type")
        .map(display_value)
        .unwrap_or_else(|| text(locale, "journal.window.content.entity.type.unknown"));
    let name = record
        .get("name")
        .map(display_value)
        .unwrap_or_else(|| text(locale, "journal.window.content.entity.name.unnamed"));
    let mut lines = vec![format!("### {entity_type}: {name}"), String::new()];

    if let Some(description) = record
        .get("description")
        .filter(|value| json_truthy(Some(value)))
    {
        lines.push(display_value(description));
    } else {
        lines.push(text(
            locale,
            "journal.window.content.entity.description.none",
        ));
    }
    lines.push(String::new());

    append_array_field(
        record,
        "tags",
        "journal.window.content.entity.tags",
        locale,
        &mut lines,
    );
    append_array_field(
        record,
        "aka",
        "journal.window.content.entity.aka",
        locale,
        &mut lines,
    );

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
    IndexChunk {
        content: lines.join("\n"),
        occurrence_time_ms: entity_timestamp(record, detected_day).map(OccurrenceTimeMs),
        source: Some(record.clone()),
        group_offset_ms: None,
    }
}

/// When a facet entity record's activity happened, if it says.
///
/// Links store `updated_at` and `attached_at` as epoch milliseconds or RFC
/// 3339 text, depending on which release wrote them. A record with neither
/// and no day is indexed without a time rather than at a stand-in date.
fn entity_timestamp(record: &JsonObject, detected_day: Option<&str>) -> Option<i64> {
    if let Some(day) = detected_day {
        return day_timestamp(day);
    }
    if let Some(timestamp) = record
        .get("last_seen")
        .and_then(Value::as_str)
        .and_then(day_timestamp)
    {
        return Some(timestamp);
    }
    ["updated_at", "attached_at"]
        .into_iter()
        .find_map(|field| stored_timestamp_ms(record.get(field)))
}

fn stored_timestamp_ms(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => solstone_core_time::parse_rfc3339(text)
            .ok()
            .map(|value| value.timestamp().as_millisecond()),
        _ => None,
    }
}

fn day_timestamp(day: &str) -> Option<i64> {
    solstone_core_time::parse_day(day)
        .and_then(|day| {
            Offset::UTC
                .to_timestamp(day.to_datetime(jiff::civil::Time::midnight()))
                .ok()
        })
        .map(|instant| instant.as_millisecond())
}

fn append_array_field(
    record: &JsonObject,
    key: &str,
    message_id: &str,
    locale: &str,
    lines: &mut Vec<String>,
) {
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
    lines.push(strings(locale, message_id, [("items", joined.as_str())]));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn record(value: Value) -> JsonObject {
        value.as_object().expect("record is an object").clone()
    }

    #[test]
    fn a_link_stamped_with_rfc3339_text_keeps_its_time() {
        let chunk = render_record(
            &record(json!({"name":"Alice","attached_at":"2026-09-27T18:30:00.000000Z"})),
            None,
            ENGLISH,
        );
        assert_eq!(
            chunk.occurrence_time_ms,
            Some(OccurrenceTimeMs(1_790_533_800_000))
        );
    }

    #[test]
    fn a_link_with_no_activity_is_indexed_without_a_time() {
        let chunk = render_record(&record(json!({"name":"Alice"})), None, ENGLISH);
        assert_eq!(chunk.occurrence_time_ms, None);
    }
}
