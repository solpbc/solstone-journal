// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-observer hook: reconciles candidate suggestions against existing observations.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObserverState {
    pub context: Option<String>,
    pub served_ids: BTreeSet<String>,
    pub shown_observation_ids: BTreeMap<String, BTreeSet<u64>>,
    pub observation_before: Map<String, Value>,
    pub resolution: Option<String>,
}

type Counts = BTreeMap<&'static str, usize>;

pub const MAX_ACTIVE_ENTITIES: usize = 6;
pub const MAX_ENTITY_CONTEXT_CHARS: usize = 3800;
pub const MAX_OBSERVER_CONTEXT_CHARS: usize = 24000;

fn empty_counts() -> Counts {
    BTreeMap::from([
        ("add", 0),
        ("replace", 0),
        ("skip", 0),
        ("refused", 0),
        ("relation_unresolved", 0),
        ("unselected", 0),
    ])
}

fn write_outcome(
    journal: &Path,
    facet: &str,
    day: &str,
    counts: &Counts,
    served_ids: &BTreeSet<String>,
    error: Option<&str>,
) -> Result<(), String> {
    let path = journal
        .join("facets")
        .join(facet)
        .join("entities")
        .join(format!("{day}_observer_outcome.json"));
    let mut payload = Map::from_iter(
        counts
            .iter()
            .map(|(name, count)| ((*name).to_owned(), Value::from(*count))),
    );
    payload.insert(
        "served_ids".to_owned(),
        serde_json::to_value(served_ids).map_err(|e| e.to_string())?,
    );
    payload.insert(
        "error".to_owned(),
        error.map_or(Value::Null, |error| Value::String(error.to_owned())),
    );
    payload.insert(
        "ts".to_owned(),
        Value::from(chrono::Utc::now().timestamp_millis()),
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|write_error| write_error.to_string())?;
    }
    fs::write(path, format!("{}\n", Value::Object(payload)))
        .map_err(|write_error| write_error.to_string())
}

fn target_id(value: Option<&Value>) -> Option<u64> {
    value
        .and_then(Value::as_u64)
        .filter(|_| !value.is_some_and(Value::is_boolean))
}

fn target_quote(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(300).collect())
}

fn relation_kind(value: Option<&Value>) -> Option<&'static str> {
    let kind = value.and_then(Value::as_str)?;
    match kind {
        "works-with" => Some("works-with"),
        "works-at" => Some("works-at"),
        "reports-to" => Some("reports-to"),
        "family-of" => Some("family-of"),
        "knows" => Some("knows"),
        "uses" => Some("uses"),
        "created" => Some("created"),
        "other" => Some("other"),
        _ => None,
    }
}

fn resolve_relation_target(
    candidates: &[solstone_core_entity_matching::EntityNameCandidate],
    target_name: &str,
    current_entity_id: &str,
) -> Option<String> {
    let matched =
        solstone_core_entity_matching::find_matching_entity(target_name, candidates, 90.0)?;
    let candidate = &candidates[matched.candidate_index];
    let id = candidate.id.as_deref()?;
    if id == current_entity_id {
        return None;
    }
    Some(id.to_owned())
}

fn clean_relation(
    value: Option<&Value>,
    _op: &str,
    context: &OperationContext<'_>,
) -> Result<(Option<Value>, Option<&'static str>), String> {
    let Some(value) = value else {
        return Ok((None, None));
    };
    if value.is_null() {
        return Ok((None, None));
    }
    let Some(relation) = value.as_object() else {
        return Ok((None, Some("refused")));
    };
    let Some(kind) = relation_kind(relation.get("kind")) else {
        return Ok((None, Some("refused")));
    };
    let Some(target_name) = relation
        .get("target_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return Ok((None, Some("refused")));
    };
    let note = relation
        .get("note")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(300).collect::<String>());
    if kind == "other" && note.is_none() {
        return Ok((None, Some("refused")));
    }
    let target_entity_id =
        resolve_relation_target(context.candidates, target_name, context.entity_id);
    let status = target_entity_id.is_none().then_some("relation_unresolved");
    Ok((
        Some(
            json!({"kind":kind,"target_entity_id":target_entity_id,"target_name":target_name,"note":note}),
        ),
        status,
    ))
}

struct OperationContext<'a> {
    entity_id: &'a str,
    candidates: &'a [solstone_core_entity_matching::EntityNameCandidate],
    shown_ids: &'a BTreeSet<u64>,
}

fn clean_operation(
    item: &Value,
    seen_ids: &mut Vec<u64>,
    context: &OperationContext<'_>,
) -> Result<(Option<Value>, Option<&'static str>), String> {
    let Some(item) = item.as_object() else {
        return Ok((None, Some("refused")));
    };
    let Some(op) = item.get("op").and_then(Value::as_str) else {
        return Ok((None, Some("refused")));
    };
    if op == "skip" {
        return Ok((Some(json!({"op": "skip"})), None));
    }
    if op == "add" {
        let Some(content) = item
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return Ok((None, Some("refused")));
        };
        let (relation, status) = clean_relation(item.get("relation"), op, context)?;
        if status == Some("refused") {
            return Ok((None, status));
        }
        let mut clean = Map::from_iter([
            ("op".to_owned(), Value::String("add".to_owned())),
            ("content".to_owned(), Value::String(content.to_owned())),
        ]);
        if let Some(relation) = relation {
            clean.insert("relation".to_owned(), relation);
        }
        return Ok((Some(Value::Object(clean)), status));
    }
    if op != "replace" {
        return Ok((None, Some("refused")));
    }
    let Some(id) = target_id(item.get("target_id")) else {
        return Ok((None, Some("refused")));
    };
    if seen_ids.contains(&id) {
        return Ok((None, Some("refused")));
    }
    if !context.shown_ids.contains(&id) {
        return Ok((None, Some("refused")));
    }
    let quote_value = item.get("target_quote");
    let Some(quote) = target_quote(quote_value) else {
        return Ok((None, Some("refused")));
    };
    let Some(content) = item
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return Ok((None, Some("refused")));
    };
    seen_ids.push(id);
    let (relation, status) = clean_relation(item.get("relation"), op, context)?;
    if status == Some("refused") {
        return Ok((None, status));
    }
    let mut clean = Map::from_iter([
        ("op".to_owned(), Value::String("replace".to_owned())),
        ("target_id".to_owned(), Value::from(id)),
        ("target_quote".to_owned(), Value::String(quote)),
        ("content".to_owned(), Value::String(content.to_owned())),
    ]);
    if let Some(relation) = relation {
        clean.insert("relation".to_owned(), relation);
    }
    Ok((Some(Value::Object(clean)), status))
}

fn merge_counts(counts: &mut Counts, source: &solstone_core_facets::ObservationOperationCounts) {
    *counts.entry("add").or_default() += source.add;
    *counts.entry("replace").or_default() += source.replace;
    // The model's own "skip" decisions and the store's backstop for an "add" it
    // already holds (content, day and relation all matching a live row) both mean
    // the same thing to this outcome: no change was made. Roll both into one
    // bucket rather than let the backstop case vanish from the day's counts.
    *counts.entry("skip").or_default() += source.skip + source.keep;
    *counts.entry("refused").or_default() += source.refused;
}

pub fn gate(
    talent: &PreparedTalent,
    context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    let day = talent
        .config
        .get("day")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_error("gate", "entities:entity_observer", talent, "missing day"))?;
    let facet = talent
        .config
        .get("facet")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_error("gate", "entities:entity_observer", talent, "missing facet"))?;

    let sugg_path = context
        .journal
        .join("facets")
        .join(facet)
        .join("entities")
        .join(format!("{day}_observer_suggestions.json"));

    if !sugg_path.exists() {
        return Ok(GateDecision::Proceed);
    }

    let Ok(content) = fs::read_to_string(&sugg_path) else {
        return Ok(GateDecision::Proceed);
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&content) else {
        return Ok(GateDecision::Proceed);
    };
    let Some(entities) = parsed.get("entities").and_then(Value::as_array) else {
        return Ok(GateDecision::Proceed);
    };

    let has_suggestions = entities.iter().any(|e| {
        e.get("suggestions")
            .and_then(Value::as_array)
            .is_some_and(|s| !s.is_empty())
    });

    if !has_suggestions {
        return Ok(GateDecision::Skip("no_candidates".to_owned()));
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
                "entities:entity_observer",
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
                "entities:entity_observer",
                talent,
                "missing facet",
            ))
        })?;

    let sugg_rel = format!("facets/{facet}/entities/{day}_observer_suggestions.json");
    let sugg_path = context.journal.join(&sugg_rel);

    if !sugg_path.exists() {
        return Err(RuntimeOutcome::StageFailed(stage_error(
            "build",
            "entities:entity_observer",
            talent,
            format!("missing suggestions artifact: {sugg_rel}"),
        )));
    }

    let sugg_content = fs::read_to_string(&sugg_path).map_err(|e| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "entities:entity_observer",
            talent,
            format!("failed to read {sugg_rel}: {e}"),
        ))
    })?;

    let sugg_val: Value = serde_json::from_str(&sugg_content).map_err(|e| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "entities:entity_observer",
            talent,
            format!("invalid suggestions JSON in {sugg_rel}: {e}"),
        ))
    })?;

    let entities = sugg_val
        .get("entities")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "entities:entity_observer",
                talent,
                "suggestions JSON missing entities array",
            ))
        })?;

    let mut candidate_map: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for e in entities {
        let Some(entity_id) = e.get("entity_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(suggestions) = e.get("suggestions").and_then(Value::as_array) else {
            continue;
        };
        if !suggestions.is_empty() {
            candidate_map.insert(entity_id.to_owned(), suggestions.clone());
        }
    }

    if candidate_map.is_empty() {
        return Err(RuntimeOutcome::Skipped {
            stage: "build".to_owned(),
            talent: talent.name.clone(),
            reason: "no_candidates".to_owned(),
        });
    }

    let scoped =
        solstone_core_facets::list_scoped_facet_entities(&context.journal, facet, true, true)
            .map_err(|e| {
                RuntimeOutcome::StageFailed(stage_error(
                    "build",
                    "entities:entity_observer",
                    talent,
                    e.to_string(),
                ))
            })?;

    let selected = candidate_map
        .into_iter()
        .take(MAX_ACTIVE_ENTITIES)
        .collect::<Vec<_>>();

    let mut sections = Vec::with_capacity(selected.len());
    let mut served_ids = BTreeSet::new();
    let mut shown_observation_ids = BTreeMap::new();
    let mut observation_before = Map::new();

    for (entity_id, suggestions) in &selected {
        let Some(scoped_entity) = scoped.iter().find(|e| &e.entity_id == entity_id) else {
            continue;
        };

        let before = solstone_core_facets::read_facet_entity_observations(
            &context.journal,
            facet,
            &scoped_entity.relationship_dir,
        )
        .map_err(|e| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "entities:entity_observer",
                talent,
                e.to_string(),
            ))
        })?;

        let (packet, shown_ids) = render_reconcile_packet(
            &context.journal,
            facet,
            scoped_entity,
            suggestions,
            before.as_deref(),
        )
        .map_err(|e| {
            RuntimeOutcome::StageFailed(stage_error("build", "entities:entity_observer", talent, e))
        })?;

        observation_before.insert(
            scoped_entity.entity_id.clone(),
            before.map(Value::String).unwrap_or(Value::Null),
        );
        served_ids.insert(scoped_entity.entity_id.clone());
        shown_observation_ids.insert(scoped_entity.entity_id.clone(), shown_ids);
        sections.push(packet);
    }

    let header = [
        "# Entity Observer Context".to_owned(),
        String::new(),
        format!("## Facet: {facet}"),
        format!("## Day: {day}"),
        format!("## Active Entities: {}", sections.len()),
        String::new(),
        "### Entities".to_owned(),
        String::new(),
    ];
    let mut context_str = header.join("\n");
    context_str.push_str(&sections.join("\n\n---\n\n"));

    let context_chars = context_str.chars().count();
    if context_chars > MAX_OBSERVER_CONTEXT_CHARS {
        return Err(RuntimeOutcome::StageFailed(stage_error(
            "build",
            "entities:entity_observer",
            talent,
            format!(
                "entity observer context is {context_chars} characters; maximum is {MAX_OBSERVER_CONTEXT_CHARS}"
            ),
        )));
    }

    Ok(PrePostState::EntityObserver(ObserverState {
        context: Some(context_str),
        served_ids,
        shown_observation_ids,
        observation_before,
        resolution: None,
    }))
}

fn render_reconcile_packet(
    _journal: &Path,
    _facet: &str,
    entity: &solstone_core_facets::ScopedFacetEntity,
    suggestions: &[Value],
    before_raw: Option<&str>,
) -> Result<(String, BTreeSet<u64>), String> {
    let name = entity
        .identity
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&entity.entity_id);
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

    parts.push(String::new());
    parts.push("Suggested today:".to_owned());
    for (idx, sugg) in suggestions.iter().take(3).enumerate() {
        let num = idx + 1;
        let content = sugg.get("content").and_then(Value::as_str).unwrap_or("");
        let truncated: String = content.chars().take(600).collect();
        parts.push(format!("S{num}: {truncated}"));
    }

    let mut rows = Vec::new();
    if let Some(raw) = before_raw {
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            let is_live =
                val.get("retired").is_none() || val.get("retired").is_some_and(Value::is_null);
            if !is_live {
                continue;
            }
            if let Some(id) = val.get("id").and_then(Value::as_u64) {
                let content = val
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let observed_at = val.get("observed_at").and_then(Value::as_i64).unwrap_or(0);
                let source_day = val
                    .get("source_day")
                    .and_then(Value::as_str)
                    .map(|s| s.to_owned());
                let has_history = val
                    .get("history")
                    .and_then(Value::as_array)
                    .is_some_and(|h| !h.is_empty());
                rows.push((id, content, observed_at, source_day, has_history));
            }
        }
    }

    rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.0.cmp(&a.0)));

    let total_live = rows.len();
    let mut shown_ids = BTreeSet::new();
    let mut rendered_obs = Vec::new();

    let base_chars: usize = parts.iter().map(|p| p.chars().count() + 1).sum();
    let mut accum_chars = base_chars + 60;

    for (id, content, observed_at, source_day, has_history) in &rows {
        let date_str = if let Some(day) = source_day {
            chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|_| day.clone())
        } else if *observed_at > 1_000_000_000_000 {
            chrono::DateTime::from_timestamp_millis(*observed_at)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "unknown".to_string())
        } else if *observed_at > 1_000_000_000 {
            chrono::DateTime::from_timestamp(*observed_at, 0)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            "unknown".to_string()
        };
        let revised_str = if *has_history { " [revised]" } else { "" };
        let line = format!("#{id} ({date_str}): {content}{revised_str}");
        let line_chars = line.chars().count() + 1;
        if accum_chars + line_chars <= MAX_ENTITY_CONTEXT_CHARS {
            shown_ids.insert(*id);
            rendered_obs.push(line);
            accum_chars += line_chars;
        } else {
            break;
        }
    }

    parts.push(String::new());
    if total_live == 0 {
        parts.push("Current observations: no observations yet".to_owned());
    } else if rendered_obs.is_empty() {
        parts.push(format!("Current observations: showing 0 of {total_live}"));
    } else {
        parts.push(format!(
            "Current observations (showing {} of {total_live}):",
            rendered_obs.len()
        ));
        for line in rendered_obs {
            parts.push(format!("- {line}"));
        }
    }

    let rendered = parts.join("\n");
    let count = rendered.chars().count();
    if count > MAX_ENTITY_CONTEXT_CHARS {
        return Err(format!(
            "entity packet for {} is {count} characters; maximum is {MAX_ENTITY_CONTEXT_CHARS}",
            entity.entity_id
        ));
    }

    Ok((rendered, shown_ids))
}

pub fn apply_prompt_override(
    talent: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::EntityObserver(state) = state else {
        return Err(stage_error(
            "override",
            "entities:entity_observer",
            talent,
            "expected entity observer state",
        ));
    };
    if let Some(context) = &state.context {
        apply_template_vars(
            &mut talent.config,
            &Map::from_iter([(
                "observer_context".to_owned(),
                Value::String(context.clone()),
            )]),
        );
    }
    Ok(())
}

pub fn parse(
    output: &str,
    talent: &PreparedTalent,
    _state: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    let _data: Value = serde_json::from_str(output).map_err(|error| {
        stage_error(
            "parse",
            "entities:entity_observer",
            talent,
            format!("could not parse output as JSON: {error}"),
        )
    })?;
    Ok(ParsedOutput::Text(output.to_owned()))
}

pub fn commit(
    parsed: ParsedOutput,
    talent: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "entities:entity_observer",
            talent,
            "expected text output",
        ));
    };
    let PrePostState::EntityObserver(state) = state else {
        return Err(stage_error(
            "commit",
            "entities:entity_observer",
            talent,
            "expected entity observer state",
        ));
    };
    let facet = talent
        .config
        .get("facet")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            stage_error(
                "commit",
                "entities:entity_observer",
                talent,
                "missing facet",
            )
        })?;
    let day = talent
        .config
        .get("day")
        .and_then(Value::as_str)
        .ok_or_else(|| stage_error("commit", "entities:entity_observer", talent, "missing day"))?;
    Ok(CommitPlan::Write(WriteIntent::EntityObserver {
        output,
        facet: facet.to_owned(),
        day: day.to_owned(),
        served_ids: state.served_ids.clone(),
        shown_observation_ids: state.shown_observation_ids.clone(),
    }))
}

pub fn prepare_publication(
    journal: &Path,
    output: &str,
    facet: &str,
    day: &str,
    served_ids: &BTreeSet<String>,
    shown_observation_ids: &BTreeMap<String, BTreeSet<u64>>,
    _prepared: &PreparedTalent,
) -> Result<(Vec<solstone_core_facets::PreparedObservationBatch>, Value), String> {
    let mut counts = empty_counts();
    let data: Value = serde_json::from_str(output).map_err(|e| format!("validation: {e}"))?;
    let entries = data
        .get("entities")
        .and_then(Value::as_array)
        .ok_or("validation: observer entities must be an array")?;

    let (attached_ids, candidates) = attached_entities(journal, facet)?;
    let mut combined: BTreeMap<String, Vec<Value>> = BTreeMap::new();

    for entry in entries {
        let Some(entry_obj) = entry.as_object() else {
            continue;
        };
        let ops = entry_obj
            .get("decisions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let Some(id) = entry_obj.get("entity_id").and_then(Value::as_str) else {
            *counts.entry("unselected").or_default() += ops.len();
            continue;
        };
        if !served_ids.contains(id) {
            *counts.entry("unselected").or_default() += ops.len();
            continue;
        }
        if !attached_ids.iter().any(|attached| attached == id) {
            return Err("conflict: served entity is no longer attached".into());
        }
        combined.entry(id.into()).or_default().extend(ops);
    }

    let mut batches = Vec::new();
    let mut inputs = Vec::new();

    let frozen_observation_before = _prepared
        .config
        .get("_daily_observation_before")
        .and_then(Value::as_object);

    for (entity_id, operations) in combined {
        let mut clean = Vec::new();
        let mut seen = Vec::new();
        let empty_set = BTreeSet::new();
        let entity_shown = shown_observation_ids.get(&entity_id).unwrap_or(&empty_set);
        let context = OperationContext {
            entity_id: &entity_id,
            candidates: &candidates,
            shown_ids: entity_shown,
        };

        for raw in operations {
            let (operation, status) = clean_operation(&raw, &mut seen, &context)?;
            if let Some(status) = status {
                *counts.entry(status).or_default() += 1;
            }
            if let Some(operation) = operation {
                clean.push(operation);
            }
        }

        let relationship_dir =
            match solstone_core_facets::resolve_observation_entity_dir(journal, facet, &entity_id)
                .map_err(|e| e.to_string())?
            {
                solstone_core_facets::ObservationEntityResolution::Resolved { entity_dir } => {
                    entity_dir
                }
                solstone_core_facets::ObservationEntityResolution::NoSuchEntity => {
                    return Err("conflict: observation entity disappeared".into());
                }
            };
        let expected = match frozen_observation_before.and_then(|map| map.get(&entity_id)) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Null) => None,
            _ => solstone_core_facets::read_facet_entity_observations(
                journal,
                facet,
                &relationship_dir,
            )
            .map_err(|e| e.to_string())?,
        };

        inputs.push((entity_id, clean, expected));
    }

    for (entity_id, clean, expected) in inputs {
        let batch = solstone_core_facets::prepare_observation_batch(
            journal,
            facet,
            &entity_id,
            &clean,
            Some(day),
        )
        .map_err(|e| match e {
            solstone_core_facets::ObservationWriteError::Conflict { message } => {
                format!("conflict: {message}")
            }
            other => other.to_string(),
        })?;

        if batch.before.as_deref() != expected.as_deref() {
            return Err("conflict: observation changed after prompt preparation".into());
        }
        merge_counts(&mut counts, &batch.counts);
        batches.push(batch);
    }

    let mut outcome = Map::new();
    for (key, count) in counts {
        outcome.insert(key.into(), Value::from(count));
    }
    outcome.insert(
        "served_ids".into(),
        serde_json::to_value(served_ids).map_err(|e| e.to_string())?,
    );
    outcome.insert("error".into(), Value::Null);
    outcome.insert(
        "ts".into(),
        Value::from(chrono::Utc::now().timestamp_millis()),
    );

    Ok((batches, Value::Object(outcome)))
}

fn attached_entities(
    journal: &Path,
    facet: &str,
) -> Result<
    (
        Vec<String>,
        Vec<solstone_core_entity_matching::EntityNameCandidate>,
    ),
    String,
> {
    let scoped = solstone_core_facets::list_scoped_facet_entities(journal, facet, true, true)
        .map_err(|e| e.to_string())?;
    let attached_ids = scoped.iter().map(|e| e.entity_id.clone()).collect();
    let candidates = scoped
        .into_iter()
        .map(|e| solstone_core_entity_matching::EntityNameCandidate {
            id: Some(e.entity_id),
            name: e
                .identity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            aka: e
                .identity
                .get("aka")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            emails: Vec::new(),
        })
        .collect();
    Ok((attached_ids, candidates))
}

pub fn apply_result(
    journal: &Path,
    output: &str,
    facet: &str,
    day: &str,
    served_ids: &BTreeSet<String>,
    shown_observation_ids: &BTreeMap<String, BTreeSet<u64>>,
) -> Result<(), String> {
    let mut counts = empty_counts();
    let mut error = None;
    let result = (|| -> Result<(), String> {
        let Value::Object(data) = serde_json::from_str(output)
            .map_err(|_| "could not parse result as JSON".to_owned())?
        else {
            return Err("result is not a JSON object".to_owned());
        };
        let Some(entries) = data.get("entities").and_then(Value::as_array) else {
            return Err("entities is not a list".to_owned());
        };
        let (attached_ids, candidates) = attached_entities(journal, facet)?;

        let mut served_entries = Vec::new();
        for entry in entries {
            let Some(entry_obj) = entry.as_object() else {
                continue;
            };
            let operations = entry_obj.get("decisions").and_then(Value::as_array);
            let op_len = operations.map_or(0, |ops| ops.len());

            let Some(entity_id) = entry_obj.get("entity_id").and_then(Value::as_str) else {
                *counts.entry("unselected").or_default() += op_len;
                continue;
            };
            if !attached_ids.iter().any(|id| id == entity_id) {
                *counts.entry("unselected").or_default() += op_len;
                continue;
            }
            if !served_ids.contains(entity_id) {
                *counts.entry("unselected").or_default() += op_len;
                continue;
            }
            served_entries.push(entry.clone());
        }

        for entry in &served_entries {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let operations = entry.get("decisions").and_then(Value::as_array);
            let Some(operations) = operations else {
                continue;
            };
            let Some(entity_id) = entry.get("entity_id").and_then(Value::as_str) else {
                continue;
            };
            let mut clean = Vec::new();
            let mut seen = Vec::new();
            let empty_set = BTreeSet::new();
            let entity_shown = shown_observation_ids.get(entity_id).unwrap_or(&empty_set);
            let operation_context = OperationContext {
                entity_id,
                candidates: &candidates,
                shown_ids: entity_shown,
            };
            for operation in operations {
                let (operation, status) =
                    clean_operation(operation, &mut seen, &operation_context)?;
                if let Some(status) = status {
                    *counts.entry(status).or_default() += 1;
                }
                if let Some(operation) = operation {
                    clean.push(operation);
                }
            }
            if clean.is_empty() {
                continue;
            }
            match solstone_core_facets::record_observation_ops_strict(
                journal,
                facet,
                entity_id,
                &clean,
                Some(day),
            ) {
                Ok(op_counts) => merge_counts(&mut counts, &op_counts),
                Err(write_error) => {
                    error = Some(format!("ObservationWriteError: {write_error}"));
                    *counts.entry("refused").or_default() += clean.len();
                }
            }
        }
        Ok(())
    })();
    if let Err(detail) = result {
        error = Some(detail);
    }
    write_outcome(journal, facet, day, &counts, served_ids, error.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_missing_suggestions_artifact_returns_stage_failed() {
        let temp = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(temp.path(), "work", "Work", "", "", "", None).unwrap();
        let mut talent = PreparedTalent {
            name: "entities:entity_observer".into(),
            config: serde_json::json!({"day": "20260910", "facet": "work"})
                .as_object()
                .unwrap()
                .clone(),
        };
        let context = ExecutionContext {
            journal: temp.path().to_path_buf(),
        };
        let err = build(&mut talent, &context).unwrap_err();
        match err {
            RuntimeOutcome::StageFailed(e) => {
                assert!(e.detail.contains("missing suggestions artifact: facets/work/entities/20260910_observer_suggestions.json"));
            }
            other => panic!("expected StageFailed, got {other:?}"),
        }
    }

    #[test]
    fn observer_empty_suggestions_artifact_returns_skipped_no_candidates() {
        let temp = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(temp.path(), "work", "Work", "", "", "", None).unwrap();
        let entities_dir = temp.path().join("facets/work/entities");
        std::fs::create_dir_all(&entities_dir).unwrap();
        std::fs::write(
            entities_dir.join("20260910_observer_suggestions.json"),
            serde_json::json!({
                "facet": "work",
                "day": "20260910",
                "entities": []
            })
            .to_string(),
        )
        .unwrap();

        let mut talent = PreparedTalent {
            name: "entities:entity_observer".into(),
            config: serde_json::json!({"day": "20260910", "facet": "work"})
                .as_object()
                .unwrap()
                .clone(),
        };
        let context = ExecutionContext {
            journal: temp.path().to_path_buf(),
        };
        let res = build(&mut talent, &context);
        match res {
            Err(RuntimeOutcome::Skipped { reason, .. }) => {
                assert_eq!(reason, "no_candidates");
            }
            other => panic!("expected Skipped with no_candidates, got {other:?}"),
        }
    }

    #[test]
    fn observer_build_formats_context_newest_first_with_suggestions() {
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
        solstone_core_facets::add_observation(
            temp.path(),
            "work",
            "ada",
            "First observation",
            Some("20260908"),
            None,
        )
        .unwrap();
        solstone_core_facets::add_observation(
            temp.path(),
            "work",
            "ada",
            "Second observation",
            Some("20260909"),
            None,
        )
        .unwrap();

        let entities_dir = temp.path().join("facets/work/entities");
        std::fs::create_dir_all(&entities_dir).unwrap();
        std::fs::write(
            entities_dir.join("20260910_observer_suggestions.json"),
            serde_json::json!({
                "facet": "work",
                "day": "20260910",
                "entities": [
                    {
                        "entity_id": "ada",
                        "suggestions": [
                            {"content": "New suggestion about analytical engine"}
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let mut talent = PreparedTalent {
            name: "entities:entity_observer".into(),
            config: serde_json::json!({
                "day": "20260910",
                "facet": "work",
                "prompt": "Prompt:\n$observer_context\nEnd"
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
        assert!(prompt.contains("S1: New suggestion about analytical engine"));
        assert!(prompt.contains("#2 (2026-09-09): Second observation"));
        assert!(prompt.contains("#1 (2026-09-08): First observation"));
    }

    #[test]
    fn observer_six_cap_order_and_unselected_count() {
        let temp = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(temp.path(), "work", "Work", "", "", "", None).unwrap();
        // Create 8 entities e1..e8
        let mut entity_entries = Vec::new();
        let mut created_ids = Vec::new();
        for i in 1..=8 {
            let name = format!("entity {:02}", i);
            let res = solstone_core_facets::attach_or_reactivate_entity(
                temp.path(),
                "work",
                "Person",
                &name,
                "Role",
            )
            .unwrap();
            let id = res.relationship["entity_id"].as_str().unwrap().to_string();
            entity_entries.push(serde_json::json!({
                "entity_id": id,
                "suggestions": [{"content": format!("Suggestion for {id}")}]
            }));
            created_ids.push(id);
        }

        let entities_dir = temp.path().join("facets/work/entities");
        std::fs::create_dir_all(&entities_dir).unwrap();
        std::fs::write(
            entities_dir.join("20260910_observer_suggestions.json"),
            serde_json::json!({
                "facet": "work",
                "day": "20260910",
                "entities": entity_entries
            })
            .to_string(),
        )
        .unwrap();

        let mut talent = PreparedTalent {
            name: "entities:entity_observer".into(),
            config: serde_json::json!({
                "day": "20260910",
                "facet": "work",
                "prompt": "$observer_context"
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let context = ExecutionContext {
            journal: temp.path().to_path_buf(),
        };
        let state = build(&mut talent, &context).unwrap();
        let PrePostState::EntityObserver(ref obs_state) = state else {
            panic!("expected EntityObserver state");
        };
        // Exactly 6 entities served
        assert_eq!(obs_state.served_ids.len(), 6);
        assert!(obs_state.served_ids.contains(&created_ids[0]));
        assert!(obs_state.served_ids.contains(&created_ids[5]));
        assert!(!obs_state.served_ids.contains(&created_ids[6]));
        assert!(!obs_state.served_ids.contains(&created_ids[7]));

        // When model responds for entity 0 AND entity 6, entity 6 is counted unselected
        let output = serde_json::json!({
            "entities": [
                {
                    "entity_id": created_ids[0],
                    "decisions": [{"op": "add", "content": "Fact 1"}]
                },
                {
                    "entity_id": created_ids[6],
                    "decisions": [{"op": "add", "content": "Fact 7"}]
                }
            ]
        })
        .to_string();

        let (batches, outcome) = prepare_publication(
            temp.path(),
            &output,
            "work",
            "20260910",
            &obs_state.served_ids,
            &obs_state.shown_observation_ids,
            &talent,
        )
        .unwrap();

        assert_eq!(batches.len(), 1);
        assert_eq!(outcome["add"], 1);
        assert_eq!(outcome["unselected"], 1);
    }

    #[test]
    fn observer_refuses_target_id_not_in_shown_ids() {
        let temp = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(temp.path(), "work", "Work", "", "", "", None).unwrap();
        solstone_core_facets::attach_or_reactivate_entity(
            temp.path(),
            "work",
            "Person",
            "Ada",
            "Role",
        )
        .unwrap();
        solstone_core_facets::add_observation(
            temp.path(),
            "work",
            "ada",
            "Existing live fact",
            Some("20260908"),
            None,
        )
        .unwrap();

        let mut served_ids = BTreeSet::new();
        served_ids.insert("ada".to_string());
        let mut shown_observation_ids = BTreeMap::new();
        // Suppose shown_observation_ids has id 1, but model attempts to target id 99 (not shown)
        shown_observation_ids.insert("ada".to_string(), BTreeSet::from([1]));

        let output = serde_json::json!({
            "entities": [
                {
                    "entity_id": "ada",
                    "decisions": [
                        {
                            "op": "replace",
                            "target_id": 99,
                            "target_quote": "Existing live fact",
                            "content": "Replaced fact"
                        }
                    ]
                }
            ]
        })
        .to_string();

        let talent = PreparedTalent {
            name: "entities:entity_observer".into(),
            config: serde_json::json!({
                "day": "20260910",
                "facet": "work"
            })
            .as_object()
            .unwrap()
            .clone(),
        };

        let (batches, outcome) = prepare_publication(
            temp.path(),
            &output,
            "work",
            "20260910",
            &served_ids,
            &shown_observation_ids,
            &talent,
        )
        .unwrap();

        assert_eq!(batches.len(), 1);
        assert_eq!(outcome["refused"], 1);
        assert_eq!(outcome["replace"], 0);
    }

    #[test]
    fn observer_ignores_legacy_operations_key() {
        let temp = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(temp.path(), "work", "Work", "", "", "", None).unwrap();
        solstone_core_facets::attach_or_reactivate_entity(
            temp.path(),
            "work",
            "Person",
            "Ada",
            "Role",
        )
        .unwrap();

        let mut served_ids = BTreeSet::new();
        served_ids.insert("ada".to_string());
        let shown_observation_ids = BTreeMap::new();

        let output = serde_json::json!({
            "entities": [
                {
                    "entity_id": "ada",
                    "operations": [
                        {
                            "op": "add",
                            "content": "Fact with operations key"
                        }
                    ]
                }
            ]
        })
        .to_string();

        let talent = PreparedTalent {
            name: "entities:entity_observer".into(),
            config: serde_json::json!({
                "day": "20260910",
                "facet": "work"
            })
            .as_object()
            .unwrap()
            .clone(),
        };

        let (batches, outcome) = prepare_publication(
            temp.path(),
            &output,
            "work",
            "20260910",
            &served_ids,
            &shown_observation_ids,
            &talent,
        )
        .unwrap();

        assert_eq!(batches.len(), 1);
        assert_eq!(outcome["add"], 0);
        assert_eq!(outcome["refused"], 0);
    }
}
