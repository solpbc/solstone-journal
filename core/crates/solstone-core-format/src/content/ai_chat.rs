// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::NaiveDate;
use serde_json::Value;

use super::{JsonObject, ProducedChunks, recorded_chunk};
use crate::segment::segment_parse;

pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    if records.is_empty() {
        return ProducedChunks {
            chunks: Vec::new(),
            agent_override: None,
            header: None,
            warnings: Vec::new(),
        };
    }

    let source_key = rel
        .split('/')
        .find_map(|part| part.strip_prefix("import."))
        .map(str::to_lowercase)
        .unwrap_or_else(|| "ai_chat".to_string());
    let metadata = records
        .first()
        .filter(|record| !record.contains_key("start"));
    let mut chunks = Vec::new();

    for record in records {
        if !record.contains_key("start") {
            continue;
        }
        let speaker = record.get("speaker").and_then(Value::as_str).unwrap_or("");
        let text = record.get("text").and_then(Value::as_str).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        chunks.push(recorded_chunk(
            format!("**{speaker}:** {text}"),
            ai_chat_timestamp(rel, record),
            record,
        ));
    }

    ProducedChunks {
        chunks,
        agent_override: Some(format!("import.{source_key}")),
        header: Some(ai_chat_header(&source_key, metadata)),
        warnings: Vec::new(),
    }
}

fn ai_chat_header(source_key: &str, metadata: Option<&JsonObject>) -> String {
    let source = match source_key {
        "chatgpt" => "ChatGPT".to_string(),
        "claude" => "Claude".to_string(),
        "gemini" => "Gemini".to_string(),
        "ai_chat" => "AI chat".to_string(),
        _ => capitalize_source(source_key),
    };
    let mut lines = vec![format!("# {source} conversation")];
    if let Some(model) = metadata
        .and_then(|metadata| metadata.get("model"))
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
    {
        lines.push(format!("Model: {model}"));
    }
    if let Some(facet) = metadata
        .and_then(|metadata| metadata.get("imported"))
        .and_then(Value::as_object)
        .and_then(|imported| imported.get("facet"))
        .and_then(Value::as_str)
        .filter(|facet| !facet.is_empty())
    {
        lines.push(format!("Facet: {facet}"));
    }
    lines.join("\n")
}

fn capitalize_source(source: &str) -> String {
    let mut chars = source.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn ai_chat_timestamp(rel: &str, record: &JsonObject) -> i64 {
    let mut timestamp = ai_chat_base_timestamp(rel);
    if let Some(start) = record.get("start").and_then(Value::as_str) {
        let parts: Vec<_> = start.split(':').collect();
        if let [hours, minutes, seconds] = parts.as_slice()
            && let (Ok(hours), Ok(minutes), Ok(seconds)) = (
                hours.parse::<i64>(),
                minutes.parse::<i64>(),
                seconds.parse::<i64>(),
            )
        {
            timestamp += (hours * 3600 + minutes * 60 + seconds) * 1000;
        }
    }
    timestamp
}

fn ai_chat_base_timestamp(rel: &str) -> i64 {
    let Some(day) = rel
        .split('/')
        .find(|part| part.len() == 8 && part.bytes().all(|byte| byte.is_ascii_digit()))
    else {
        return 0;
    };
    let Some(times) = segment_parse(rel) else {
        return 0;
    };
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .ok()
        .and_then(|day| {
            day.and_hms_opt(times.hour as u32, times.minute as u32, times.second as u32)
        })
        .map(|time| time.and_utc().timestamp_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_jsonl_objects;

    #[test]
    fn renders_only_start_bearing_non_empty_turns() {
        let records = parse_jsonl_objects(
            r#"{"model":"claude-3"}
{"start":"00:00:01","speaker":"User","text":"Hello"}
{"start":"00:00:02","speaker":"Assistant","text":""}
{"speaker":"Narrator","text":"metadata-like"}
"#,
        );
        let produced = render(
            "20260101/import.claude/thread_a/conversation_transcript.jsonl",
            &records,
        );

        assert_eq!(produced.agent_override.as_deref(), Some("import.claude"));
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(produced.chunks[0].content, "**User:** Hello");
    }
}
