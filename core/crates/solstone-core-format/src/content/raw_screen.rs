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
    frames.sort_by(|left, right| {
        timestamp_seconds(left)
            .partial_cmp(&timestamp_seconds(right))
            .expect("timestamp_seconds always returns a finite value")
    });

    let (base_timestamp, base_hour, base_minute, base_second) = base_time(rel);
    let chunks = frames
        .into_iter()
        .map(|frame| {
            let offset_ms = timestamp_millis(frame);
            let clock_ms = (base_hour * 3_600_000 + base_minute * 60_000 + base_second * 1_000)
                .saturating_add(offset_ms)
                .rem_euclid(86_400_000);
            let hour = clock_ms / 3_600_000;
            let minute = (clock_ms / 60_000).rem_euclid(60);
            let second = (clock_ms / 1_000).rem_euclid(60);
            let millisecond = clock_ms.rem_euclid(1_000);
            let heading = if millisecond == 0 {
                format!("### {hour:02}:{minute:02}:{second:02}")
            } else {
                format!("### {hour:02}:{minute:02}:{second:02}.{millisecond:03}")
            };
            let mut lines = vec![heading, String::new()];
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
            recorded_chunk(
                lines.join("\n"),
                base_timestamp.saturating_add(offset_ms),
                frame,
            )
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

fn timestamp_seconds(frame: &JsonObject) -> f64 {
    frame
        .get("timestamp")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

fn timestamp_millis(frame: &JsonObject) -> i64 {
    (timestamp_seconds(frame) * 1_000.0).round() as i64
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::content::OccurrenceTimeMs;

    fn record(timestamp: Value, marker: &str) -> JsonObject {
        json!({"timestamp": timestamp, "content": {"marker": marker}})
            .as_object()
            .expect("record object")
            .clone()
    }

    #[test]
    fn fractional_offsets_are_sorted_numerically_and_rendered_to_milliseconds() {
        let records = vec![
            record(json!(1.5), "late"),
            record(json!(0.75), "early"),
            record(json!(1), "middle"),
        ];
        let produced = render("20260304/workstation/090000_300/screen.jsonl", &records);

        assert_eq!(produced.chunks.len(), 3);
        assert!(produced.chunks[0].content.starts_with("### 09:00:00.750\n"));
        assert!(produced.chunks[0].content.contains("early"));
        assert!(produced.chunks[1].content.starts_with("### 09:00:01\n"));
        assert!(produced.chunks[1].content.contains("middle"));
        assert!(produced.chunks[2].content.starts_with("### 09:00:01.500\n"));
        assert!(produced.chunks[2].content.contains("late"));
        assert_eq!(
            produced
                .chunks
                .iter()
                .map(|chunk| chunk.occurrence_time_ms)
                .collect::<Vec<_>>(),
            vec![
                Some(OccurrenceTimeMs(1_772_614_800_750)),
                Some(OccurrenceTimeMs(1_772_614_801_000)),
                Some(OccurrenceTimeMs(1_772_614_801_500)),
            ]
        );
    }

    #[test]
    fn equal_numeric_offsets_preserve_source_row_order() {
        let records = vec![record(json!(1.5), "first"), record(json!(1.500), "second")];
        let produced = render("20260304/workstation/090000_300/screen.jsonl", &records);

        assert!(produced.chunks[0].content.contains("first"));
        assert!(produced.chunks[1].content.contains("second"));
    }

    #[test]
    fn signed_zero_offsets_are_equal_and_preserve_source_row_order() {
        let records = vec![
            record(json!(0.0), "positive-zero-first"),
            record(json!(-0.0), "negative-zero-second"),
        ];
        let produced = render("20260304/workstation/090000_300/screen.jsonl", &records);

        assert!(produced.chunks[0].content.contains("positive-zero-first"));
        assert!(produced.chunks[1].content.contains("negative-zero-second"));
    }
}
