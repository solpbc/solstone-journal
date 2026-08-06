// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{
    JsonObject, ProducedChunks, display_value, json_truthy, recorded_chunk,
    stripped_truthy_display, titleize,
};

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();
    for record in records {
        let normalized = normalize_record(record);
        let mut lines = vec![format!("### {}", fallback_title(record))];

        if let Some(activity) = activity_type(record) {
            lines.push(format!("- Activity: {activity}"));
        }
        if let Some(facet) = stripped_truthy_display(record, "facet") {
            lines.push(format!("- Facet: {facet}"));
        }
        if let Some(day) = stripped_truthy_display(record, "day") {
            lines.push(format!("- Day: {day}"));
        }
        if let Some(time_range) = activity_time_range(record.get("segments")) {
            lines.push(format!("- Time: {time_range}"));
        }
        if let Some(level) = record.get("level_avg") {
            lines.push(format!("- Level: {}", display_value(level)));
        }
        if let Some(description) = stripped_truthy_display(record, "description") {
            lines.push(format!("- Description: {description}"));
        }
        if let Some(details) = stripped_truthy_display(record, "details") {
            lines.push(format!("- Details: {details}"));
        }
        if let Some(participation) = participation(record) {
            lines.push(format!("- Participation: {participation}"));
        }

        if let Some(Value::Object(story)) = record.get("story") {
            if let Some(body) = story.get("body").and_then(Value::as_str) {
                let stripped = body.trim();
                if !stripped.is_empty() {
                    lines.push(String::new());
                    lines.push(stripped.to_string());
                }
            }
            if let Some(Value::Array(topics)) = story.get("topics") {
                let topic_values: Vec<String> = topics
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|topic| !topic.is_empty())
                    .map(str::to_string)
                    .collect();
                if !topic_values.is_empty() {
                    lines.push(format!("Topics: {}", topic_values.join(", ")));
                }
            }
        }

        if json_truthy(record.get("hidden")) {
            lines.push("- Hidden: yes".to_string());
        }

        let occurrence = record
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        chunks.push(recorded_chunk(lines.join("\n"), occurrence, &normalized));
    }
    ProducedChunks {
        chunks,
        agent_override: Some("activity".to_string()),
        header: Some(activity_header(rel)),
        warnings: Vec::new(),
    }
}

fn normalize_record(record: &JsonObject) -> JsonObject {
    let mut normalized = record.clone();
    normalized.insert("title".to_string(), Value::String(source_title(record)));
    normalized.insert(
        "details".to_string(),
        Value::String(
            record
                .get("details")
                .map(display_value)
                .filter(|value| !value.is_empty())
                .unwrap_or_default(),
        ),
    );
    normalized.insert(
        "hidden".to_string(),
        Value::Bool(json_truthy(record.get("hidden"))),
    );
    normalized.insert(
        "edits".to_string(),
        Value::Array(
            record
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|edit| edit.is_object())
                .cloned()
                .collect(),
        ),
    );
    normalized
}

fn source_title(record: &JsonObject) -> String {
    if let Some(title) = stripped_truthy_display(record, "title") {
        return title;
    }
    if let Some(description) = stripped_truthy_display(record, "description") {
        return description;
    }
    if let Some(activity) = activity_type(record) {
        return titleize(&activity);
    }
    "untitled activity".to_string()
}

fn activity_header(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    let facet = parts
        .windows(3)
        .find_map(|parts| (parts[0] == "facets" && parts[2] == "activities").then_some(parts[1]))
        .unwrap_or("unknown");
    let day = rel
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".jsonl"));
    match day.filter(|value| value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())) {
        Some(day) => format!(
            "# Activities: {facet} ({}-{}-{})",
            &day[..4],
            &day[4..6],
            &day[6..]
        ),
        None => format!("# Activities: {facet}"),
    }
}

fn fallback_title(record: &JsonObject) -> String {
    if let Some(title) = stripped_truthy_display(record, "title") {
        return title;
    }
    if let Some(description) = stripped_truthy_display(record, "description") {
        return description;
    }
    if let Some(activity) = activity_type(record) {
        return titleize(&activity);
    }
    "Untitled activity".to_string()
}

fn activity_type(record: &JsonObject) -> Option<String> {
    stripped_truthy_display(record, "activity").or_else(|| stripped_truthy_display(record, "id"))
}

fn participation(record: &JsonObject) -> Option<String> {
    let Value::Array(entries) = record.get("participation")? else {
        return None;
    };
    let names: Vec<String> = entries
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|entry| {
            stripped_truthy_display(entry, "name")
                .or_else(|| stripped_truthy_display(entry, "entity_id"))
        })
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

fn activity_time_range(value: Option<&Value>) -> Option<String> {
    let Value::Array(segments) = value? else {
        return None;
    };
    let first = segments.first()?.as_str()?;
    let last = segments.last()?.as_str()?;
    let (start_hour, start_minute, _, _) = parse_segment(first)?;
    let (_, _, end_second, duration) = parse_segment(last)?;
    let end_second = (end_second + duration).min(23 * 3600 + 59 * 60 + 59);
    Some(format!(
        "{start_hour:02}:{start_minute:02}-{:02}:{:02}",
        end_second / 3600,
        (end_second % 3600) / 60
    ))
}

fn parse_segment(segment: &str) -> Option<(u32, u32, u32, u32)> {
    let (time_part, length_part) = segment.split_once('_')?;
    if time_part.len() != 6
        || !time_part.bytes().all(|byte| byte.is_ascii_digit())
        || length_part.is_empty()
        || !length_part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hour = time_part[0..2].parse::<u32>().ok()?;
    let minute = time_part[2..4].parse::<u32>().ok()?;
    let second = time_part[4..6].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let duration = length_part.parse::<u32>().ok()?;
    Some((hour, minute, hour * 3600 + minute * 60 + second, duration))
}
