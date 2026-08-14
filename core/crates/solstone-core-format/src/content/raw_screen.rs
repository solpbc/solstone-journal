// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;
use serde_json::{Map, Value};

use super::{JsonObject, ProducedChunks, json_truthy, recorded_chunk};
use crate::segment::{is_date_key, segment_parse};

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let mut skipped = 0usize;
    let mut frames = Vec::new();
    for (index, record) in records.iter().enumerate() {
        if index == 0 && !record.contains_key("timestamp") && record.contains_key("raw") {
            continue;
        }
        if record.contains_key("timestamp") {
            frames.push(record);
        } else {
            skipped += 1;
        }
    }
    frames.sort_by_key(|frame| timestamp(frame));

    let (base_timestamp, base_hour, base_minute, base_second) = base_time(rel);
    let chunks = frames
        .into_iter()
        .map(|frame| {
            let offset = timestamp(frame);
            let total_seconds = base_hour * 3600 + base_minute * 60 + base_second + offset;
            let hour = (total_seconds / 3600).rem_euclid(24);
            let minute = (total_seconds / 60).rem_euclid(60);
            let second = total_seconds.rem_euclid(60);
            let mut lines = vec![
                format!("### {hour:02}:{minute:02}:{second:02}"),
                String::new(),
            ];
            if let Some(analysis) = frame.get("analysis").and_then(Value::as_object) {
                let category = analysis
                    .get("primary")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                lines.push(format!("**Category:** {category}"));
                lines.push(String::new());
                if let Some(description) = analysis
                    .get("visual_description")
                    .and_then(Value::as_str)
                    .filter(|description| !description.is_empty())
                {
                    lines.push(description.to_owned());
                    lines.push(String::new());
                }
            }
            if let Some(content) = frame.get("content").and_then(Value::as_object) {
                for (category, value) in content {
                    if !json_truthy(Some(value)) {
                        continue;
                    }
                    let formatted = format_category(category, value);
                    if !formatted.is_empty() {
                        lines.push(formatted);
                    }
                }
            }
            recorded_chunk(lines.join("\n"), base_timestamp + offset * 1000, frame)
        })
        .collect();

    ProducedChunks {
        chunks,
        agent_override: Some("screen".to_string()),
        header: Some(screen_header(rel)),
        error: (skipped > 0)
            .then(|| format!("Skipped {skipped} entries missing 'timestamp' field in {rel}")),
        warnings: Vec::new(),
    }
}

fn base_time(rel: &str) -> (i64, i64, i64, i64) {
    let Some(times) = segment_parse(rel) else {
        return (0, 0, 0, 0);
    };
    let hour = i64::from(times.hour);
    let minute = i64::from(times.minute);
    let second = i64::from(times.second);
    let base_timestamp = rel
        .split('/')
        .find(|part| is_date_key(part))
        .and_then(|day| NaiveDate::parse_from_str(day, "%Y%m%d").ok())
        .and_then(|day| {
            day.and_hms_opt(times.hour.into(), times.minute.into(), times.second.into())
        })
        .map(|time| time.and_utc().timestamp_millis())
        .unwrap_or(0);
    (base_timestamp, hour, minute, second)
}

fn timestamp(frame: &JsonObject) -> i64 {
    frame.get("timestamp").and_then(Value::as_i64).unwrap_or(0)
}

fn screen_header(rel: &str) -> String {
    let stem = rel
        .rsplit('/')
        .next()
        .unwrap_or(rel)
        .strip_suffix(".jsonl")
        .unwrap_or(rel);
    let (position, connector) = parse_screen_filename(stem);
    if position == "unknown" || connector == "unknown" {
        "# Frame Analyses".to_string()
    } else {
        format!("# Frame Analyses ({position} - {connector})")
    }
}

fn parse_screen_filename(filename: &str) -> (&str, &str) {
    let Some(prefix) = filename.strip_suffix("_screen") else {
        return ("unknown", "unknown");
    };
    let Some((position, connector)) = prefix.rsplit_once('_') else {
        return ("unknown", "unknown");
    };
    let position_valid = !position.is_empty()
        && position
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-');
    let connector_valid = !connector.is_empty()
        && connector
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if position_valid && connector_valid {
        (position, connector)
    } else {
        ("unknown", "unknown")
    }
}

fn format_category(category: &str, content: &Value) -> String {
    if category == "meeting"
        && let Some(content) = content.as_object()
    {
        return format_meeting(content);
    }
    match content {
        Value::String(content) => {
            format!("**{}:**\n\n{}\n", python_title(category), content.trim())
        }
        Value::Object(content) => format!(
            "**{}:**\n\n```json\n{}\n```\n",
            python_title(category),
            serde_json::to_string_pretty(content).unwrap_or_default()
        ),
        _ => String::new(),
    }
}

fn format_meeting(content: &Map<String, Value>) -> String {
    let mut lines = vec![
        format!(
            "**Meeting** ({})",
            content
                .get("platform")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
        String::new(),
    ];
    if let Some(participants) = content.get("participants").and_then(Value::as_array)
        && !participants.is_empty()
    {
        lines.push("**Participants:**".to_string());
        for participant in participants {
            let Some(participant) = participant.as_object() else {
                continue;
            };
            let name = participant
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Unknown");
            let status = participant
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let video = participant
                .get("video")
                .is_some_and(|value| json_truthy(Some(value)));
            lines.push(format!(
                "- {} {name} ({status})",
                if video { "📹" } else { "🔇" }
            ));
        }
        lines.push(String::new());
    }
    if let Some(screen_share) = content.get("screen_share").and_then(Value::as_object)
        && !screen_share.is_empty()
    {
        let presenter = screen_share.get("presenter").and_then(Value::as_str);
        let description = screen_share
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let formatted_text = screen_share
            .get("formatted_text")
            .and_then(Value::as_str)
            .unwrap_or("");
        let presenter = presenter
            .map(|value| format!(" by {value}"))
            .unwrap_or_default();
        lines.push(format!("**Screen Share{presenter}:**"));
        if !description.is_empty() {
            lines.push(format!("*{description}*"));
        }
        lines.push(String::new());
        if !formatted_text.is_empty() {
            lines.push(formatted_text.trim().to_string());
            lines.push(String::new());
        }
    }
    lines.join("\n")
}

fn python_title(value: &str) -> String {
    let mut result = String::new();
    let mut capitalize = true;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if capitalize {
                result.extend(character.to_uppercase());
            } else {
                result.extend(character.to_lowercase());
            }
            capitalize = false;
        } else {
            result.push(character);
            capitalize = true;
        }
    }
    result
}
