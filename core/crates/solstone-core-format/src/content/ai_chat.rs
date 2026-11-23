// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jiff::tz::Offset;
use serde_json::Value;
use solstone_core_window_catalog::{ENGLISH, strings, text};

use super::{JsonObject, ProducedChunks, recorded_chunk_for_locale};
use crate::segment::segment_parse;

#[allow(dead_code)]
pub(super) fn render(rel: &str, records: &[JsonObject]) -> ProducedChunks {
    render_for_locale(rel, records, ENGLISH)
}

pub(super) fn render_for_locale(rel: &str, records: &[JsonObject], locale: &str) -> ProducedChunks {
    if records.is_empty() {
        return ProducedChunks {
            chunks: Vec::new(),
            agent_override: None,
            header: None,
            error: None,
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
        let Some(content) = indexed_turn_text_for_locale(record, locale) else {
            continue;
        };
        chunks.push(recorded_chunk_for_locale(
            content,
            ai_chat_timestamp(rel, record),
            record,
            locale,
        ));
    }

    ProducedChunks {
        chunks,
        agent_override: Some(format!("import.{source_key}")),
        header: Some(ai_chat_header(&source_key, metadata)),
        error: None,
        warnings: Vec::new(),
    }
}

pub(super) fn indexed_turn_text(record: &JsonObject) -> Option<String> {
    indexed_turn_text_for_locale(record, ENGLISH)
}

fn localized_speaker(speaker: &str, locale: &str) -> String {
    if locale == ENGLISH {
        return speaker.to_owned();
    }
    let id = match speaker {
        "You" => "journal.window.content.chat.speaker.you",
        "Assistant" => "journal.window.content.chat.speaker.assistant",
        "Human" => "journal.window.content.chat.speaker.human",
        "Unknown" => "journal.window.content.chat.speaker.unknown",
        _ => return speaker.to_owned(),
    };
    text(locale, id)
}

fn indexed_turn_text_for_locale(record: &JsonObject, locale: &str) -> Option<String> {
    if !record.contains_key("start") {
        return None;
    }
    let body = record
        .get("text")?
        .as_str()
        .filter(|text| !text.is_empty())?;
    match record
        .get("speaker")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(speaker) => {
            let speaker = localized_speaker(speaker, locale);
            Some(strings(
                locale,
                "journal.window.content.chat.turn",
                [("speaker", speaker.as_str()), ("text", body)],
            ))
        }
        None => Some(body.to_owned()),
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

/// Turn timestamp in Unix milliseconds. Explicit `source_ms` wins; otherwise
/// falls back to local day and segment start time treated as UTC, plus elapsed `start`.
fn ai_chat_timestamp(rel: &str, record: &JsonObject) -> i64 {
    if let Some(millis) = record.get("source_ms").and_then(Value::as_i64) {
        return millis;
    }
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
    solstone_core_time::parse_day(day)
        .and_then(|day| {
            Offset::UTC
                .to_timestamp(day.to_datetime(times.clock()?))
                .ok()
        })
        .map(|time| time.as_millisecond())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{OccurrenceTimeMs, parse_jsonl_objects};

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

    #[test]
    fn source_ms_wins_over_path_and_start() {
        let records = parse_jsonl_objects(
            r#"{"start":"00:00:00","speaker":"User","text":"Hello","source_ms":1793523480000}
"#,
        );
        let produced = render(
            "20261101/import.chatgpt/015800_301/conversation_transcript.jsonl",
            &records,
        );
        assert_eq!(
            produced.chunks[0].occurrence_time_ms,
            Some(OccurrenceTimeMs(1793523480000))
        );
    }

    #[test]
    fn absent_source_ms_stepped_length_produces_same_timestamp() {
        let records = parse_jsonl_objects(
            r#"{"start":"00:00:00","speaker":"User","text":"Hello"}
"#,
        );
        let produced_300 = render(
            "20261101/import.chatgpt/015800_300/conversation_transcript.jsonl",
            &records,
        );
        let produced_301 = render(
            "20261101/import.chatgpt/015800_301/conversation_transcript.jsonl",
            &records,
        );
        assert_eq!(
            produced_300.chunks[0].occurrence_time_ms,
            produced_301.chunks[0].occurrence_time_ms
        );
    }

    #[test]
    fn absent_source_ms_on_unparseable_path_uses_base_zero_plus_start() {
        let records = parse_jsonl_objects(
            r#"{"start":"00:00:01","speaker":"User","text":"Hello"}
"#,
        );
        let produced = render(
            "20260101/import.claude/thread_a/conversation_transcript.jsonl",
            &records,
        );
        assert_eq!(
            produced.chunks[0].occurrence_time_ms,
            Some(OccurrenceTimeMs(1000))
        );
    }
}
