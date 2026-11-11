// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Granola supplies the speech and its attribution. Note context is a separate
//! chunk, never a spoken turn or a claim that a microphone identified the owner.

use super::{JsonObject, ProducedChunks, recorded_chunk};
use serde_json::Value;

/// The source snapshot header shared by its writer and readiness/read paths.
pub const HEADER_KEY: &str = "granola";
/// One stable meeting stream per remote note.
pub const STREAM_PREFIX: &str = "import.granola.";

pub(super) fn render(metadata: &JsonObject, records: &[JsonObject]) -> ProducedChunks {
    let note = &metadata[HEADER_KEY];
    let mut context = Vec::new();
    for (key, label) in [
        ("title", "title"),
        ("summary_markdown", "summary from Granola"),
        ("private_notes_markdown", "private notes from Granola"),
    ] {
        let value = note
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| match key {
                "summary_markdown" => note.get("summary_text").and_then(Value::as_str),
                "private_notes_markdown" => note.get("private_notes_text").and_then(Value::as_str),
                _ => None,
            });
        if let Some(value) = value.filter(|s| !s.is_empty()) {
            context.push(format!("{label}:\n{value}"));
        }
    }
    if let Some(calendar) = note.get("calendar_event")
        && let Some(title) = calendar.get("event_title").and_then(Value::as_str)
    {
        context.push(format!("calendar title: {title}"));
    }
    if let Some(attendees) = note.get("attendees").and_then(Value::as_array) {
        let names: Vec<_> = attendees
            .iter()
            .filter_map(|a| {
                let name = a.get("name").and_then(Value::as_str).unwrap_or("");
                let email = a.get("email").and_then(Value::as_str).unwrap_or("");
                match (name.is_empty(), email.is_empty()) {
                    (false, false) => Some(format!("{name} <{email}>")),
                    (false, true) => Some(name.to_owned()),
                    (true, false) => Some(email.to_owned()),
                    _ => None,
                }
            })
            .collect();
        if !names.is_empty() {
            context.push(format!(
                "attendees reported by Granola: {}",
                names.join(", ")
            ));
        }
    }
    let mut chunks = Vec::new();
    if !context.is_empty() {
        chunks.push(recorded_chunk(
            format!(
                "Granola meeting notes (not spoken):\n\n{}",
                context.join("\n\n")
            ),
            metadata
                .get("granola_start_ms")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            metadata,
        ));
    }
    for record in records {
        let Some(at) = record.get("source_ms").and_then(Value::as_i64) else {
            continue;
        };
        let text = record.get("text").and_then(Value::as_str).unwrap_or("");
        let label = record.get("speaker").and_then(Value::as_str).unwrap_or("");
        let time = record.get("start").and_then(Value::as_str).unwrap_or("");
        chunks.push(recorded_chunk(
            format!("[{time}] {label}: {text}"),
            at,
            record,
        ));
    }
    ProducedChunks {
        chunks,
        agent_override: Some("granola".into()),
        header: Some("# Granola meeting".into()),
        error: None,
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{OccurrenceTimeMs, parse_jsonl_objects};
    #[test]
    fn context_is_not_speech_and_absolute_instants_survive_the_reader() {
        let records = parse_jsonl_objects(
            r#"{"granola":{"title":"Planning","summary_text":"Agreed","private_notes_text":"Ask later","attendees":[{"name":"Robin","email":"r@example.org"}],"calendar_event":{"event_title":"Weekly"}},"granola_start_ms":1793523480000}
{"start":"01:58:00","source_ms":1793523480000,"speaker":"Granola: me","text":"First"}
{"start":"01:01:00","source_ms":1793523660000,"speaker":"Granola: Speaker A","text":"Second"}"#,
        );
        let produced = super::super::audio::render(
            "20261101/import.granola.not_abcdefghijk123/015800_3600/audio.jsonl",
            &records,
        );
        assert_eq!(produced.chunks.len(), 3);
        for value in [
            "Planning",
            "Agreed",
            "Ask later",
            "Robin",
            "r@example.org",
            "Weekly",
        ] {
            assert!(produced.chunks[0].content.contains(value));
        }
        assert!(produced.chunks[0].content.contains("not spoken"));
        assert_eq!(
            produced.chunks[1].occurrence_time_ms,
            Some(OccurrenceTimeMs(1793523480000))
        );
        assert_eq!(
            produced.chunks[2].occurrence_time_ms,
            Some(OccurrenceTimeMs(1793523660000))
        );
        assert!(produced.chunks[1].content.contains("Granola: me"));
        assert!(!produced.chunks[1].content.contains("You"));
        let empty = render(&records[0], &[]);
        assert_eq!(empty.chunks.len(), 1);
    }
}
