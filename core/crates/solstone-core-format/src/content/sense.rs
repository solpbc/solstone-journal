// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{JsonObject, ProducedChunks, clean_value, display_value, json_truthy, recorded_chunk};

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let mut chunks = Vec::new();
    if let Some(sense_obj) = records.first() {
        let markdown = render_sense(sense_obj);
        if !markdown.is_empty() {
            chunks.push(recorded_chunk(markdown, 0, sense_obj));
        }
    }

    ProducedChunks {
        chunks,
        agent_override: Some("sense".to_string()),
        header: None,
        warnings: Vec::new(),
    }
}

fn render_sense(sense_obj: &JsonObject) -> String {
    let mut lines = Vec::new();
    let content_type = clean_value(sense_obj.get("content_type"));
    let emotional_register = clean_value(sense_obj.get("emotional_register"));
    let heading_parts = [content_type.as_str(), emotional_register.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if !heading_parts.is_empty() {
        lines.push(format!("## Sense: {}", heading_parts.join(" · ")));
    }

    let activity_summary = clean_value(sense_obj.get("activity_summary"));
    if !activity_summary.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(activity_summary);
    }

    let entity_lines = entities(sense_obj);
    if !entity_lines.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("### Entities".to_string());
        lines.extend(entity_lines);
    }

    let facet_lines = facets(sense_obj);
    if !facet_lines.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("### Facets".to_string());
        lines.extend(facet_lines);
    }

    if json_truthy(sense_obj.get("meeting_detected"))
        && let Some(Value::Array(speakers)) = sense_obj.get("speakers")
    {
        let speaker_names = speakers
            .iter()
            .map(|speaker| display_value(speaker).trim().to_string())
            .filter(|speaker| !speaker.is_empty())
            .collect::<Vec<_>>();
        if !speaker_names.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("**Speakers:** {}", speaker_names.join(", ")));
        }
    }

    lines.join("\n").trim().to_string()
}

fn entities(sense_obj: &JsonObject) -> Vec<String> {
    let mut lines = Vec::new();
    let Some(Value::Array(entities)) = sense_obj.get("entities") else {
        return lines;
    };

    for entity in entities {
        let Value::Object(entity) = entity else {
            continue;
        };
        let name = clean_value(entity.get("name"));
        if name.is_empty() {
            continue;
        }
        let etype = clean_value(entity.get("type"));
        let context_text = clean_value(entity.get("context"));
        let level = clean_value(entity.get("level"));
        let prefix = if etype.is_empty() {
            String::new()
        } else {
            format!("{etype}: ")
        };
        let mut line = format!("- {prefix}{name}");
        if !context_text.is_empty() {
            line.push_str(&format!(" — {context_text}"));
        }
        if !level.is_empty() {
            line.push_str(&format!(" ({level})"));
        }
        lines.push(line);
    }

    lines
}

fn facets(sense_obj: &JsonObject) -> Vec<String> {
    let mut lines = Vec::new();
    let Some(Value::Array(facets)) = sense_obj.get("facets") else {
        return lines;
    };

    for facet in facets {
        let Value::Object(facet) = facet else {
            continue;
        };
        let facet_name = clean_value(facet.get("facet"));
        let activity = clean_value(facet.get("activity"));
        let mut level = clean_value(facet.get("level"));
        if [facet_name.as_str(), activity.as_str(), level.as_str()]
            .iter()
            .all(|part| part.is_empty())
        {
            continue;
        }

        let mut text = if !facet_name.is_empty() && !activity.is_empty() {
            format!("{facet_name}: {activity}")
        } else if !facet_name.is_empty() {
            facet_name
        } else if !activity.is_empty() {
            activity
        } else {
            let text = format!("({level})");
            level.clear();
            text
        };
        if !level.is_empty() {
            text.push_str(&format!(" ({level})"));
        }
        lines.push(format!("- {text}"));
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
    fn renders_full_sense_record_and_agent_override() {
        let produced = render_one(
            r#"{"content_type":"meeting","emotional_register":"collaborative","activity_summary":"Reviewed the launch timeline with product leads.","entities":[{"type":"Person","name":"Alice Smith","context":"Owned the launch checklist.","level":"high"},{"type":"Tool","name":"Grafana","context":"Displayed dashboard latency."}],"facets":[{"facet":"work","activity":"launch planning","level":"high"}],"meeting_detected":true,"speakers":["Alice Smith","Bob Chen"]}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("sense"));
        assert_eq!(produced.chunks.len(), 1);
        let rendered = &produced.chunks[0].content;
        assert!(rendered.contains("## Sense: meeting · collaborative"));
        assert!(rendered.contains("Reviewed the launch timeline with product leads."));
        assert!(rendered.contains("### Entities"));
        assert!(rendered.contains("- Person: Alice Smith — Owned the launch checklist. (high)"));
        assert!(rendered.contains("- Tool: Grafana — Displayed dashboard latency."));
        assert!(rendered.contains("### Facets"));
        assert!(rendered.contains("- work: launch planning (high)"));
        assert!(rendered.contains("**Speakers:** Alice Smith, Bob Chen"));
    }

    #[test]
    fn empty_object_returns_zero_chunks() {
        let produced = render_one("{}");
        assert_eq!(produced.agent_override.as_deref(), Some("sense"));
        assert!(produced.chunks.is_empty());
    }

    #[test]
    fn drops_entities_without_name_and_formats_facets_by_available_fields() {
        let produced = render_one(
            r#"{"entities":[{"type":"Person","context":"No name."},{"name":"Named item"}],"facets":[{},{"level":"low"},{"facet":"work"},{"activity":"focus"}]}"#,
        );
        let rendered = &produced.chunks[0].content;
        assert!(!rendered.contains("No name."));
        assert!(rendered.contains("- Named item"));
        assert!(rendered.contains("- (low)"));
        assert!(rendered.contains("- work"));
        assert!(rendered.contains("- focus"));
    }

    #[test]
    fn suppresses_speakers_when_meeting_is_falsy_or_names_are_empty() {
        let produced = render_one(
            r#"{"content_type":"meeting","meeting_detected":false,"speakers":["Alice"]}"#,
        );
        assert!(!produced.chunks[0].content.contains("**Speakers:**"));

        let produced = render_one(
            r#"{"content_type":"meeting","meeting_detected":true,"speakers":["","   "]}"#,
        );
        assert!(!produced.chunks[0].content.contains("**Speakers:**"));
    }

    #[test]
    fn missing_or_non_object_record_keeps_agent_and_zero_chunks() {
        let produced = render(&[]);
        assert_eq!(produced.agent_override.as_deref(), Some("sense"));
        assert!(produced.chunks.is_empty());

        let produced = render_one("[]");
        assert_eq!(produced.agent_override.as_deref(), Some("sense"));
        assert!(produced.chunks.is_empty());
    }
}
