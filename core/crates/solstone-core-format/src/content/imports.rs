// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::DateTime;
use serde_json::Value;

use super::{
    JsonObject, ProducedChunks, display_or_default, display_value, json_falsy, recorded_chunk,
    truthy_display,
};

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let Some(header) = records.first() else {
        return ProducedChunks {
            chunks: Vec::new(),
            agent_override: None,
            header: None,
            error: None,
            warnings: Vec::new(),
        };
    };
    let source = header
        .get("import")
        .and_then(Value::as_object)
        .and_then(|import| import.get("source"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut chunks = Vec::new();

    for entry in &records[1..] {
        let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or("");
        let markdown = format_entry(entry_type, entry);
        if !markdown.is_empty() {
            let occurrence = entry
                .get("ts")
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.timestamp_millis())
                .unwrap_or(0);
            chunks.push(recorded_chunk(markdown, occurrence, entry));
        }
    }

    ProducedChunks {
        chunks,
        agent_override: Some(format!("import.{source}")),
        header: Some(format!(
            "# Imported from {source} ({} entries)",
            header
                .get("entry_count")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        )),
        error: None,
        warnings: Vec::new(),
    }
}

fn format_entry(entry_type: &str, entry: &JsonObject) -> String {
    match entry_type {
        "calendar_event" => format_calendar_event(entry),
        "note" => format_note(entry),
        "highlight" => format_highlight(entry),
        _ => format_generic(entry),
    }
}

fn format_calendar_event(entry: &JsonObject) -> String {
    let title = display_or_default(entry, "title", "Untitled event");
    let mut lines = vec![format!("## {title}")];

    let mut time_parts = Vec::new();
    if let Some(ts) = entry.get("ts").and_then(Value::as_str)
        && let Some(time) = iso_hour_minute_12h(ts)
    {
        time_parts.push(time);
    }
    if let Some(duration) = truthy_display(entry, "duration_minutes") {
        time_parts.push(format!("{duration} min"));
    }
    if !time_parts.is_empty() {
        lines.push(time_parts.join(" | "));
    }

    if let Some(location) = truthy_display(entry, "location") {
        lines.push(format!("Location: {location}"));
    }

    if let Some(attendees) = display_attendees(entry.get("attendees")) {
        lines.push(format!("Attendees: {attendees}"));
    }

    if let Some(content) = truthy_display(entry, "content") {
        lines.push(String::new());
        lines.push(content);
    }

    lines.join("\n")
}

fn format_note(entry: &JsonObject) -> String {
    let title = display_or_default(entry, "title", "Untitled note");
    let mut lines = vec![format!("## {title}")];

    if let Some(tags) = display_array(entry.get("tags")) {
        lines.push(format!("Tags: {tags}"));
    }

    if let Some(wikilinks) = display_array(entry.get("wikilinks")) {
        lines.push(format!("Links: {wikilinks}"));
    }

    if let Some(content) = truthy_display(entry, "content") {
        lines.push(String::new());
        lines.push(content);
    }

    lines.join("\n")
}

fn format_highlight(entry: &JsonObject) -> String {
    let book = display_or_default(entry, "book_title", "Unknown book");
    let mut header = format!("## {book}");
    if let Some(author) = truthy_display(entry, "author") {
        header.push_str(&format!(" by {author}"));
    }
    let mut lines = vec![header];

    let mut loc_parts = Vec::new();
    if let Some(page) = truthy_display(entry, "page") {
        loc_parts.push(format!("Page {page}"));
    }
    if let Some(location) = truthy_display(entry, "location") {
        loc_parts.push(format!("Location {location}"));
    }
    if !loc_parts.is_empty() {
        lines.push(loc_parts.join(" | "));
    }

    if let Some(content) = truthy_display(entry, "content") {
        lines.push(String::new());
        if entry.get("clip_type").and_then(Value::as_str) == Some("note") {
            lines.push(format!("Note: {content}"));
        } else {
            lines.push(format!("> {content}"));
        }
    }

    lines.join("\n")
}

fn format_generic(entry: &JsonObject) -> String {
    let mut lines = Vec::new();
    if let Some(title) = truthy_display(entry, "title") {
        lines.push(format!("## {title}"));
    }
    if let Some(content) = truthy_display(entry, "content") {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(content);
    }
    lines.join("\n")
}

fn display_array(value: Option<&Value>) -> Option<String> {
    let Value::Array(values) = value? else {
        return None;
    };
    let rendered: Vec<String> = values
        .iter()
        .filter_map(|value| {
            if json_falsy(Some(value)) {
                None
            } else {
                Some(display_value(value))
            }
        })
        .collect();
    if rendered.is_empty() {
        None
    } else {
        Some(rendered.join(", "))
    }
}

fn display_attendees(value: Option<&Value>) -> Option<String> {
    let Value::Array(values) = value? else {
        return None;
    };
    let names: Vec<String> = values
        .iter()
        .filter_map(|value| {
            if let Value::Object(attendee) = value {
                return truthy_display(attendee, "name")
                    .or_else(|| truthy_display(attendee, "email"));
            }
            if json_falsy(Some(value)) {
                None
            } else {
                Some(display_value(value))
            }
        })
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

fn iso_hour_minute_12h(value: &str) -> Option<String> {
    let start = value.find('T')? + 1;
    let time = value.get(start..start + 5)?;
    let bytes = time.as_bytes();
    if bytes.len() != 5
        || !bytes[0].is_ascii_digit()
        || !bytes[1].is_ascii_digit()
        || bytes[2] != b':'
        || !bytes[3].is_ascii_digit()
        || !bytes[4].is_ascii_digit()
    {
        return None;
    }
    let hour = time[0..2].parse::<u32>().ok()?;
    let minute = time[3..5].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    let hour_12 = match hour % 12 {
        0 => 12,
        value => value,
    };
    let meridiem = if hour >= 12 { "PM" } else { "AM" };
    Some(format!("{hour_12:02}:{minute:02} {meridiem}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_jsonl_objects;

    #[test]
    fn formats_calendar_event_fields() {
        let records = parse_jsonl_objects(
            r#"{"import":{"source":"ics"}}
{"type":"calendar_event","title":"Planning","ts":"2026-01-01T09:30:00-07:00","duration_minutes":45,"location":"Room 1","attendees":[{"name":"Alice"},{"email":"bob@example.com"},"Charlie"],"content":"Agenda"}
"#,
        );
        let produced = render(&records);

        assert_eq!(produced.agent_override.as_deref(), Some("import.ics"));
        assert_eq!(
            produced.chunks[0].content,
            "## Planning\n09:30 AM | 45 min\nLocation: Room 1\nAttendees: Alice, bob@example.com, Charlie\n\nAgenda"
        );
    }

    #[test]
    fn formats_note_highlight_and_generic_entries() {
        let records = parse_jsonl_objects(
            r#"{"import":{"source":"obsidian"}}
{"type":"note","title":"Daily","tags":["work","plan"],"wikilinks":["Project"],"content":"Notes"}
{"type":"highlight","book_title":"Book","author":"Writer","page":12,"location":34,"content":"Quote"}
{"title":"Generic","content":"Body"}
{"type":"generic"}
"#,
        );
        let produced = render(&records);

        assert_eq!(produced.chunks.len(), 3);
        assert_eq!(
            produced.chunks[0].content,
            "## Daily\nTags: work, plan\nLinks: Project\n\nNotes"
        );
        assert_eq!(
            produced.chunks[1].content,
            "## Book by Writer\nPage 12 | Location 34\n\n> Quote"
        );
        assert_eq!(produced.chunks[2].content, "## Generic\n\nBody");
    }
}
