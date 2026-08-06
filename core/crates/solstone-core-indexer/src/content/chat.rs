// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{ChatLabels, JsonObject, ProducedChunks, display_value, json_falsy, recorded_chunk};

pub(super) fn render(records: &[JsonObject], labels: &ChatLabels) -> ProducedChunks {
    let mut chunks = Vec::new();

    for record in records {
        let kind = record
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let markdown = match kind {
            "owner_message" => Some(speaker_line(&labels.owner, record.get("text"))),
            "sol_message" => Some(speaker_line(&labels.agent, record.get("text"))),
            "talent_spawned" => Some(format!(
                "*[{} spawned: {}]*",
                field(record, "name"),
                field(record, "task")
            )),
            "talent_finished" => Some(format!(
                "*[{} finished: {}]*",
                field(record, "name"),
                field(record, "summary")
            )),
            "talent_errored" => Some(format!(
                "*[{} errored: {}]*",
                field(record, "name"),
                field(record, "reason")
            )),
            "chat_error" => Some(format!("*[chat trouble: {}]*", field(record, "reason"))),
            "sol_chat_request" => Some(sol_chat_request(record)),
            "chat_queue_depth"
            | "support_draft"
            | "support_submit_claim"
            | "result"
            | "sol_chat_request_superseded"
            | "owner_chat_open"
            | "owner_chat_dismissed" => None,
            // Python raises ValueError here, but native must keep indexing infallible:
            // Python catches that upstream and still writes files(path, mtime).
            _ => None,
        };
        if let Some(content) = markdown
            && !content.is_empty()
        {
            let occurrence = record.get("ts").and_then(Value::as_i64).unwrap_or(0);
            chunks.push(recorded_chunk(content, occurrence, record));
        }
    }

    ProducedChunks {
        chunks,
        agent_override: Some("chat".to_string()),
        header: None,
        warnings: Vec::new(),
    }
}

fn sol_chat_request(record: &JsonObject) -> String {
    let mut text = format!("[sol] {}", falsy_text(record.get("summary")))
        .trim()
        .to_string();
    let message = falsy_text(record.get("message"));
    if !message.is_empty() {
        text.push('\n');
        text.push_str(&message);
    }
    text
}

fn speaker_line(label: &str, body: Option<&Value>) -> String {
    let text = falsy_text(body);
    if text.is_empty() {
        format!("**{label}**")
    } else {
        format!("**{label}** {text}")
    }
}

fn falsy_text(value: Option<&Value>) -> String {
    if json_falsy(value) {
        String::new()
    } else {
        value
            .map(display_value)
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

fn field(record: &JsonObject, key: &str) -> String {
    record.get(key).map(display_value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_jsonl_objects;

    #[test]
    fn renders_chat_content_and_skips_non_content_kinds() {
        let records = parse_jsonl_objects(
            r#"{"kind":"owner_message","ts":1,"text":"Need a diff"}
{"kind":"owner_message","ts":2,"text":"   "}
{"kind":"sol_message","ts":3,"text":"I can do that"}
{"kind":"talent_spawned","ts":4,"name":"exec","task":"compare drafts"}
{"kind":"talent_finished","ts":5,"name":"exec","summary":"summarized the differences"}
{"kind":"talent_errored","ts":6,"name":"exec","reason":"repo unavailable"}
{"kind":"chat_error","ts":7,"reason":"unknown"}
{"kind":"sol_chat_request","ts":8,"summary":"unique solar cue","message":"extended detail"}
{"kind":"owner_chat_open","ts":9,"surface":"convey"}
{"kind":"mystery","ts":10,"text":"skip me"}
"#,
        );
        let produced = render(&records, &ChatLabels::default());
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();

        assert_eq!(produced.agent_override.as_deref(), Some("chat"));
        assert_eq!(
            contents,
            vec![
                "**Owner** Need a diff",
                "**Owner**",
                "**Sol** I can do that",
                "*[exec spawned: compare drafts]*",
                "*[exec finished: summarized the differences]*",
                "*[exec errored: repo unavailable]*",
                "*[chat trouble: unknown]*",
                "[sol] unique solar cue\nextended detail",
            ]
        );
    }

    #[test]
    fn falsy_chat_fields_match_python_coercion_boundaries() {
        let records = parse_jsonl_objects(
            r#"{"kind":"owner_message","text":0}
{"kind":"sol_message","text":null}
{"kind":"sol_chat_request","summary":0,"message":"body text"}
{"kind":"talent_spawned","name":0,"task":"x"}
"#,
        );
        let produced = render(&records, &ChatLabels::default());
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();

        assert_eq!(
            contents,
            vec![
                "**Owner**",
                "**Sol**",
                "[sol]\nbody text",
                "*[0 spawned: x]*",
            ]
        );
    }

    #[test]
    fn missing_chat_fields_render_as_empty_strings() {
        let records = parse_jsonl_objects(
            r#"{"kind":"talent_spawned","name":"exec"}
{"kind":"chat_error"}
{"kind":"sol_chat_request","message":"  detail only  "}
"#,
        );
        let produced = render(&records, &ChatLabels::default());
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();

        assert_eq!(
            contents,
            vec![
                "*[exec spawned: ]*",
                "*[chat trouble: ]*",
                "[sol]\ndetail only",
            ]
        );
    }
}
