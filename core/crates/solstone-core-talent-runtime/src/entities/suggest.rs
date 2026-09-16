// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-suggest hook: gathers detection summaries and source evidence to generate candidate observations.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

pub const MAX_ACTIVE_ENTITIES: usize = 6;
pub const MAX_SUGGESTIONS_PER_ENTITY: usize = 10;
pub const MAX_ENTITIES_PER_FACET: usize = 20;
pub const MAX_SUMMARY_CHARS: usize = 1200;
pub const MAX_SOURCE_SEGMENTS: usize = 3;
pub const MAX_SEGMENT_CONTEXT_CHARS: usize = 800;
pub const MAX_ENTITY_CONTEXT_CHARS: usize = 3800;
pub const MAX_SUGGEST_CONTEXT_CHARS: usize = 24000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SuggestState {
    pub context: Option<String>,
    pub served_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionItem {
    pub content: String,
    pub reasoning: Option<String>,
    pub relation: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySuggestions {
    pub entity_id: String,
    pub suggestions: Vec<SuggestionItem>,
}

pub fn gate(
    talent: &PreparedTalent,
    _context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    if talent.config.get("day").and_then(Value::as_str).is_none() {
        return Ok(GateDecision::Skip("no_day".to_owned()));
    }
    if talent.config.get("facet").and_then(Value::as_str).is_none() {
        return Ok(GateDecision::Skip("no_facet".to_owned()));
    }
    Ok(GateDecision::Proceed)
}

pub fn build(
    talent: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let day = talent
        .config
        .get("day")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "entities:entity_suggest",
                talent,
                "missing day",
            ))
        })?;
    let facet = talent
        .config
        .get("facet")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "entities:entity_suggest",
                talent,
                "missing facet",
            ))
        })?;

    let assembly = assemble_suggest_context(&context.journal, facet, day).map_err(|e| {
        RuntimeOutcome::StageFailed(stage_error("build", "entities:entity_suggest", talent, e))
    })?;

    Ok(PrePostState::EntitySuggest(SuggestState {
        context: Some(assembly.context),
        served_ids: assembly.served_ids,
    }))
}

pub fn apply_prompt_override(
    talent: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::EntitySuggest(state) = state else {
        return Err(stage_error(
            "override",
            "entities:entity_suggest",
            talent,
            "expected entity suggest state",
        ));
    };
    if let Some(context) = &state.context {
        apply_template_vars(
            &mut talent.config,
            &Map::from_iter([("suggest_context".to_owned(), Value::String(context.clone()))]),
        );
    }
    Ok(())
}

pub fn parse(
    output: &str,
    talent: &PreparedTalent,
    _state: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    let data: Value = serde_json::from_str(output).map_err(|error| {
        stage_error(
            "parse",
            "entities:entity_suggest",
            talent,
            format!("could not parse output as JSON: {error}"),
        )
    })?;
    let cleaned = clean_suggestions_output(&data, talent)?;
    Ok(ParsedOutput::Text(cleaned))
}

fn clean_suggestions_output(data: &Value, talent: &PreparedTalent) -> Result<String, StageError> {
    let facet = talent
        .config
        .get("facet")
        .and_then(Value::as_str)
        .unwrap_or("");
    let day = talent
        .config
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or("");
    let Some(entities_arr) = data.get("entities").and_then(Value::as_array) else {
        return Err(stage_error(
            "parse",
            "entities:entity_suggest",
            talent,
            "entities must be an array",
        ));
    };
    let mut cleaned_entities = Vec::new();
    for entity_val in entities_arr.iter().take(MAX_ENTITIES_PER_FACET) {
        let Some(entity_obj) = entity_val.as_object() else {
            continue;
        };
        let Some(entity_id) = entity_obj.get("entity_id").and_then(Value::as_str) else {
            continue;
        };
        let suggestions_arr = entity_obj.get("suggestions").and_then(Value::as_array);
        let mut cleaned_suggestions = Vec::new();
        if let Some(suggs) = suggestions_arr {
            for s in suggs.iter().take(MAX_SUGGESTIONS_PER_ENTITY) {
                if let Some(content) = s
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    let mut item = Map::new();
                    item.insert("content".into(), Value::String(content.to_string()));
                    if let Some(rel) = s.get("relation").filter(|v| !v.is_null()) {
                        item.insert("relation".into(), rel.clone());
                    }
                    cleaned_suggestions.push(Value::Object(item));
                }
            }
        }
        cleaned_entities.push(serde_json::json!({
            "entity_id": entity_id,
            "suggestions": cleaned_suggestions
        }));
    }
    let result = serde_json::json!({
        "facet": facet,
        "day": day,
        "entities": cleaned_entities
    });
    Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()))
}

pub fn commit(
    parsed: ParsedOutput,
    talent: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "entities:entity_suggest",
            talent,
            "expected text output",
        ));
    };
    let PrePostState::EntitySuggest(_state) = state else {
        return Err(stage_error(
            "commit",
            "entities:entity_suggest",
            talent,
            "expected entity suggest state",
        ));
    };
    let facet = talent
        .config
        .get("facet")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_error("commit", "entities:entity_suggest", talent, "missing facet"))?;
    let day = talent
        .config
        .get("day")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_error("commit", "entities:entity_suggest", talent, "missing day"))?;

    Ok(CommitPlan::Write(WriteIntent::EntitySuggest {
        output,
        facet: facet.to_owned(),
        day: day.to_owned(),
    }))
}

struct SuggestContextAssembly {
    context: String,
    served_ids: BTreeSet<String>,
}

fn assemble_suggest_context(
    journal: &Path,
    facet: &str,
    day: &str,
) -> Result<SuggestContextAssembly, String> {
    let detected = solstone_core_facets::read_detected_entities(journal, facet, day)
        .map_err(|e| e.to_string())?;
    let scoped = solstone_core_facets::list_scoped_facet_entities(journal, facet, true, true)
        .map_err(|e| e.to_string())?;

    let candidates = scoped
        .iter()
        .map(
            |entity| solstone_core_entity_matching::EntityNameCandidate {
                id: Some(entity.entity_id.clone()),
                name: entity
                    .identity
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                aka: entity
                    .identity
                    .get("aka")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                emails: Vec::new(),
            },
        )
        .collect::<Vec<_>>();

    let mut active =
        BTreeMap::<String, (solstone_core_facets::ScopedFacetEntity, Vec<Value>)>::new();
    for row in detected {
        let name = row.get("name").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        if let Some(found) =
            solstone_core_entity_matching::find_matching_entity(name, &candidates, 90.0)
        {
            let entity = scoped[found.candidate_index].clone();
            active
                .entry(entity.entity_id.clone())
                .or_insert_with(|| (entity, Vec::new()))
                .1
                .push(row);
        }
    }

    if active.is_empty() {
        return Ok(SuggestContextAssembly {
            context: "No active entities found in today's content.".to_owned(),
            served_ids: BTreeSet::new(),
        });
    }

    let selected = active
        .into_values()
        .take(MAX_ACTIVE_ENTITIES)
        .collect::<Vec<_>>();

    let mut sections = Vec::with_capacity(selected.len());
    let mut served_ids = BTreeSet::new();

    for (entity, rows) in &selected {
        let packet = render_suggest_entity_packet(journal, facet, day, entity, rows)?;
        served_ids.insert(entity.entity_id.clone());
        sections.push(packet);
    }

    let header = [
        "# Entity Suggest Context".to_owned(),
        String::new(),
        format!("## Facet: {facet}"),
        format!("## Day: {day}"),
        format!("## Active Entities: {}", sections.len()),
        String::new(),
        "### Entities".to_owned(),
        String::new(),
    ];
    let mut context = header.join("\n");
    context.push_str(&sections.join("\n\n---\n\n"));

    let context_chars = context.chars().count();
    if context_chars > MAX_SUGGEST_CONTEXT_CHARS {
        return Err(format!(
            "entity suggest context is {context_chars} characters; maximum is {MAX_SUGGEST_CONTEXT_CHARS}"
        ));
    }

    Ok(SuggestContextAssembly {
        context,
        served_ids,
    })
}

fn render_suggest_entity_packet(
    journal: &Path,
    _facet: &str,
    day: &str,
    entity: &solstone_core_facets::ScopedFacetEntity,
    detected_rows: &[Value],
) -> Result<String, String> {
    let name = entity
        .identity
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let entity_type = entity
        .identity
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let description = entity
        .identity
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let aka = entity
        .identity
        .get("aka")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let mut parts = vec![
        format!("#### Entity: {name}"),
        format!("entity_id: {}", entity.entity_id),
        format!("type: {entity_type}"),
    ];
    if !description.is_empty() {
        parts.push(format!("description: {description}"));
    }
    if !aka.is_empty() {
        parts.push(format!("aliases: {aka}"));
    }

    // Detection summaries up to MAX_SUMMARY_CHARS (1200)
    let mut all_summaries = Vec::new();
    for row in detected_rows {
        if let Some(summary) = row
            .get("description")
            .or_else(|| row.get("summary"))
            .and_then(Value::as_str)
        {
            let trimmed = summary.trim();
            if !trimmed.is_empty() {
                all_summaries.push(trimmed);
            }
        }
    }

    let total_summary_chars: usize = all_summaries.iter().map(|s| s.chars().count()).sum();
    let mut rendered_summaries = Vec::new();
    let mut accumulated_chars = 0usize;

    for summary in &all_summaries {
        let count = summary.chars().count();
        if accumulated_chars + count <= MAX_SUMMARY_CHARS {
            rendered_summaries.push(*summary);
            accumulated_chars += count;
        } else {
            break;
        }
    }

    parts.push(String::new());
    parts.push("Detection summaries:".to_owned());
    if rendered_summaries.is_empty() {
        parts.push("  (none)".to_owned());
    } else {
        for summary in &rendered_summaries {
            parts.push(format!("- {summary}"));
        }
        if total_summary_chars > MAX_SUMMARY_CHARS {
            parts.push(format!(
                "  (showing {} of {} summaries)",
                rendered_summaries.len(),
                all_summaries.len()
            ));
        }
    }

    // Evidence from day segments
    parts.push(String::new());
    parts.push("Source evidence:".to_owned());

    let mut segment_labels = BTreeSet::new();
    for row in detected_rows {
        if let Some(Value::Array(segments)) = row.get("segments") {
            for seg in segments {
                if let Some(label) = seg.as_str().filter(|l| !l.trim().is_empty()) {
                    segment_labels.insert(label.to_owned());
                }
            }
        }
    }

    let mut segment_count = 0usize;
    for label in segment_labels {
        if segment_count >= MAX_SOURCE_SEGMENTS {
            break;
        }
        if let Some(text) = load_segment_text(journal, day, &label) {
            let truncated: String = text.chars().take(MAX_SEGMENT_CONTEXT_CHARS).collect();
            parts.push(format!("- [source {label}]: {truncated}"));
            segment_count += 1;
        }
    }
    if segment_count == 0 {
        parts.push("  (no source excerpts available)".to_owned());
    }

    let rendered = parts.join("\n");
    let count = rendered.chars().count();
    if count > MAX_ENTITY_CONTEXT_CHARS {
        return Err(format!(
            "entity packet for {} is {count} characters; maximum is {MAX_ENTITY_CONTEXT_CHARS}",
            entity.entity_id
        ));
    }

    Ok(rendered)
}

fn load_segment_text(journal: &Path, day: &str, label: &str) -> Option<String> {
    let parts = label.split('/').collect::<Vec<_>>();
    let (seg_day, stream, seg_key) = match parts.as_slice() {
        [seg_day, seg] => (*seg_day, solstone_core_journal_io::DEFAULT_STREAM, *seg),
        [seg_day, stream, seg] if !stream.is_empty() => (*seg_day, *stream, *seg),
        _ => return None,
    };
    if seg_day != day {
        return None;
    }
    let source_config = Map::from_iter([
        ("transcripts".to_owned(), Value::Bool(true)),
        ("percepts".to_owned(), Value::Bool(true)),
        ("talents".to_owned(), Value::Bool(false)),
    ]);
    let (source, counts) = crate::transcript::load_segment_transcript(
        journal,
        day,
        seg_key,
        Some(stream),
        &source_config,
    );
    if counts.total() == 0 {
        None
    } else {
        Some(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_gate_proceeds_with_day_and_facet() {
        let talent = PreparedTalent {
            name: "entities:entity_suggest".into(),
            config: serde_json::json!({"day": "20260910", "facet": "work"})
                .as_object()
                .unwrap()
                .clone(),
        };
        let context = ExecutionContext {
            journal: std::path::PathBuf::from("/tmp/nonexistent"),
        };
        assert_eq!(gate(&talent, &context).unwrap(), GateDecision::Proceed);
    }

    #[test]
    fn suggest_build_no_active_entities_returns_empty_context() {
        let temp = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(temp.path(), "work", "Work", "", "", "", None).unwrap();
        let mut talent = PreparedTalent {
            name: "entities:entity_suggest".into(),
            config: serde_json::json!({"day": "20260910", "facet": "work"})
                .as_object()
                .unwrap()
                .clone(),
        };
        let context = ExecutionContext {
            journal: temp.path().to_path_buf(),
        };
        let state = build(&mut talent, &context).unwrap();
        let PrePostState::EntitySuggest(state) = state else {
            panic!("expected EntitySuggest state");
        };
        assert!(state.served_ids.is_empty());
        assert!(state.context.unwrap().contains("No active entities"));
    }

    #[test]
    fn suggest_build_and_override_with_active_entity() {
        let temp = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(temp.path(), "work", "Work", "", "", "", None).unwrap();
        solstone_core_facets::attach_or_reactivate_entity(
            temp.path(),
            "work",
            "Person",
            "Ada",
            "Mathematician",
        )
        .unwrap();
        solstone_core_facets::save_detected_entity(
            temp.path(),
            "work",
            "20260910",
            "Person",
            "Ada",
            "Worked on the analytical engine notes",
        )
        .unwrap();

        let mut talent = PreparedTalent {
            name: "entities:entity_suggest".into(),
            config: serde_json::json!({
                "day": "20260910",
                "facet": "work",
                "prompt": "Context:\n$suggest_context\nEnd"
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let context = ExecutionContext {
            journal: temp.path().to_path_buf(),
        };
        let state = build(&mut talent, &context).unwrap();
        let PrePostState::EntitySuggest(ref sugg_state) = state else {
            panic!("expected EntitySuggest state");
        };
        assert!(sugg_state.served_ids.contains("ada"));

        apply_prompt_override(&mut talent, &state).unwrap();
        let prompt = talent.config.get("prompt").and_then(Value::as_str).unwrap();
        assert!(prompt.contains("Ada"));
        assert!(prompt.contains("Worked on the analytical engine notes"));
    }

    #[test]
    fn suggest_summary_char_truncation() {
        let temp = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(temp.path(), "work", "Work", "", "", "", None).unwrap();
        solstone_core_facets::attach_or_reactivate_entity(
            temp.path(),
            "work",
            "Person",
            "Ada",
            "Mathematician",
        )
        .unwrap();
        solstone_core_facets::save_detected_entity(
            temp.path(),
            "work",
            "20260910",
            "Person",
            "Ada",
            &format!("Detailed detection summary block which contains quite a lot of text repeating words to fill up characters: {}", "x".repeat(1500)),
        )
        .unwrap();

        let mut talent = PreparedTalent {
            name: "entities:entity_suggest".into(),
            config: serde_json::json!({
                "day": "20260910",
                "facet": "work",
                "prompt": "$suggest_context"
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let context = ExecutionContext {
            journal: temp.path().to_path_buf(),
        };
        let state = build(&mut talent, &context).unwrap();
        apply_prompt_override(&mut talent, &state).unwrap();
        let prompt = talent.config.get("prompt").and_then(Value::as_str).unwrap();
        assert!(prompt.contains("Ada"));
        assert!(prompt.contains("Detection summaries:"));
    }

    #[test]
    fn suggest_output_capping_and_commit() {
        let temp = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(temp.path(), "work", "Work", "", "", "", None).unwrap();

        let mut suggestions = Vec::new();
        for i in 0..15 {
            suggestions.push(serde_json::json!({"content": format!("Suggestion {i}")}));
        }
        let output = serde_json::json!({
            "entities": [
                {
                    "entity_id": "ada",
                    "suggestions": suggestions
                }
            ]
        })
        .to_string();

        let talent = PreparedTalent {
            name: "entities:entity_suggest".into(),
            config: serde_json::json!({
                "day": "20260910",
                "facet": "work"
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let state = PrePostState::EntitySuggest(SuggestState {
            context: None,
            served_ids: BTreeSet::from(["ada".into()]),
        });

        let parsed = parse(&output, &talent, &state).unwrap();
        let plan = commit(parsed, &talent, &state).unwrap();
        let context = ExecutionContext {
            journal: temp.path().to_path_buf(),
        };
        crate::writers::apply(plan, &context).unwrap();

        let path = temp
            .path()
            .join("facets/work/entities/20260910_observer_suggestions.json");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed_json: Value = serde_json::from_str(&content).unwrap();
        let entities = parsed_json["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0]["entity_id"], "ada");
        // Must be capped at MAX_SUGGESTIONS_PER_ENTITY = 10
        let suggs = entities[0]["suggestions"].as_array().unwrap();
        assert_eq!(suggs.len(), 10);
    }
}
