// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{JsonObject, ProducedChunks, display_value, json_falsy, recorded_chunk};

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();

    for record in records {
        let kind = clean_text(record.get("t"));
        let markdown = match kind.as_str() {
            "segment_start" => format_snapshot(record),
            "delta" => format_delta(record),
            _ => String::new(),
        };
        if !markdown.is_empty() {
            let occurrence = record.get("ts").and_then(Value::as_i64).unwrap_or(0);
            chunks.push(recorded_chunk(markdown, occurrence, record));
        }
    }

    ProducedChunks {
        chunks,
        agent_override: Some("browser".to_string()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

fn format_snapshot(row: &JsonObject) -> String {
    let title = clean_text(row.get("title"));
    let site = clean_text(row.get("site"));
    let url = clean_text(row.get("url"));
    let heading =
        first_non_empty(&[title.as_str(), site.as_str(), url.as_str()]).unwrap_or("Browser Page");
    let mut lines = vec![format!("## {heading}")];

    let adapter = clean_text(row.get("adapter"));
    let subline = [adapter.as_str(), site.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if !subline.is_empty() {
        lines.push(String::new());
        lines.push(subline);
    }

    let block_lines = format_blocks(row.get("blocks"));
    if !block_lines.is_empty() {
        lines.push(String::new());
        lines.extend(block_lines);
    }
    lines.join("\n").trim().to_string()
}

fn format_blocks(blocks: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(blocks)) = blocks else {
        return Vec::new();
    };

    let mut lines = Vec::new();
    for block in blocks {
        let Value::Object(block) = block else {
            continue;
        };
        let text = clean_text(block.get("text"));
        if text.is_empty() {
            continue;
        }
        if block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            == "heading"
        {
            lines.push(format!("### {text}"));
        } else {
            lines.push(text);
        }
    }
    lines
}

fn format_delta(row: &JsonObject) -> String {
    match clean_text(row.get("op")).as_str() {
        "add" | "update" => {}
        _ => return String::new(),
    }
    let Some(Value::Object(block)) = row.get("block") else {
        return String::new();
    };
    clean_text(block.get("text"))
}

fn first_non_empty<'a>(values: &[&'a str]) -> Option<&'a str> {
    values.iter().copied().find(|value| !value.is_empty())
}

fn clean_text(value: Option<&Value>) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_jsonl_objects;

    #[test]
    fn renders_snapshots_and_deltas() {
        let records = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"site":"mail.google.com","url":"https://mail.google.com/mail/u/0/#inbox","title":"Inbox - Gmail","adapter":"gmail","blocks":[{"type":"heading","text":"Inbox"},{"type":"row","text":"Ari Patel - Browser stream contract review"},{"type":"row","text":"   "},{"type":"link","text":"Open pull request"}]}
{"t":"delta","ts":2,"op":"add","block":{"type":"row","text":"New: Casey Morgan - Lunch moved"}}
{"t":"delta","ts":3,"op":"update","block":{"type":"row","text":"Ari Patel - Browser stream contract review (2 replies)"}}
{"t":"delta","ts":4,"op":"remove","block":{"type":"row","text":"Promotions tab collapsed"}}
{"t":"ignored","ts":5,"text":"skip me"}
"#,
        );
        let produced = render(&records);
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();

        assert_eq!(produced.agent_override.as_deref(), Some("browser"));
        assert_eq!(
            contents,
            vec![
                "## Inbox - Gmail\n\ngmail · mail.google.com\n\n### Inbox\nAri Patel - Browser stream contract review\nOpen pull request",
                "New: Casey Morgan - Lunch moved",
                "Ari Patel - Browser stream contract review (2 replies)",
            ]
        );
    }

    #[test]
    fn snapshot_uses_url_or_default_heading_and_delta_skip_rules() {
        let records = parse_jsonl_objects(
            r#"{"t":"segment_start","ts":1,"url":"https://example.com/fallback","blocks":[{"type":"text","text":"Fallback page text"}]}
{"t":"segment_start","ts":2,"blocks":[]}
{"t":"delta","ts":3,"op":"add","block":{"type":"row","text":"   "}}
{"t":"delta","ts":4,"op":"noop","block":{"type":"row","text":"skip"}}
{"t":"delta","ts":5,"op":"update","block":"not an object"}
"#,
        );
        let produced = render(&records);
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();

        assert_eq!(
            contents,
            vec![
                "## https://example.com/fallback\n\nFallback page text",
                "## Browser Page",
            ]
        );
    }
}
