// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{JsonObject, ProducedChunks, clean_value, recorded_chunk};

const BRIEFING_ABSENT_TEXT: &str = "Nothing to report.";
const SECTION_KEYS: [(&str, &str); 5] = [
    ("your_day", "Your Day"),
    ("yesterday", "Yesterday"),
    ("needs_attention", "Needs Attention"),
    ("forward_look", "Forward Look"),
    ("reading", "Reading"),
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
        "your_day" => your_day(briefing),
        "yesterday" => string_items(briefing.get("yesterday")),
        "needs_attention" => needs_attention(briefing),
        "forward_look" => string_items(briefing.get("forward_look")),
        "reading" => reading(briefing),
        _ => String::new(),
    }
}

fn your_day(briefing: &JsonObject) -> String {
    let mut lines = Vec::new();
    let Some(Value::Array(items)) = briefing.get("your_day") else {
        return String::new();
    };

    for item in items {
        let Value::Object(item) = item else {
            continue;
        };
        let text = clean_value(item.get("text"));
        if text.is_empty() {
            continue;
        }
        let time = clean_value(item.get("time"));
        if time.is_empty() {
            lines.push(format!("- {text}"));
        } else {
            lines.push(format!("- **{time}** — {text}"));
        }
    }
    lines.join("\n")
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

fn reading(briefing: &JsonObject) -> String {
    let mut lines = Vec::new();
    let Some(Value::Array(items)) = briefing.get("reading") else {
        return String::new();
    };

    for item in items {
        let Value::Object(item) = item else {
            continue;
        };
        let facet = clean_value(item.get("facet"));
        let summary = clean_value(item.get("summary"));
        if !facet.is_empty() && !summary.is_empty() {
            lines.push(format!("- **{facet}** — {summary}"));
        } else if !facet.is_empty() {
            lines.push(format!("- **{facet}**"));
        } else if !summary.is_empty() {
            lines.push(format!("- {summary}"));
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
    fn renders_all_five_sections_and_agent_override() {
        let produced = render_one(
            r#"{"metadata":{"coverage_preamble":"Built from test sources. No gaps."},"your_day":[{"time":"09:00","text":"Meet Sarah."}],"yesterday":["Shipped the formatter."],"needs_attention":[{"text":"Review the report.","source_id":"sol://20260327/default/090000_300"}],"forward_look":["Prepare for Monday."],"reading":[{"facet":"work","summary":"Newsletter summary."}]}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("morning_briefing"));
        assert_eq!(produced.chunks.len(), 1);
        let rendered = &produced.chunks[0].content;
        let headings = [
            "Your Day",
            "Yesterday",
            "Needs Attention",
            "Forward Look",
            "Reading",
        ];
        let positions: Vec<_> = headings
            .iter()
            .map(|heading| rendered.find(&format!("## {heading}")).expect("heading"))
            .collect();
        assert!(positions.windows(2).all(|window| window[0] < window[1]));
        assert!(rendered.contains("> Built from test sources. No gaps."));
        assert!(rendered.contains("- **09:00** — Meet Sarah."));
        assert!(rendered.contains("- Shipped the formatter."));
        assert!(rendered.contains("- Review the report."));
        assert!(rendered.contains("- Prepare for Monday."));
        assert!(rendered.contains("- **work** — Newsletter summary."));
    }

    #[test]
    fn quotes_preamble_blank_lines_with_bare_blockquote_marker() {
        let produced = render_one(
            r#"{"metadata":{"coverage_preamble":"Line one.\n\nLine three."},"your_day":[],"yesterday":[],"needs_attention":[],"forward_look":[],"reading":[]}"#,
        );
        let rendered = &produced.chunks[0].content;
        assert!(rendered.starts_with("> Line one.\n>\n> Line three.\n\n## Your Day"));
    }

    #[test]
    fn empty_object_renders_all_sections_with_absent_text() {
        let produced = render_one("{}");
        assert_eq!(produced.agent_override.as_deref(), Some("morning_briefing"));
        assert_eq!(produced.chunks.len(), 1);
        let rendered = &produced.chunks[0].content;
        assert_eq!(rendered.matches("## ").count(), 5);
        assert_eq!(rendered.matches(BRIEFING_ABSENT_TEXT).count(), 5);
    }

    #[test]
    fn reading_handles_facet_only_summary_only_and_both_variants() {
        let produced = render_one(
            r#"{"reading":[{"facet":"work","summary":"Newsletter summary."},{"facet":"personal"},{"summary":"Loose item."}]}"#,
        );
        let rendered = &produced.chunks[0].content;
        assert!(rendered.contains("- **work** — Newsletter summary."));
        assert!(rendered.contains("- **personal**"));
        assert!(rendered.contains("- Loose item."));
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
