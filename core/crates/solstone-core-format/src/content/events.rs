// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{NaiveDate, NaiveTime};
use serde_json::Value;

use super::{
    JsonObject, ProducedChunks, capitalize, display_value, json_truthy, recorded_chunk,
    truthy_display,
};

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();
    for event in records {
        let Some(title) = truthy_display(event, "title") else {
            continue;
        };
        let event_type =
            capitalize(&truthy_display(event, "type").unwrap_or_else(|| "event".to_string()));
        let occurred = event
            .get("occurred")
            .is_none_or(|value| json_truthy(Some(value)));
        let planned_prefix = if occurred { "" } else { "Planned " };
        let mut lines = vec![
            format!("### {planned_prefix}{event_type}: {title}"),
            String::new(),
        ];

        let start_time = event.get("start").and_then(Value::as_str).unwrap_or("");
        if !start_time.is_empty() {
            let end_time = event.get("end").and_then(Value::as_str).unwrap_or("");
            let label = if occurred { "Occurred" } else { "Scheduled" };
            let start_display = first_five(start_time);
            if end_time.is_empty() {
                lines.push(format!("**Time {label}:** {start_display}"));
            } else {
                lines.push(format!(
                    "**Time {label}:** {start_display} - {}",
                    first_five(end_time)
                ));
            }
        }

        if let Some(Value::Array(participants)) = event.get("participants") {
            let names: Vec<String> = participants
                .iter()
                .filter_map(|value| {
                    let display = display_value(value);
                    if display.is_empty() {
                        None
                    } else {
                        Some(display)
                    }
                })
                .collect();
            if !names.is_empty() {
                let label = if occurred {
                    "Participants"
                } else {
                    "Expected Participants"
                };
                lines.push(format!("**{label}:** {}", names.join(", ")));
            }
        }

        if !occurred
            && let Some(source) = event.get("source").and_then(Value::as_str)
            && let Some(created) = created_day(source)
        {
            lines.push(format!("**Created on:** {created}"));
        }

        lines.push(String::new());

        if let Some(summary) = truthy_display(event, "summary") {
            lines.push(summary);
            lines.push(String::new());
        }
        if let Some(details) = truthy_display(event, "details") {
            lines.push(details);
            lines.push(String::new());
        }

        chunks.push(recorded_chunk(
            lines.join("\n"),
            event_timestamp(rel, start_time),
            event,
        ));
    }
    ProducedChunks {
        chunks,
        agent_override: Some("event".to_string()),
        header: Some(event_header(rel)),
        error: None,
        warnings: Vec::new(),
    }
}

fn event_header(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    let facet = parts
        .windows(3)
        .find_map(|parts| (parts[0] == "facets" && parts[2] == "events").then_some(parts[1]))
        .unwrap_or("unknown");
    let day = rel
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".jsonl"));
    match day.filter(|value| value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())) {
        Some(day) => format!(
            "# Events for '{facet}' facet on {}-{}-{}",
            &day[..4],
            &day[4..6],
            &day[6..]
        ),
        None => format!("# Events for '{facet}' facet"),
    }
}

fn event_timestamp(rel: &str, start: &str) -> i64 {
    let Some(day) = rel
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".jsonl"))
    else {
        return 0;
    };
    let Ok(day) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return 0;
    };
    let time = NaiveTime::parse_from_str(start, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(start, "%H:%M"))
        .unwrap_or(NaiveTime::MIN);
    day.and_time(time).and_utc().timestamp_millis()
}

fn first_five(value: &str) -> String {
    value.chars().take(5).collect()
}

fn created_day(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    if bytes.len() < 9 || bytes[8] != b'/' || !bytes[..8].iter().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(format!(
        "{}-{}-{}",
        &source[..4],
        &source[4..6],
        &source[6..8]
    ))
}
