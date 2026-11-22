// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ops::Range;

use serde_json::Value;
use solstone_core_cli_copy::{ENGLISH, Value as Argument, render_for};

use crate::stored_markdown::Document;

use super::{JsonObject, ProducedChunks, clean_value, display_value, json_truthy, recorded_chunk};

/// Render the saved Sense projections for the owner without changing what
/// thinking and Search index. The entity list is also stored in individual
/// index chunks, which have the same heading and omit the bullet.
pub fn owner_sense_text(markdown: &str) -> Option<String> {
    if let Some((heading, body)) = markdown.split_once('\n')
        && heading.trim_end() == "# Sense Entities"
    {
        return owner_sense_list(body.trim_start_matches(['\n', '\r']));
    }
    owner_sense_record(markdown)
}

fn owner_sense_list(body: &str) -> Option<String> {
    let is_list = body.starts_with("- ");
    let mut projected = false;
    let lines = body
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let item = if is_list {
                line.strip_prefix("- ").map(|item| ("- ", item))
            } else {
                (index == 0).then_some(("", line))
            };
            let Some((bullet, (name, context))) =
                item.and_then(|(bullet, item)| Some((bullet, sense_entity_fields(item)?)))
            else {
                return line.to_owned();
            };
            projected = true;
            if context.is_empty() {
                format!("{bullet}{name}")
            } else {
                format!("{bullet}{name} — {context}")
            }
        })
        .collect::<Vec<_>>();
    projected.then(|| lines.join("\n"))
}

fn sense_entity_fields(item: &str) -> Option<(&str, &str)> {
    let (kind, entity) = item.split_once(" — ")?;
    if !entity_kind(kind) {
        return None;
    }
    entity
        .match_indices(" (role=")
        .find_map(|(offset, marker)| {
            let fields = &entity[offset + marker.len()..];
            let (metadata, context) = fields
                .split_once(") — ")
                .or_else(|| fields.strip_suffix(") —").map(|metadata| (metadata, "")))?;
            let (role, source) = metadata.split_once(", source=")?;
            // The inserted fields are enum values. A name can contain "(role=".
            [role, source]
                .iter()
                .all(|field| {
                    field
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                })
                .then_some((&entity[..offset], context))
        })
}

// JSON Sense records are indexed using render_sense below. Keep their summary,
// facets and speakers while projecting the generated entity rows.
fn owner_sense_record(markdown: &str) -> Option<String> {
    let mut projected = false;
    let mut in_entities = false;
    let lines = markdown
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if index == 0 && line.starts_with("## Sense: ") {
                projected = true;
                return None;
            }
            if line.starts_with("### ") {
                in_entities = line == "### Entities";
                if in_entities {
                    projected = true;
                    return None;
                }
            }
            if in_entities
                && let Some(item) = line.strip_prefix("- ")
                && let Some((kind, detail)) = item.split_once(": ")
                && entity_kind(kind)
            {
                let detail = [" (high)", " (medium)", " (low)"]
                    .iter()
                    .find_map(|level| detail.strip_suffix(level))
                    .unwrap_or(detail);
                return Some(format!("- {detail}"));
            }
            Some(line.to_owned())
        })
        .collect::<Vec<_>>();
    let text = lines.join("\n");
    (projected && !text.trim().is_empty()).then(|| text.trim().to_owned())
}

fn entity_kind(kind: &str) -> bool {
    matches!(kind, "Person" | "Company" | "Project" | "Tool")
}

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
        error: None,
        warnings: Vec::new(),
    }
}

fn render_sense(record: &JsonObject) -> String {
    sense_text(record, ENGLISH).body.into_text()
}

struct Edit {
    range: Range<usize>,
    translated: Option<String>,
    hide_in_excerpt: bool,
}

#[derive(Default)]
struct SenseText {
    body: Document,
    edits: Vec<Edit>,
}

impl SenseText {
    fn section(&mut self) {
        if !self.body.is_empty() {
            self.body.push_str("\n\n");
        }
    }

    fn field(&mut self, english: &str, translated: Option<String>, hide_in_excerpt: bool) {
        let start = self.body.len();
        self.body.push_str(english);
        if translated.is_some() || hide_in_excerpt {
            self.edits.push(Edit {
                range: start..self.body.len(),
                translated,
                hide_in_excerpt,
            });
        }
    }

    fn message(&mut self, locale: &str, id: &str, args: &[(&str, Argument<'_>)], hide: bool) {
        self.field(
            &render_for(ENGLISH, id, args),
            (locale != ENGLISH).then(|| render_for(locale, id, args)),
            hide,
        );
    }

    // The index's English cutoff selects the same owner characters in both views.
    fn display(&self, retained: usize, tail: &str, excerpt: bool) -> String {
        let mut shown = String::new();
        let mut end = 0;
        for edit in self.edits.iter().take_while(|e| e.range.start < retained) {
            shown.push_str(&self.body[end..edit.range.start]);
            end = edit.range.end.min(retained);
            if excerpt && edit.hide_in_excerpt {
                continue;
            }
            let kept = &self.body[edit.range.start..end];
            if end == edit.range.end {
                shown.push_str(edit.translated.as_deref().unwrap_or(kept));
            } else {
                shown.push_str(kept);
            }
        }
        shown.push_str(&self.body[end..retained]);
        shown.push_str(tail);
        shown.trim().to_owned()
    }
}

pub fn sense_presentations(bytes: &[u8], locale: &str) -> Option<(String, String, Option<String>)> {
    let record: JsonObject = serde_json::from_slice(bytes).ok()?;
    let mut text = sense_text(&record, locale);
    if text.body.is_empty() {
        return None;
    }
    let retained = super::append_partial_input_notice(&mut text.body, &record);
    let tail = text.body.render_from(retained, locale);
    let full = text.display(retained, &tail, false);
    let owner = text
        .edits
        .iter()
        .any(|e| e.hide_in_excerpt && e.range.start < retained)
        .then(|| text.display(retained, &tail, true))
        .filter(|s| !s.is_empty());
    Some((text.body.into_text(), full, owner))
}

fn sense_text(record: &JsonObject, locale: &str) -> SenseText {
    let mut text = SenseText::default();
    let kind = clean_value(record.get("content_type"));
    if !kind.is_empty() {
        text.message(
            locale,
            "journal.cli.think.markdown.sense.record",
            &[("kind", Argument::String(&kind))],
            true,
        );
    }

    let summary = clean_value(record.get("activity_summary"));
    if !summary.is_empty() {
        text.section();
        text.body.push_str(&summary);
    }

    let mut first_entity = true;
    for entity in record
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        let name = clean_value(entity.get("name"));
        if name.is_empty() {
            continue;
        }
        if first_entity {
            text.section();
            text.message(
                locale,
                "journal.cli.think.markdown.sense.entities",
                &[],
                true,
            );
            text.field("\n", None, true);
            first_entity = false;
        } else {
            text.body.push_str("\n");
        }
        let kind = clean_value(entity.get("type"));
        let context = clean_value(entity.get("context"));
        let level = clean_value(entity.get("level"));
        text.body.push_str("- ");
        if !kind.is_empty() {
            text.field(&format!("{kind}: "), None, entity_kind(&kind));
        }
        text.body.push_str(&name);
        if !context.is_empty() {
            text.body.push_str(" — ");
            text.body.push_str(&context);
        }
        if !level.is_empty() {
            text.field(
                &format!(" ({level})"),
                None,
                entity_kind(&kind) && matches!(level.as_str(), "high" | "medium" | "low"),
            );
        }
    }

    let facet_lines = facets(record);
    if !facet_lines.is_empty() {
        text.section();
        text.message(
            locale,
            "journal.cli.think.markdown.sense.facets",
            &[],
            false,
        );
        text.body.push_str("\n");
        text.body.push_str(&facet_lines.join("\n"));
    }
    if json_truthy(record.get("meeting_detected"))
        && let Some(Value::Array(speakers)) = record.get("speakers")
    {
        let names = speakers
            .iter()
            .map(|s| display_value(s).trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        if !names.is_empty() {
            text.section();
            text.message(
                locale,
                "journal.cli.think.markdown.sense.speakers",
                &[("names", Argument::String(&names))],
                false,
            );
        }
    }
    text
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
    fn owner_names_read_saved_entity_markdown_and_its_indexed_chunk() {
        let saved = "# Sense Entities\n\n- Project — Acorn — v2 (role=reader) (role=mentioned, source=transcript) — Reviewed the plan (role=editor).\n  Tool — B (role=mentioned, source=screen) — quoted example\n- Person — Ada (role=attendee, source=speaker_label) — \n\nLeft out: Some input did not fit.";
        let shown = owner_sense_text(saved).expect("saved Sense list");
        assert!(shown.contains("Acorn — v2"));
        assert!(shown.contains("Reviewed the plan (role=editor)."));
        assert!(shown.contains("- Ada\n"));
        assert!(shown.contains("Left out:"));
        assert!(!shown.contains("source=transcript"));
        assert!(!shown.contains("source=speaker_label"));

        let indexed = crate::chunker::format_markdown(saved);
        let excerpt =
            owner_sense_text(&indexed.chunks[0].markdown).expect("Sense list item after chunking");
        assert!(excerpt.starts_with("Acorn — v2 (role=reader)"), "{excerpt}");
        assert!(excerpt.contains("Reviewed the plan (role=editor)."));
        assert!(excerpt.contains("Tool — B (role=mentioned, source=screen) — quoted example"));
        assert!(!excerpt.contains("source=transcript"));
        assert!(owner_sense_text("# Sense Entities\n\nA note with no entity fields.").is_none());
    }

    #[test]
    fn owner_sense_record_keeps_the_summary_and_names_from_the_existing_index_text() {
        let stored = render_one(
            r#"{"content_type":"meeting","activity_summary":"Reviewed launch.","entities":[{"type":"Person","name":"Ada","context":"Confirmed the priority was (high)","level":"medium"}],"facets":[{"facet":"work","activity":"planning"}]}"#,
        );
        let shown = owner_sense_text(&stored.chunks[0].content).expect("Sense record");
        assert!(shown.starts_with("Reviewed launch."));
        assert!(shown.contains("Ada — Confirmed the priority was (high)"));
        assert!(shown.contains("work: planning"));
        for internal in ["Sense: meeting", "Person: Ada", "(medium)"] {
            assert!(!shown.contains(internal));
        }
    }

    #[test]
    fn renders_full_sense_record_and_agent_override() {
        let produced = render_one(
            r#"{"content_type":"meeting","emotional_register":"collaborative","activity_summary":"Reviewed the launch timeline with product leads.","entities":[{"type":"Person","name":"Alice Smith","context":"Owned the launch checklist.","level":"high"},{"type":"Tool","name":"Grafana","context":"Displayed dashboard latency."}],"facets":[{"facet":"work","activity":"launch planning","level":"high"}],"meeting_detected":true,"speakers":["Alice Smith","Bob Chen"]}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("sense"));
        assert_eq!(produced.chunks.len(), 1);
        let rendered = &produced.chunks[0].content;
        // A record written by an older version still carries a mood label;
        // the journal never shows one.
        assert!(rendered.starts_with("## Sense: meeting\n"));
        assert!(!rendered.contains("collaborative"));
        assert!(rendered.contains("Reviewed the launch timeline with product leads."));
        assert!(rendered.contains("### Entities"));
        assert!(rendered.contains("- Person: Alice Smith — Owned the launch checklist. (high)"));
        assert!(rendered.contains("- Tool: Grafana — Displayed dashboard latency."));
        assert!(rendered.contains("### Facets"));
        assert!(rendered.contains("- work: launch planning (high)"));
        assert!(rendered.contains("**Speakers:** Alice Smith, Bob Chen"));
    }

    #[test]
    fn partial_sense_keeps_exactly_the_english_summary_in_each_size_tier() {
        let locale = "en-XA";
        let heading = "## Sense: meeting\n\n";
        let notice = crate::stored_markdown::Message::PartialInput;
        for limit in [16 * 1024, 64 * 1024] {
            let mut summary = "界".repeat((limit - heading.len()) / "界".len());
            summary.push_str(&"x".repeat(limit - heading.len() - summary.len()));
            let record = serde_json::json!({
                "content_type": "meeting", "activity_summary": summary, "partial_input": {}
            });
            let indexed = render_one(&record.to_string()).chunks.remove(0).content;
            let (source, full, owner) =
                sense_presentations(&serde_json::to_vec(&record).unwrap(), locale).unwrap();
            assert_eq!(source, indexed);
            let visible = source
                .strip_prefix(heading)
                .unwrap()
                .strip_suffix(&format!("\n\n{}", notice.render(ENGLISH)))
                .unwrap();
            assert!(summary.starts_with(visible) && visible.len() < summary.len());
            let display_note = notice.render(locale);
            assert_eq!(owner.unwrap(), format!("{visible}\n\n{display_note}"));
            let display_heading = render_for(
                locale,
                "journal.cli.think.markdown.sense.record",
                &[("kind", Argument::String("meeting"))],
            );
            assert_eq!(
                full,
                format!("{display_heading}\n\n{visible}\n\n{display_note}")
            );
        }
    }

    #[test]
    fn structured_owner_excerpt_only_removes_system_fields() {
        let summary = "### Entities\n- Person: quoted (high)";
        let record = serde_json::json!({"activity_summary": summary, "entities": []});
        let (_, full, owner) =
            sense_presentations(&serde_json::to_vec(&record).unwrap(), "en-XA").unwrap();
        assert_eq!(full, summary);
        assert!(owner.is_none());

        let record = serde_json::json!({"content_type":"meeting", "activity_summary":summary,
            "entities":[{"type":"Person","name":"Person: Ada (high)","context":"","level":"high"},
                        {"type":"Tool","name":"Notes","context":"### Entities","level":"unknown"}],
            "meeting_detected":true, "speakers":[2,"Speaker 2"]});
        let (_, _, owner) =
            sense_presentations(&serde_json::to_vec(&record).unwrap(), "en-XA").unwrap();
        let speakers = render_for(
            "en-XA",
            "journal.cli.think.markdown.sense.speakers",
            &[("names", Argument::String("2, Speaker 2"))],
        );
        assert_eq!(
            owner.unwrap(),
            format!(
                "{summary}\n\n- Person: Ada (high)\n- Notes — ### Entities (unknown)\n\n{speakers}"
            )
        );
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
