// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{JsonObject, ProducedChunks, clean_value, recorded_chunk};

const BRIEFING_ABSENT_TEXT: &str = "Nothing to report.";
const SECTION_KEYS: [(&str, &str); 2] = [
    ("yesterday", "Yesterday"),
    ("needs_attention", "Needs Attention"),
];

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let chunks = records
        .first()
        .and_then(|briefing| {
            let content = render_briefing(briefing);
            (!content.is_empty()).then(|| recorded_chunk(content, 0, briefing))
        })
        .into_iter()
        .collect();

    ProducedChunks {
        chunks,
        agent_override: Some("morning_briefing".to_string()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

fn render_briefing(briefing: &JsonObject) -> String {
    let mut lines = Vec::new();
    let preamble = briefing
        .get("metadata")
        .and_then(Value::as_object)
        .map(|metadata| clean_value(metadata.get("coverage_preamble")))
        .unwrap_or_default();
    if !preamble.is_empty() {
        lines.extend(preamble.lines().map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        }));
    }

    for (key, heading) in SECTION_KEYS {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("## {heading}"));
        lines.push(String::new());
        let body = section_body(briefing, key);
        if body.is_empty() {
            lines.push(BRIEFING_ABSENT_TEXT.to_string());
        } else {
            lines.push(body);
        }
    }

    lines.join("\n").trim().to_string()
}

fn section_body(briefing: &JsonObject, key: &str) -> String {
    match key {
        "yesterday" => string_items(briefing.get("yesterday")),
        "needs_attention" => needs_attention(briefing),
        _ => String::new(),
    }
}

fn string_items(value: Option<&Value>) -> String {
    let Some(Value::Array(items)) = value else {
        return String::new();
    };
    items
        .iter()
        .map(|item| clean_value(Some(item)))
        .filter(|text| !text.is_empty())
        .map(|text| format!("- {text}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn needs_attention(briefing: &JsonObject) -> String {
    let mut lines = Vec::new();
    let Some(Value::Array(items)) = briefing.get("needs_attention") else {
        return String::new();
    };

    for item in items {
        let Value::Object(item) = item else {
            continue;
        };
        let text = clean_value(item.get("text"));
        if !text.is_empty() {
            lines.push(format!("- {text}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_json_object;

    fn render_one(text: &str) -> ProducedChunks {
        render(&parse_json_object(text))
    }

    #[test]
    fn renders_sections_in_order_and_agent_override() {
        let produced = render_one(
            r#"{"metadata":{"coverage_preamble":"Built from test sources. No gaps."},"yesterday":["Shipped the formatter."],"needs_attention":[{"text":"Review the report.","source_id":"sol://20260327/default/090000_300"}]}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("morning_briefing"));
        assert_eq!(produced.chunks.len(), 1);
        let rendered = &produced.chunks[0].content;
        let headings = ["Yesterday", "Needs Attention"];
        let positions: Vec<_> = headings
            .iter()
            .map(|heading| rendered.find(&format!("## {heading}")).expect("heading"))
            .collect();
        assert!(positions.windows(2).all(|window| window[0] < window[1]));
        assert!(rendered.contains("> Built from test sources. No gaps."));
        assert!(rendered.contains("- Shipped the formatter."));
        assert!(rendered.contains("- Review the report."));
    }

    #[test]
    fn stored_forecast_sections_are_not_rendered() {
        let produced = render_one(
            r#"{"your_day":[{"start":"09:00","end":"09:30","text":"Meet Sarah."}],"yesterday":["Shipped the formatter."],"needs_attention":[],"forward_look":["Prepare for Monday."]}"#,
        );
        let rendered = &produced.chunks[0].content;
        assert!(rendered.contains("- Shipped the formatter."));
        assert_eq!(rendered.matches("## ").count(), 2);
        for stored in ["Meet Sarah.", "09:00", "Prepare for Monday."] {
            assert!(!rendered.contains(stored), "{stored} must not render");
        }
    }

    #[test]
    fn quotes_preamble_blank_lines_with_bare_blockquote_marker() {
        let produced = render_one(
            r#"{"metadata":{"coverage_preamble":"Line one.\n\nLine three."},"yesterday":[],"needs_attention":[]}"#,
        );
        let rendered = &produced.chunks[0].content;
        assert!(rendered.starts_with("> Line one.\n>\n> Line three.\n\n## Yesterday"));
    }

    #[test]
    fn empty_object_renders_all_sections_with_absent_text() {
        let produced = render_one("{}");
        assert_eq!(produced.agent_override.as_deref(), Some("morning_briefing"));
        assert_eq!(produced.chunks.len(), 1);
        let rendered = &produced.chunks[0].content;
        assert_eq!(rendered.matches("## ").count(), 2);
        assert_eq!(rendered.matches(BRIEFING_ABSENT_TEXT).count(), 2);
    }

    #[test]
    fn reading_heading_is_not_rendered_when_key_is_present_or_absent() {
        let with_reading = render_one(
            r#"{"reading":[{"facet":"work","summary":"Newsletter summary."},{"facet":"personal"},{"summary":"Loose item."}]}"#,
        );
        assert!(!with_reading.chunks[0].content.contains("Reading"));
        let without_reading = render_one("{}");
        assert!(!without_reading.chunks[0].content.contains("Reading"));
    }

    #[test]
    fn missing_or_non_object_record_keeps_agent_and_zero_chunks() {
        let produced = render(&[]);
        assert_eq!(produced.agent_override.as_deref(), Some("morning_briefing"));
        assert!(produced.chunks.is_empty());

        let produced = render_one("null");
        assert_eq!(produced.agent_override.as_deref(), Some("morning_briefing"));
        assert!(produced.chunks.is_empty());
    }
}
