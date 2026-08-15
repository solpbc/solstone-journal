// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::DateTime;
use serde_json::Value;

use super::{
    JsonObject, ProducedChunks, display_or_default, display_value, recorded_chunk, titleize,
    truncate_string, truthy_display,
};

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();
    for entry in records {
        let Some(action) = truthy_display(entry, "action") else {
            continue;
        };
        let actor = display_or_default(entry, "actor", "unknown");
        let action_display = titleize(&action);
        let mut lines = vec![format!("### {action_display} by {actor}"), String::new()];

        let source = display_or_default(entry, "source", "unknown");
        let mut meta_parts = vec![format!("**Source:** {source}")];
        if let Some(timestamp) = entry.get("timestamp").and_then(Value::as_str)
            && let Some(time) = iso_time(timestamp)
        {
            meta_parts.push(format!("**Time:** {time}"));
        }
        lines.push(meta_parts.join(" | "));

        if let Some(use_id) = truthy_display(entry, "use_id") {
            lines.push(format!(
                "**Talent:** [{use_id}](/app/thinking/#runs/run/{use_id})"
            ));
        }

        lines.push(String::new());

        if let Some(Value::Object(params)) = entry.get("params")
            && !params.is_empty()
        {
            lines.push("**Parameters:**".to_string());
            for (key, value) in params {
                let rendered = match value {
                    Value::String(value) => truncate_string(value, 100),
                    _ => display_value(value),
                };
                lines.push(format!("- {key}: {rendered}"));
            }
            lines.push(String::new());
        }

        let occurrence = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp_millis())
            .unwrap_or(0);
        chunks.push(recorded_chunk(lines.join("\n"), occurrence, entry));
    }
    ProducedChunks {
        chunks,
        agent_override: Some("action".to_string()),
        header: Some(action_log_header(rel)),
        error: None,
        warnings: Vec::new(),
    }
}

fn action_log_header(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    let journal_level = matches!(parts.as_slice(), ["config", "actions", _]);
    let facet = parts
        .windows(3)
        .find_map(|parts| (parts[0] == "facets" && parts[2] == "logs").then_some(parts[1]));
    let day = rel
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".jsonl"));
    let suffix = day
        .filter(|value| value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .map(|day| format!(" ({}-{}-{})", &day[..4], &day[4..6], &day[6..]))
        .unwrap_or_default();
    if journal_level {
        format!("# Journal Action Log{suffix}")
    } else if let Some(facet) = facet {
        format!("# Action Log: {facet}{suffix}")
    } else {
        format!("# Action Log{suffix}")
    }
}

fn iso_time(timestamp: &str) -> Option<&str> {
    let start = timestamp.find('T')? + 1;
    let end = start + 8;
    let value = timestamp.get(start..end)?;
    let bytes = value.as_bytes();
    if bytes.len() == 8
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b':'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[5] == b':'
        && bytes[6].is_ascii_digit()
        && bytes[7].is_ascii_digit()
    {
        Some(value)
    } else {
        None
    }
}
