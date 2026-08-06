// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{JsonObject, ProducedChunks, clean_value, recorded_chunk};

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let chunks = records
        .first()
        .and_then(|record| {
            let content = render_record(record);
            (!content.is_empty()).then(|| recorded_chunk(content, 0, record))
        })
        .into_iter()
        .collect();

    ProducedChunks {
        chunks,
        agent_override: Some("screen".to_string()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

fn render_record(record: &JsonObject) -> String {
    let mut lines = Vec::new();
    let narrative = clean_value(record.get("narrative"));
    if !narrative.is_empty() {
        lines.push(narrative);
    }

    let entity_lines = entities(record);
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push("## Entities".to_string());
    lines.push(String::new());
    if entity_lines.is_empty() {
        lines.push("Not specified".to_string());
    } else {
        lines.extend(entity_lines);
    }

    lines.join("\n").trim().to_string()
}

fn entities(record: &JsonObject) -> Vec<String> {
    let mut lines = Vec::new();
    let Some(Value::Array(entities)) = record.get("entities") else {
        return lines;
    };

    for entity in entities {
        let Value::Object(entity) = entity else {
            continue;
        };
        let entity_type = clean_value(entity.get("type"));
        let name = clean_value(entity.get("name"));
        let role = clean_value(entity.get("role"));
        let context_text = clean_value(entity.get("context"));
        if [
            entity_type.as_str(),
            name.as_str(),
            role.as_str(),
            context_text.as_str(),
        ]
        .iter()
        .all(|part| part.is_empty())
        {
            continue;
        }

        let mut label = if entity_type.is_empty() {
            name
        } else {
            format!("{entity_type}: {name}")
        };
        if label.is_empty() {
            label = "Entity".to_string();
        }
        if !role.is_empty() {
            label.push_str(&format!(" ({role})"));
        }
        if !context_text.is_empty() {
            label.push_str(&format!(" - {context_text}"));
        }
        lines.push(format!("- {label}"));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_json_object;

    fn render_one(text: &str) -> ProducedChunks {
        render(&parse_json_object(text))
    }

    #[test]
    fn renders_narrative_entities_and_agent_override() {
        let produced = render_one(
            r#"{"narrative":"09:00 Alice Smith discussed the solstone repository.","entities":[{"type":"Person","name":"Alice Smith","role":"attendee","context":"Visible in the meeting participant tile."},{"type":"FilePath","name":"solstone/think/cluster.py","role":"mentioned","context":"Open in the editor."}]}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("screen"));
        assert_eq!(produced.chunks.len(), 1);
        let rendered = &produced.chunks[0].content;
        assert!(rendered.contains("09:00 Alice Smith discussed the solstone repository."));
        assert!(rendered.contains("## Entities"));
        assert!(rendered.contains(
            "- Person: Alice Smith (attendee) - Visible in the meeting participant tile."
        ));
        assert!(
            rendered.contains(
                "- FilePath: solstone/think/cluster.py (mentioned) - Open in the editor."
            )
        );
    }

    #[test]
    fn empty_object_renders_entities_skeleton() {
        let produced = render_one("{}");
        assert_eq!(produced.agent_override.as_deref(), Some("screen"));
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(produced.chunks[0].content, "## Entities\n\nNot specified");
    }

    #[test]
    fn skips_empty_entities_and_uses_entity_fallback_label() {
        let produced = render_one(
            r#"{"entities":[{},[],{"role":"mentioned"},{"context":"Visible but unnamed."}]}"#,
        );
        let rendered = &produced.chunks[0].content;
        assert!(rendered.contains("- Entity (mentioned)"));
        assert!(rendered.contains("- Entity - Visible but unnamed."));
        assert_eq!(rendered.matches("- Entity").count(), 2);
    }

    #[test]
    fn missing_or_non_object_record_keeps_agent_and_zero_chunks() {
        let produced = render(&[]);
        assert_eq!(produced.agent_override.as_deref(), Some("screen"));
        assert!(produced.chunks.is_empty());

        let produced = render_one("42");
        assert_eq!(produced.agent_override.as_deref(), Some("screen"));
        assert!(produced.chunks.is_empty());
    }
}
