// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;
use serde_json::Value;

use super::{
    JsonObject, ProducedChunks, capitalize, display_value, recorded_chunk, truthy_display,
};
use crate::segment::{is_date_key, segment_parse};

const HEADER_SKIP_FIELDS: &[&str] = &[
    "error",
    "raw",
    "imported",
    "_solstone_processing",
    "sound_tags",
];

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    let metadata = records
        .first()
        .filter(|record| !record.contains_key("start"));
    let mut skipped = 0usize;
    let mut chunks = Vec::new();
    let (base_timestamp, start_header) = base_timestamp_and_header(rel);

    for (index, record) in records.iter().enumerate() {
        if index == 0 && metadata.is_some() {
            continue;
        }
        if !record.contains_key("start") {
            skipped += 1;
            continue;
        }
        let start = record.get("start").map(display_value).unwrap_or_default();
        let timestamp = parse_offset(&start)
            .map(|offset| base_timestamp + offset)
            .unwrap_or(base_timestamp);
        let mut parts = Vec::new();
        if !start.is_empty() {
            parts.push(format!("[{start}]"));
        }
        let source = record.get("source").map(display_value).unwrap_or_default();
        if !source.is_empty() {
            parts.push(format!("({source})"));
        }
        match record.get("speaker") {
            Some(Value::Number(value)) => parts.push(format!("Speaker {value}:")),
            Some(value) => parts.push(format!("{}:", display_value(value))),
            None => parts.push(String::new()),
        }

        let text = truthy_display(record, "corrected")
            .or_else(|| truthy_display(record, "text"))
            .unwrap_or_default();
        let prefix = parts.join(" ").trim().to_string();
        let mut markdown = if !prefix.is_empty() {
            if text.is_empty() {
                prefix
            } else {
                format!("{prefix} {text}")
            }
        } else if !text.is_empty() {
            text
        } else {
            continue;
        };
        let emotion = truthy_display(record, "emotion").unwrap_or_default();
        if !emotion.is_empty() && !emotion.eq_ignore_ascii_case("neutral") {
            markdown.push_str(&format!(" *({emotion})*"));
        }
        chunks.push(recorded_chunk(markdown, timestamp, record));
    }

    let header = audio_header(metadata, start_header);
    ProducedChunks {
        chunks,
        agent_override: Some("audio".to_string()),
        header,
        error: (skipped > 0)
            .then(|| format!("Skipped {skipped} entries missing 'start' field in {rel}")),
        warnings: Vec::new(),
    }
}

fn audio_header(metadata: Option<&JsonObject>, start_header: Option<String>) -> Option<String> {
    let mut parts = start_header.into_iter().collect::<Vec<_>>();
    if let Some(metadata) = metadata {
        for (key, value) in metadata {
            if HEADER_SKIP_FIELDS.contains(&key.as_str()) {
                continue;
            }
            let value = header_value(value);
            if !value.is_empty() {
                parts.push(format!("{}: {value}", capitalize(key)));
            }
        }
        if let Some(imported) = metadata.get("imported").and_then(Value::as_object) {
            if let Some(facet) = imported.get("facet").map(display_value)
                && !facet.is_empty()
            {
                parts.push(format!("Facet: {facet}"));
            }
            if let Some(id) = imported.get("id").map(display_value)
                && !id.is_empty()
            {
                parts.push(format!("Import ID: {id}"));
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn base_timestamp_and_header(rel: &str) -> (i64, Option<String>) {
    let Some(day) = rel.split('/').find(|part| is_date_key(part)) else {
        return (0, None);
    };
    let Some(times) = segment_parse(rel) else {
        return (0, None);
    };
    let Some(day) = NaiveDate::parse_from_str(day, "%Y%m%d").ok() else {
        return (0, None);
    };
    let Some(start) = day.and_hms_opt(times.hour.into(), times.minute.into(), times.second.into())
    else {
        return (0, None);
    };
    (
        start.and_utc().timestamp_millis(),
        Some(start.format("Start: %Y-%m-%d %I:%M%P").to_string()),
    )
}

fn parse_offset(value: &str) -> Option<i64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<i64>().ok()?;
    let minutes = parts.next()?.parse::<i64>().ok()?;
    let seconds = parts.next()?.parse::<i64>().ok()?;
    parts
        .next()
        .is_none()
        .then_some((hours * 3600 + minutes * 60 + seconds) * 1000)
}

fn header_value(value: &Value) -> String {
    match value {
        Value::Array(values) => values
            .iter()
            .map(display_value)
            .collect::<Vec<_>>()
            .join(", "),
        _ => display_value(value),
    }
}
