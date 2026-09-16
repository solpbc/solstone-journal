// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-observer hook.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EntityBudgetExclusion {
    pub entity_id: String,
    pub chars: usize,
}

enum EntityPacketOutcome {
    Rendered(String),
    BudgetExcluded(EntityBudgetExclusion),
}

struct ObserverContextAssembly {
    context: String,
    served_ids: BTreeSet<String>,
    exclusions: Vec<EntityBudgetExclusion>,
    observation_before: Map<String, Value>,
    resolution: ObserverResolutionSnapshot,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObserverState {
    pub context: Option<String>,
    pub served_ids: BTreeSet<String>,
    pub exclusions: Vec<EntityBudgetExclusion>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ObserverResolutionCandidate {
    id: Option<String>,
    name: String,
    aka: Vec<String>,
    emails: Vec<String>,
    blocked: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ObserverResolutionSnapshot {
    entities: Vec<ObserverResolutionCandidate>,
    choices: Vec<Value>,
}

impl ObserverResolutionSnapshot {
    fn entities(&self) -> Vec<solstone_core_entity::EntityResolutionEntity> {
        self.entities
            .iter()
            .map(|entity| solstone_core_entity::EntityResolutionEntity {
                id: entity.id.clone(),
                name: entity.name.clone(),
                aka: entity.aka.clone(),
                emails: entity.emails.clone(),
                blocked: entity.blocked,
            })
            .collect()
    }
}

type Counts = BTreeMap<&'static str, usize>;

const MAX_ACTIVE_ENTITIES: usize = 6;
const MAX_SOURCE_SEGMENTS: usize = 3;
const MAX_SEGMENT_CONTEXT_CHARS: usize = 800;
const MAX_ENTITY_CONTEXT_CHARS: usize = 3_800;
const MAX_OBSERVER_CONTEXT_CHARS: usize = 24_000;

#[cfg(test)]
const HEADER_ALLOWANCE: usize = MAX_OBSERVER_CONTEXT_CHARS
    - MAX_ACTIVE_ENTITIES * MAX_ENTITY_CONTEXT_CHARS
    - (MAX_ACTIVE_ENTITIES - 1) * 7;

fn empty_counts() -> Counts {
    BTreeMap::from([
        ("update", 0),
        ("add", 0),
        ("drop", 0),
        ("keep", 0),
        ("skipped", 0),
        ("relation_unresolved", 0),
        ("excluded", 0),
        ("unselected", 0),
    ])
}

fn write_outcome(
    journal: &Path,
    facet: &str,
    day: &str,
    counts: &Counts,
    exclusions: &[EntityBudgetExclusion],
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
        "exclusions".to_owned(),
        Value::Array(
            exclusions
                .iter()
                .map(|exclusion| {
                    json!({
                        "entity_id": exclusion.entity_id,
                        "chars": exclusion.chars,
                    })
                })
                .collect(),
        ),
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

fn target_index(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_i64)
        .filter(|_| !value.is_some_and(Value::is_boolean))
}

fn target_quote(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn attached_entities(
    journal: &Path,
    facet: &str,
) -> Result<
    (
        Vec<String>,
        Vec<solstone_core_entity::EntityResolutionEntity>,
    ),
    String,
> {
    let scoped = solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false)
        .map_err(|error| error.to_string())?;
    let ids = scoped
        .iter()
        .map(|entity| entity.entity_id.clone())
        .collect::<Vec<_>>();
    let entities = scoped
        .into_iter()
        .map(|entity| solstone_core_entity::EntityResolutionEntity {
            id: Some(entity.entity_id),
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
            blocked: entity.blocked,
        })
        .collect();
    Ok((ids, entities))
}

fn clean_relation(
    value: Option<&Value>,
    op: &str,
    context: &OperationContext<'_>,
) -> Result<(Option<Value>, Option<&'static str>), String> {
    if value.is_none() || value.is_some_and(Value::is_null) || matches!(op, "drop" | "keep") {
        return Ok((None, None));
    }
    let Some(value) = value.and_then(Value::as_object) else {
        return Ok((None, Some("skipped")));
    };
    let (Some(kind), Some(target_name), Some(note)) = (
        value.get("kind").and_then(Value::as_str),
        value.get("target_name").and_then(Value::as_str),
        value.get("note").and_then(Value::as_str),
    ) else {
        return Ok((None, Some("skipped")));
    };
    if !crate::story::RELATIONS.contains(&kind) || (kind == "other" && note.trim().is_empty()) {
        return Ok((None, Some("skipped")));
    }
    let OperationContext {
        journal,
        facet,
        day,
        entity_id,
        entities,
        read_only,
    } = context;
    let resolution = solstone_core_entity::record_entity_resolution(
        journal,
        target_name,
        entities,
        json!({"kind":"facet","facet":facet}),
        json!({"lane":"apps.entities.entity_observer","facet":facet,"day":day,"record_id":entity_id,"field":"relation.target_name"}),
        90.0,
        *read_only,
    )
    .map_err(|error| error.to_string())?;
    let target_entity_id = (resolution.outcome
        == solstone_core_entity::EntityResolutionOutcome::Resolved)
        .then(|| {
            resolution
                .entity_index
                .and_then(|index| entities[index].id.clone())
        })
        .flatten();
    let status = target_entity_id.is_none().then_some("relation_unresolved");
    Ok((
        Some(
            json!({"kind":kind,"target_entity_id":target_entity_id,"target_name":target_name,"note":note}),
        ),
        status,
    ))
}

struct OperationContext<'a> {
    read_only: bool,
    journal: &'a Path,
    facet: &'a str,
    day: &'a str,
    entity_id: &'a str,
    entities: &'a [solstone_core_entity::EntityResolutionEntity],
}

fn clean_operation(
    item: &Value,
    seen_indexes: &mut Vec<i64>,
    context: &OperationContext<'_>,
) -> Result<(Option<Value>, Option<&'static str>), String> {
    let Some(item) = item.as_object() else {
        return Ok((None, Some("skipped")));
    };
    let Some(op) = item.get("op").and_then(Value::as_str) else {
        return Ok((None, Some("skipped")));
    };
    if op == "add" {
        let Some(content) = item
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return Ok((None, Some("skipped")));
        };
        let (relation, status) = clean_relation(item.get("relation"), op, context)?;
        if status == Some("skipped") {
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
    if !matches!(op, "update" | "drop" | "keep") {
        return Ok((None, Some("skipped")));
    }
    let Some(index) = target_index(item.get("target_index")) else {
        return Ok((None, Some("skipped")));
    };
    if seen_indexes.contains(&index) {
        return Ok((None, Some("skipped")));
    }
    let quote_value = item.get("target_quote");
    if quote_value.is_some() && quote_value.and_then(Value::as_str).is_none() {
        return Ok((None, Some("skipped")));
    }
    let content = if op == "update" {
        let Some(content) = item
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return Ok((None, Some("skipped")));
        };
        Some(content)
    } else {
        None
    };
    seen_indexes.push(index);
    let (relation, status) = clean_relation(item.get("relation"), op, context)?;
    if status == Some("skipped") {
        return Ok((None, status));
    }
    let mut clean = Map::from_iter([
        ("op".to_owned(), Value::String(op.to_owned())),
        ("target_index".to_owned(), Value::from(index)),
    ]);
    if let Some(quote) = target_quote(quote_value) {
        clean.insert("target_quote".to_owned(), Value::String(quote));
    }
    if let Some(content) = content {
        clean.insert("content".to_owned(), Value::String(content.to_owned()));
    }
    if let Some(relation) = relation {
        clean.insert("relation".to_owned(), relation);
    }
    Ok((Some(Value::Object(clean)), status))
}

fn merge_counts(counts: &mut Counts, source: &solstone_core_facets::ObservationOperationCounts) {
    for (name, count) in [
        ("update", source.update),
        ("add", source.add),
        ("drop", source.drop),
        ("keep", source.keep),
        ("skipped", source.skipped),
    ] {
        *counts.entry(name).or_default() += count;
    }
}

fn preflight_observation_snapshots(
    journal: &Path,
    facet: &str,
    entries: &[Value],
    attached_ids: &[String],
) -> Result<(), String> {
    let mut checked = BTreeSet::new();
    for entry in entries {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(entity_id) = entry.get("entity_id").and_then(Value::as_str) else {
            continue;
        };
        if !attached_ids.iter().any(|attached| attached == entity_id)
            || !entry
                .get("operations")
                .and_then(Value::as_array)
                .is_some_and(|operations| !operations.is_empty())
            || !checked.insert(entity_id.to_owned())
        {
            continue;
        }
        let relationship_dir =
            match solstone_core_facets::resolve_observation_entity_dir(journal, facet, entity_id)
                .map_err(|error| error.to_string())?
            {
                solstone_core_facets::ObservationEntityResolution::Resolved { entity_dir } => {
                    entity_dir
                }
                solstone_core_facets::ObservationEntityResolution::NoSuchEntity => {
                    entity_id.to_owned()
                }
            };
        solstone_core_facets::read_live_observations(
            journal,
            facet,
            &relationship_dir,
            solstone_core_facets::ObservationReadQuery::default(),
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn gate(prepared: &PreparedTalent, _: &ExecutionContext) -> Result<GateDecision, StageError> {
    let day = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .filter(|day| !day.is_empty());
    if day.is_none() {
        return Ok(GateDecision::Skip("no_day".to_owned()));
    }
    let facet = prepared
        .config
        .get("facet")
        .and_then(Value::as_str)
        .filter(|facet| !facet.is_empty());
    if facet.is_none() {
        return Ok(GateDecision::Skip("no_facet".to_owned()));
    }
    Ok(GateDecision::Proceed)
}

fn skip_missing_scope(prepared: &PreparedTalent) -> RuntimeOutcome {
    let has_day = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .is_some_and(|day| !day.is_empty());
    RuntimeOutcome::Skipped {
        stage: "entities:entity_observer".to_owned(),
        talent: prepared.name.clone(),
        reason: if has_day {
            "no_facet".to_owned()
        } else {
            "no_day".to_owned()
        },
    }
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let facet = prepared.config.get("facet").and_then(Value::as_str);
    let day = prepared.config.get("day").and_then(Value::as_str);
    let (context_str, served_ids, exclusions) = match (facet, day) {
        (Some(facet), Some(day)) if !facet.is_empty() && !day.is_empty() => {
            let assembly =
                assemble_observer_context(&context.journal, facet, day).map_err(|detail| {
                    RuntimeOutcome::StageFailed(stage_error(
                        "build",
                        "entities:entity_observer",
                        prepared,
                        detail,
                    ))
                })?;
            prepared.config.insert(
                "_daily_observer_resolution".to_owned(),
                serde_json::to_value(&assembly.resolution).expect("observer snapshot serializes"),
            );
            prepared.config.insert(
                "_daily_observation_before".to_owned(),
                Value::Object(assembly.observation_before),
            );
            (
                Some(assembly.context),
                assembly.served_ids,
                assembly.exclusions,
            )
        }
        _ => return Err(skip_missing_scope(prepared)),
    };
    Ok(PrePostState::EntityObserver(ObserverState {
        context: context_str,
        served_ids,
        exclusions,
    }))
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::EntityObserver(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "entities:entity_observer",
            prepared,
            "missing observer state",
        ));
    };
    if let Some(context) = &state.context {
        apply_template_vars(
            &mut prepared.config,
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
    _: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.to_owned()))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "entities:entity_observer",
            prepared,
            "expected text output",
        ));
    };
    let (Some(facet), Some(day), PrePostState::EntityObserver(state)) = (
        prepared.config.get("facet").and_then(Value::as_str),
        prepared.config.get("day").and_then(Value::as_str),
        state,
    ) else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::EntityObserver {
        output,
        facet: facet.to_owned(),
        day: day.to_owned(),
        served_ids: state.served_ids.clone(),
        exclusions: state.exclusions.clone(),
    }))
}

pub fn prepare_publication(
    journal: &Path,
    output: &str,
    facet: &str,
    day: &str,
    served_ids: &BTreeSet<String>,
    exclusions: &[EntityBudgetExclusion],
    prepared: &PreparedTalent,
) -> Result<(Vec<solstone_core_facets::PreparedObservationBatch>, Value), String> {
    let data: Value = serde_json::from_str(output).map_err(|e| e.to_string())?;
    let entries = data
        .get("entities")
        .and_then(Value::as_array)
        .ok_or("entities is not a list")?;
    let frozen_before = prepared
        .config
        .get("_daily_observation_before")
        .and_then(Value::as_object)
        .ok_or("missing frozen observation snapshots")?;
    let frozen: ObserverResolutionSnapshot = serde_json::from_value(
        prepared
            .config
            .get("_daily_observer_resolution")
            .cloned()
            .ok_or("missing frozen observer resolution")?,
    )
    .map_err(|e| format!("invalid frozen observer resolution: {e}"))?;
    let entities = frozen.entities();
    let _trust = solstone_core_facets::hold_facet_trust_lock(journal).map_err(|e| e.to_string())?;
    let (attached_ids, live_entities) = attached_entities(journal, facet)?;
    let mut counts = empty_counts();
    let mut batches = Vec::new();
    let mut combined: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    for entry in entries {
        let Some(ops) = entry.get("operations").and_then(Value::as_array) else {
            continue;
        };
        let Some(id) = entry.get("entity_id").and_then(Value::as_str) else {
            *counts.entry("skipped").or_default() += ops.len();
            continue;
        };
        if exclusions.iter().any(|ex| ex.entity_id == id) {
            *counts.entry("excluded").or_default() += ops.len();
            continue;
        }
        if !served_ids.contains(id) {
            *counts.entry("unselected").or_default() += ops.len();
            continue;
        }
        if !attached_ids.iter().any(|attached| attached == id) {
            return Err("conflict: served entity is no longer attached".into());
        }
        combined
            .entry(id.into())
            .or_default()
            .extend(ops.iter().cloned());
    }
    let mut inputs = Vec::new();
    for (entity_id, operations) in combined {
        let mut clean = Vec::new();
        let mut seen = Vec::new();
        let context = OperationContext {
            read_only: true,
            journal,
            facet,
            day,
            entity_id: &entity_id,
            entities: &entities,
        };
        let live_context = OperationContext {
            entities: &live_entities,
            ..context
        };
        for raw in operations {
            // The resolver also honors persisted choices. Refuse a changed
            // choice before invoking it against the frozen candidate set.
            if matches!(
                raw.get("op").and_then(Value::as_str),
                Some("add" | "update")
            ) && let Some(query) = raw
                .get("relation")
                .and_then(|r| r.get("target_name"))
                .and_then(Value::as_str)
            {
                let normalized = solstone_core_entity_matching::normalize_resolution_query(query);
                let expected = frozen.choices.iter().find(|row| {
                    row.get("normalized_query").and_then(Value::as_str) == Some(normalized.as_str())
                });
                let current = solstone_core_entity::load_resolved_ambiguity_choice(
                    journal,
                    &json!({"kind":"facet", "facet":facet}),
                    &normalized,
                )
                .map_err(|e| e.to_string())?;
                if current.as_ref() != expected {
                    return Err(
                        "conflict: observer relation choice changed after prompt preparation"
                            .into(),
                    );
                }
            }
            let (operation, status) = clean_operation(&raw, &mut seen, &context)?;
            if let Some(status) = status {
                *counts.entry(status).or_default() += 1;
            }
            if let Some(operation) = operation {
                if let Some(relation) = operation.get("relation") {
                    let (live, _) = clean_relation(
                        raw.get("relation"),
                        operation["op"].as_str().unwrap_or_default(),
                        &live_context,
                    )
                    .map_err(|e| format!("conflict: observer relation target changed: {e}"))?;
                    if live.as_ref() != Some(relation) {
                        return Err(
                            "conflict: observer relation target changed after prompt preparation"
                                .into(),
                        );
                    }
                }
                clean.push(operation);
            }
        }
        if clean.is_empty() {
            continue;
        }
        let expected = frozen_before
            .get(&entity_id)
            .ok_or("missing served observation snapshot")?;
        let expected = match expected {
            Value::Null => None,
            Value::String(text) => Some(text.as_str()),
            _ => return Err("invalid served observation snapshot".into()),
        };
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
        let current =
            solstone_core_facets::read_facet_entity_observations(journal, facet, &relationship_dir)
                .map_err(|e| e.to_string())?;
        if current.as_deref() != expected {
            return Err("conflict: observation changed after prompt preparation".into());
        }
        inputs.push((entity_id, clean, expected));
    }
    // All owner before-images must still match before any model reference is
    // classified as invalid. A later entity's owner edit must not be hidden by
    // an earlier entity's bad model quote.
    for (entity_id, clean, expected) in inputs {
        solstone_core_facets::validate_observation_operations(expected, &clean, Some(day))
            .map_err(|e| match e {
                solstone_core_facets::ObservationWriteError::Conflict { message } => {
                    format!("validation: {message}")
                }
                other => other.to_string(),
            })?;
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
        if batch.before.as_deref() != expected {
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
    outcome.insert(
        "exclusions".into(),
        serde_json::to_value(exclusions).map_err(|e| e.to_string())?,
    );
    outcome.insert("error".into(), Value::Null);
    outcome.insert(
        "ts".into(),
        Value::from(chrono::Utc::now().timestamp_millis()),
    );
    Ok((batches, Value::Object(outcome)))
}

pub fn apply_result(
    journal: &Path,
    output: &str,
    facet: &str,
    day: &str,
    served_ids: &BTreeSet<String>,
    exclusions: &[EntityBudgetExclusion],
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
        let (attached_ids, entities) = attached_entities(journal, facet)?;

        let mut served_entries = Vec::new();
        for entry in entries {
            let Some(entry_obj) = entry.as_object() else {
                continue;
            };
            let operations = entry_obj.get("operations").and_then(Value::as_array);
            let op_len = operations.map_or(0, |ops| ops.len());

            let Some(entity_id) = entry_obj.get("entity_id").and_then(Value::as_str) else {
                *counts.entry("skipped").or_default() += op_len;
                continue;
            };
            if !attached_ids.iter().any(|id| id == entity_id) {
                *counts.entry("skipped").or_default() += op_len;
                continue;
            }
            if exclusions.iter().any(|ex| ex.entity_id == entity_id) {
                *counts.entry("excluded").or_default() += op_len;
                continue;
            }
            if !served_ids.contains(entity_id) {
                *counts.entry("unselected").or_default() += op_len;
                continue;
            }
            served_entries.push(entry.clone());
        }

        preflight_observation_snapshots(journal, facet, &served_entries, &attached_ids)?;

        for entry in &served_entries {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let operations = entry.get("operations").and_then(Value::as_array);
            let Some(operations) = operations else {
                continue;
            };
            let Some(entity_id) = entry.get("entity_id").and_then(Value::as_str) else {
                continue;
            };
            let mut clean = Vec::new();
            let mut seen = Vec::new();
            let operation_context = OperationContext {
                read_only: false,
                journal,
                facet,
                day,
                entity_id,
                entities: &entities,
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
                    *counts.entry("skipped").or_default() += clean.len();
                }
            }
        }
        Ok(())
    })();
    if let Err(detail) = result {
        error = Some(detail);
    }
    write_outcome(journal, facet, day, &counts, exclusions, error.as_deref())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SegmentOrigin {
    label: String,
    segment: String,
    stream: String,
}

fn valid_segment_key(value: &str) -> bool {
    let Some((_, duration)) = value.split_once('_') else {
        return false;
    };
    solstone_core_format::segment::segment_parse(value).is_some()
        && duration.parse::<u64>().is_ok_and(|duration| duration > 0)
}

fn parse_segment_origin(label: &str, day: &str) -> Result<SegmentOrigin, &'static str> {
    let parts = label.split('/').collect::<Vec<_>>();
    let (origin_day, stream, segment) = match parts.as_slice() {
        [origin_day, segment] => (
            *origin_day,
            solstone_core_journal_io::DEFAULT_STREAM,
            *segment,
        ),
        [origin_day, stream, segment] if !stream.is_empty() => (*origin_day, *stream, *segment),
        _ => return Err("unsupported origin shape"),
    };
    if origin_day != day {
        return Err("origin belongs to another day");
    }
    if stream != solstone_core_journal_io::DEFAULT_STREAM
        && solstone_core_journal_io::StreamName::parse(stream).is_err()
    {
        return Err("invalid stream name");
    }
    if !valid_segment_key(segment) {
        return Err("invalid segment key");
    }
    Ok(SegmentOrigin {
        label: label.to_owned(),
        segment: segment.to_owned(),
        stream: stream.to_owned(),
    })
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    const MARKER: &str = "\n[truncated]";
    let keep = maximum.saturating_sub(MARKER.chars().count());
    value
        .chars()
        .take(keep)
        .chain(MARKER.chars().take(maximum.saturating_sub(keep)))
        .collect()
}

fn sense_evidence(segment_dir: &Path, matching_slugs: &BTreeSet<String>) -> Vec<String> {
    let path = segment_dir.join("talents/sense.json");
    if !path.exists() {
        return vec!["- Sense: unavailable (missing)".to_owned()];
    }
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => return vec![format!("- Sense: unavailable (read error: {error})")],
    };
    let sense = match serde_json::from_str::<Value>(&contents) {
        Ok(Value::Object(sense)) => sense,
        _ => return vec!["- Sense: unavailable (malformed JSON object)".to_owned()],
    };
    let mut lines = Vec::new();
    match sense.get("activity_summary").and_then(Value::as_str) {
        Some(summary) if !summary.trim().is_empty() => {
            lines.push(format!("- Segment activity summary: {}", summary.trim()));
        }
        _ => lines.push("- Segment activity summary: unavailable".to_owned()),
    }
    let contexts = sense
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .map(solstone_core_entity_matching::entity_slug)
                .is_some_and(|slug| matching_slugs.contains(&slug))
        })
        .filter_map(|row| row.get("context").and_then(Value::as_str))
        .map(str::trim)
        .filter(|context| !context.is_empty())
        .collect::<BTreeSet<_>>();
    if contexts.is_empty() {
        lines.push("- Matching sense context: unavailable".to_owned());
    } else {
        lines.extend(
            contexts
                .into_iter()
                .map(|context| format!("- Matching sense context: {context}")),
        );
    }
    lines
}

fn strip_modality_metadata(source: &str) -> String {
    let mut after_heading = false;
    let mut lines = Vec::new();
    for line in source.lines() {
        if line.starts_with("### ") {
            after_heading = true;
            lines.push(line);
            continue;
        }
        if after_heading && line.starts_with("Start: ") {
            after_heading = false;
            continue;
        }
        if !line.trim().is_empty() {
            after_heading = false;
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn segment_evidence(
    journal: &Path,
    day: &str,
    origin: &SegmentOrigin,
    matching_slugs: &BTreeSet<String>,
) -> String {
    let mut lines = vec![format!("##### Source origin: {}", origin.label)];
    let Some(segment_dir) = solstone_core_system_health::find_segment_dir(
        journal,
        day,
        &origin.segment,
        Some(&origin.stream),
    ) else {
        lines.push("- Source: unavailable (segment missing)".to_owned());
        return truncate_chars(&lines.join("\n"), MAX_SEGMENT_CONTEXT_CHARS);
    };
    lines.extend(sense_evidence(&segment_dir, matching_slugs));
    let source_config = Map::from_iter([
        ("transcripts".to_owned(), Value::Bool(true)),
        ("percepts".to_owned(), Value::Bool(true)),
        ("talents".to_owned(), Value::Bool(false)),
    ]);
    let (source, counts) = crate::transcript::load_segment_transcript(
        journal,
        day,
        &origin.segment,
        Some(&origin.stream),
        &source_config,
    );
    if counts.total() == 0 {
        lines.push("- Transcript/percept evidence: unavailable or empty".to_owned());
    } else {
        lines.push(format!(
            "- Source records: {} transcript, {} percept",
            counts.transcripts, counts.percepts
        ));
        lines.push(strip_modality_metadata(&source));
    }
    truncate_chars(&lines.join("\n"), MAX_SEGMENT_CONTEXT_CHARS)
}

fn render_entity_packet(
    journal: &Path,
    facet: &str,
    day: &str,
    entity: &solstone_core_facets::ScopedFacetEntity,
    rows: &[Value],
) -> Result<EntityPacketOutcome, String> {
    let name = entity
        .identity
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&entity.entity_id);
    let mut matching_slugs = BTreeSet::from([solstone_core_entity_matching::entity_slug(name)]);
    let mut summaries = BTreeSet::new();
    let mut evidence_status = Vec::new();
    let mut origins = BTreeMap::<String, SegmentOrigin>::new();
    for (row_index, row) in rows.iter().enumerate() {
        let row_number = row_index + 1;
        if let Some(row_name) = row.get("name").and_then(Value::as_str) {
            matching_slugs.insert(solstone_core_entity_matching::entity_slug(row_name));
        }
        match row.get("description").and_then(Value::as_str) {
            Some(summary) if !summary.trim().is_empty() => {
                summaries.insert(summary.trim().to_owned());
            }
            _ => evidence_status.push(format!("detection row {row_number}: summary unavailable")),
        }
        match row.get("segments") {
            None => evidence_status.push(format!(
                "detection row {row_number}: no linked source origins"
            )),
            Some(Value::Array(segments)) => {
                for (origin_index, value) in segments.iter().enumerate() {
                    let Some(label) = value.as_str().filter(|label| !label.trim().is_empty())
                    else {
                        evidence_status.push(format!(
                            "detection row {row_number} origin {}: rejected non-string or blank origin",
                            origin_index + 1
                        ));
                        continue;
                    };
                    match parse_segment_origin(label, day) {
                        Ok(origin) => {
                            origins.entry(origin.label.clone()).or_insert(origin);
                        }
                        Err(reason) => evidence_status.push(format!(
                            "detection row {row_number} origin {}: rejected ({reason})",
                            json!(label)
                        )),
                    }
                }
            }
            Some(_) => evidence_status.push(format!(
                "detection row {row_number}: rejected non-array segments field"
            )),
        }
    }
    let mut selected_origins = origins.into_values().collect::<Vec<_>>();
    selected_origins.sort_by(|left, right| {
        right
            .segment
            .cmp(&left.segment)
            .then_with(|| right.label.cmp(&left.label))
    });
    let origin_total = selected_origins.len();
    selected_origins.truncate(MAX_SOURCE_SEGMENTS);
    if origin_total > MAX_SOURCE_SEGMENTS {
        evidence_status.push(format!(
            "selected newest {MAX_SOURCE_SEGMENTS} of {origin_total} valid origins"
        ));
    }
    if selected_origins.is_empty() {
        evidence_status.push("no usable source origins".to_owned());
    } else {
        evidence_status.push(format!(
            "{} source origin(s) selected",
            selected_origins.len()
        ));
    }

    let mut lines = vec![
        format!("#### {name} ({})", entity.entity_id),
        format!(
            "- Type: {}",
            entity
                .identity
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
        format!(
            "- Description: {}",
            entity
                .identity
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
        format!("- Evidence status: {}", evidence_status.join("; ")),
        String::new(),
        "Detection summaries:".to_owned(),
    ];
    if summaries.is_empty() {
        lines.push("No usable detection summaries.".to_owned());
    } else {
        lines.extend(summaries.into_iter().map(|summary| format!("- {summary}")));
    }
    lines.extend([String::new(), "Fresh source evidence:".to_owned()]);
    if selected_origins.is_empty() {
        lines.push("No source excerpts available.".to_owned());
    } else {
        for origin in selected_origins {
            lines.push(segment_evidence(journal, day, &origin, &matching_slugs));
        }
    }
    lines.extend([String::new(), "Current observations:".to_owned()]);
    let page = solstone_core_facets::read_live_observations(
        journal,
        facet,
        &entity.relationship_dir,
        solstone_core_facets::ObservationReadQuery {
            order: solstone_core_facets::ObservationReadOrder::Oldest,
            limit: 200,
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())?;
    let observations = page.items;
    if observations.is_empty() {
        lines.push("No current observations.".to_owned());
    } else {
        for (index, observation) in observations.iter().enumerate() {
            let rendered = observation_context(index, observation).ok_or_else(|| {
                format!(
                    "strict observation row for {} could not be rendered",
                    entity.entity_id
                )
            })?;
            lines.push(rendered);
        }
    }
    let packet = lines.join("\n");
    let packet_chars = packet.chars().count();
    if packet_chars > MAX_ENTITY_CONTEXT_CHARS {
        return Ok(EntityPacketOutcome::BudgetExcluded(EntityBudgetExclusion {
            entity_id: entity.entity_id.clone(),
            chars: packet_chars,
        }));
    }
    Ok(EntityPacketOutcome::Rendered(packet))
}

fn assemble_observer_context(
    journal: &Path,
    facet: &str,
    day: &str,
) -> Result<ObserverContextAssembly, String> {
    let _trust = solstone_core_facets::hold_facet_trust_lock(journal).map_err(|e| e.to_string())?;
    // Relation targets include all attached entities, even entities omitted
    // from the bounded observation packet.
    let (_, attached) = attached_entities(journal, facet)?;
    let scope = json!({"kind":"facet", "facet":facet});
    let choices = solstone_core_entity::read_ambiguities(
        journal,
        solstone_core_journal_io::MalformedPolicy::Raise,
    )
    .map_err(|e| e.to_string())?
    .into_iter()
    .filter(|row| {
        row.get("scope") == Some(&scope)
            && row.get("status").and_then(Value::as_str) == Some("resolved")
    })
    .collect();
    let resolution = ObserverResolutionSnapshot {
        entities: attached
            .into_iter()
            .map(|entity| ObserverResolutionCandidate {
                id: entity.id,
                name: entity.name,
                aka: entity.aka,
                emails: entity.emails,
                blocked: entity.blocked,
            })
            .collect(),
        choices,
    };
    let scoped = solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false)
        .map_err(|error| error.to_string())?;
    let detected = solstone_core_facets::read_detected_entities_strict(journal, facet, day)
        .map_err(|error| error.to_string())?;
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
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .expect("strict detected rows have names");
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
        return Ok(ObserverContextAssembly {
            context: "No active entities found for this day.".to_owned(),
            served_ids: BTreeSet::new(),
            exclusions: Vec::new(),
            observation_before: Map::new(),
            resolution,
        });
    }
    let total = active.len();
    let selected = active
        .into_values()
        .take(MAX_ACTIVE_ENTITIES)
        .collect::<Vec<_>>();
    let mut sections = Vec::with_capacity(selected.len());
    let mut served_ids = BTreeSet::new();
    let mut exclusions = Vec::new();
    let mut observation_before = Map::new();
    for (entity, rows) in &selected {
        let before = solstone_core_facets::read_facet_entity_observations(
            journal,
            facet,
            &entity.relationship_dir,
        )
        .map_err(|e| e.to_string())?;
        let rendered = render_entity_packet(journal, facet, day, entity, rows)?;
        let after = solstone_core_facets::read_facet_entity_observations(
            journal,
            facet,
            &entity.relationship_dir,
        )
        .map_err(|e| e.to_string())?;
        if before != after {
            return Err(format!(
                "observations changed during preparation for {}",
                entity.entity_id
            ));
        }
        match rendered {
            EntityPacketOutcome::Rendered(packet) => {
                observation_before.insert(
                    entity.entity_id.clone(),
                    before.map(Value::String).unwrap_or(Value::Null),
                );
                served_ids.insert(entity.entity_id.clone());
                sections.push(packet);
            }
            EntityPacketOutcome::BudgetExcluded(exclusion) => {
                exclusions.push(exclusion);
            }
        }
    }
    let mut header_elements = vec![
        "# Entity Observer Context".to_owned(),
        String::new(),
        format!("## Facet: {facet}"),
        format!("## Day: {day}"),
        format!("## Active Entities: {} of {total} active", sections.len()),
    ];
    if !exclusions.is_empty() {
        header_elements.push(format!(
            "## Budget Exclusions: {} active entities exceeded character budget",
            exclusions.len()
        ));
    }
    header_elements.extend([String::new(), "### Entities".to_owned(), String::new()]);
    let mut context = header_elements.join("\n");
    if sections.is_empty() {
        context.push_str("All active entities were excluded due to character limits.");
    } else {
        context.push_str(&sections.join("\n\n---\n\n"));
    }
    let context_chars = context.chars().count();
    if context_chars > MAX_OBSERVER_CONTEXT_CHARS {
        return Err(format!(
            "entity observer context is {context_chars} characters; maximum is {MAX_OBSERVER_CONTEXT_CHARS}"
        ));
    }
    Ok(ObserverContextAssembly {
        context,
        served_ids,
        exclusions,
        observation_before,
        resolution,
    })
}

// Keep the identifying quote distinct from both the full content and provenance.
// The writer still verifies it against the current observation at this index.
fn observation_context(
    index: usize,
    observation: &solstone_core_facets::ObservationPageItem,
) -> Option<String> {
    let content = observation.content.trim();
    if content.is_empty() {
        return None;
    }
    let quote: String = content.chars().take(200).collect();
    let source = observation.source_day.as_deref().unwrap_or("unknown");
    Some(format!(
        "{index}. target_quote: {}\n   source_day: {}\n   content: {}",
        json!(quote),
        json!(source),
        json!(content)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: &str = "20260101";

    fn read_test_observations(journal: &Path, facet: &str, entity_dir: &str) -> Vec<Value> {
        let page = solstone_core_facets::read_live_observations(
            journal,
            facet,
            entity_dir,
            solstone_core_facets::ObservationReadQuery {
                order: solstone_core_facets::ObservationReadOrder::Oldest,
                limit: 200,
                ..Default::default()
            },
        )
        .unwrap();
        page.items
            .into_iter()
            .map(|item| {
                json!({
                    "id": item.id,
                    "content": item.content,
                    "observed_at": item.observed_at,
                    "source_day": item.source_day,
                    "relation": item.relation,
                    "by": item.by,
                })
            })
            .collect()
    }

    fn attach(root: &Path, name: &str) {
        solstone_core_facets::attach_or_reactivate_entity(root, "work", "Person", name, "")
            .unwrap();
    }

    fn segment_path(root: &Path, origin: &str) -> std::path::PathBuf {
        let path = root.join("chronicle").join(origin);
        fs::create_dir_all(path.join("talents")).unwrap();
        path
    }

    fn write_source_segment(
        root: &Path,
        origin: &str,
        transcript: &str,
        percept: &str,
        activity: &str,
        entity_context: &str,
    ) {
        let path = segment_path(root, origin);
        fs::write(
            path.join("audio.jsonl"),
            format!(
                "{}\n{}\n",
                json!({
                    "raw":"audio.wav", "backend":"parakeet", "model":"field-model.gguf",
                    "device":"cpu", "compute_type":"q8_0", "duration":180.0,
                    "noisy":false, "loud_windows":180, "speech_loud_windows":180,
                    "loud_speech_ratio":1.0, "overlap_fraction":0.0104,
                    "overlap_detector":"pyannote-segmentation-3.0-onnx",
                    "speaker_evidence":"multi", "speaker_evidence_multi_fraction":0.1395,
                    "speaker_evidence_version":"windowed-slots-v1", "stream":"field.audio",
                    "_solstone_processing":{"schema":"solstone.processing.v1","state":"analyzed","reason_code":"ok","handler":"transcribe","attempted_at":"2026-07-25T05:24:07Z","input_size":5760078}
                }),
                json!({"start":"00:00:00","text":transcript})
            ),
        )
        .unwrap();
        fs::write(
            path.join("screen.jsonl"),
            json!({"timestamp":0,"content":{"window":percept}}).to_string(),
        )
        .unwrap();
        fs::write(
            path.join("talents/sense.json"),
            json!({
                "activity_summary": activity,
                "entities": [
                    {"name":"Ada", "context":entity_context},
                    {"name":"Grace", "context":"OTHER_ENTITY_CONTAMINANT"}
                ]
            })
            .to_string(),
        )
        .unwrap();
    }

    fn save_detection(root: &Path, name: &str, description: &str, origin: &str) {
        solstone_core_facets::upsert_detection_segment(
            root,
            "work",
            DAY,
            origin,
            &[solstone_core_facets::DetectedEntityInput {
                entity_type: "Person".to_owned(),
                name: name.to_owned(),
                description: description.to_owned(),
            }],
        )
        .unwrap();
    }

    fn observer_request(root: &Path) -> crate::GenerateRequest {
        let mut prepared = PreparedTalent {
            name: "entity_observer".to_owned(),
            config: Map::from_iter([
                ("day".to_owned(), Value::String(DAY.to_owned())),
                ("facet".to_owned(), Value::String("work".to_owned())),
                (
                    "prompt".to_owned(),
                    Value::String("$observer_context".to_owned()),
                ),
            ]),
        };
        let state = build(
            &mut prepared,
            &ExecutionContext {
                journal: root.to_owned(),
            },
        )
        .unwrap();
        apply_prompt_override(&mut prepared, &state).unwrap();
        crate::generate_request(&prepared)
    }

    fn request_text(request: &crate::GenerateRequest) -> String {
        request
            .contents
            .iter()
            .map(|part| match part {
                crate::ContentPart::Text { text } => text.as_str(),
                crate::ContentPart::Image { .. } => "",
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn actual_generate_request_contains_detection_linked_source_and_changes_with_it() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        let origin = "20260101/field/090000_60";
        write_source_segment(
            root.path(),
            origin,
            "TRANSCRIPT_SENTINEL durable expertise in orbital mechanics",
            "PERCEPT_SENTINEL planning window",
            "ACTIVITY_SENTINEL reviewed launch architecture",
            "SENSE_SENTINEL explained the launch constraint",
        );
        save_detection(root.path(), "Ada", "UNCHANGED_DETECTION", origin);

        let first = observer_request(root.path());
        let first_text = request_text(&first);
        for sentinel in [
            "TRANSCRIPT_SENTINEL",
            "PERCEPT_SENTINEL",
            "ACTIVITY_SENTINEL",
            "SENSE_SENTINEL",
            "UNCHANGED_DETECTION",
            origin,
        ] {
            assert!(first_text.contains(sentinel), "missing {sentinel}");
        }
        assert!(!first_text.contains("Speaker_evidence_version"));
        assert!(!first_text.contains("OTHER_ENTITY_CONTAMINANT"));

        let path = root.path().join("chronicle").join(origin);
        fs::write(
            path.join("audio.jsonl"),
            r#"{"start":"00:00:00","text":"CHANGED_SOURCE_SENTINEL durable launch preference"}"#,
        )
        .unwrap();
        let second = observer_request(root.path());
        let second_text = request_text(&second);
        assert_ne!(first.contents, second.contents);
        assert!(second_text.contains("CHANGED_SOURCE_SENTINEL"));
        assert!(second_text.contains("UNCHANGED_DETECTION"));
    }

    #[test]
    fn origin_resolution_is_exact_and_partial_failures_stay_visible() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        write_source_segment(
            root.path(),
            "20260101/090000_60",
            "DIRECT_SENTINEL",
            "direct percept",
            "direct activity",
            "direct context",
        );
        write_source_segment(
            root.path(),
            "20260101/field/090000_60",
            "COLLIDING_NAMED_SENTINEL",
            "named percept",
            "named activity",
            "named context",
        );
        write_source_segment(
            root.path(),
            "090000_60",
            "TRAVERSAL_SENTINEL",
            "traversal percept",
            "traversal activity",
            "traversal context",
        );
        let detected_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(detected_path.parent().unwrap()).unwrap();
        fs::write(
            detected_path,
            format!(
                "{}\n",
                json!({
                    "id":"ada", "type":"Person", "name":"Ada",
                    "description":"summary", "segments":[42, "20251231/080000_60", "20260101/bad", "20260101/../090000_60", "20260101/090000_60"]
                })
            ),
        )
        .unwrap();

        let context = assemble_observer_context(root.path(), "work", DAY)
            .unwrap()
            .context;
        assert!(context.contains("DIRECT_SENTINEL"));
        assert!(!context.contains("COLLIDING_NAMED_SENTINEL"));
        assert!(!context.contains("TRAVERSAL_SENTINEL"));
        assert!(context.contains("rejected non-string or blank origin"));
        assert!(context.contains("origin belongs to another day"));
        assert!(context.contains("invalid segment key"));
        assert!(context.contains("invalid stream name"));
    }

    #[test]
    fn multiple_rows_union_newest_origins_and_keep_each_summary_once() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        for (origin, sentinel) in [
            ("20260101/field/080000_60", "OLD_EXCLUDED"),
            ("20260101/090000_60", "NINE_INCLUDED"),
            ("20260101/field/100000_60", "TEN_INCLUDED"),
            ("20260101/110000_60", "ELEVEN_INCLUDED"),
        ] {
            write_source_segment(root.path(), origin, sentinel, "p", "a", "c");
        }
        let detected_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(detected_path.parent().unwrap()).unwrap();
        let rows = [
            json!({"id":"ada-1","type":"Person","name":"Ada","description":"SUMMARY_ONE","segments":["20260101/field/080000_60","20260101/field/100000_60"]}),
            json!({"id":"ada-2","type":"Person","name":"Ada","description":"SUMMARY_TWO","segments":["20260101/090000_60","20260101/110000_60","20260101/110000_60"]}),
        ];
        fs::write(
            detected_path,
            rows.iter()
                .map(|row| row.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();

        let context = assemble_observer_context(root.path(), "work", DAY)
            .unwrap()
            .context;
        assert_eq!(context.matches("SUMMARY_ONE").count(), 1);
        assert_eq!(context.matches("SUMMARY_TWO").count(), 1);
        for sentinel in ["NINE_INCLUDED", "TEN_INCLUDED", "ELEVEN_INCLUDED"] {
            assert!(context.contains(sentinel), "missing {sentinel}");
        }
        assert!(!context.contains("OLD_EXCLUDED"));
        assert!(context.contains("selected newest 3 of 4 valid origins"));
    }

    #[test]
    fn supported_originless_row_survives_beside_valid_evidence() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        solstone_core_facets::save_detected_entity(
            root.path(),
            "work",
            DAY,
            "Person",
            "Ada",
            "originless summary",
        )
        .unwrap();
        let origin = "20260101/field/090000_60";
        write_source_segment(root.path(), origin, "GRACE_SOURCE", "p", "a", "c");
        save_detection(root.path(), "Grace", "with source", origin);

        let context = assemble_observer_context(root.path(), "work", DAY)
            .unwrap()
            .context;
        assert!(context.contains("originless summary"));
        assert!(context.contains("no linked source origins"));
        assert!(context.contains("GRACE_SOURCE"));
    }

    #[test]
    fn invalid_optional_detection_evidence_is_visible_without_hiding_siblings() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        let origin = "20260101/field/090000_60";
        write_source_segment(root.path(), origin, "GRACE_VALID_SOURCE", "p", "a", "c");
        let detected_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(detected_path.parent().unwrap()).unwrap();
        let rows = [
            json!({"id":"ada","type":"Person","name":"Ada","description":null,"segments":{"bad":true}}),
            json!({"id":"grace","type":"Person","name":"Grace","description":"valid summary","segments":[origin]}),
        ];
        fs::write(
            detected_path,
            rows.iter()
                .map(|row| row.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();

        let context = assemble_observer_context(root.path(), "work", DAY)
            .unwrap()
            .context;
        assert!(context.contains("summary unavailable"));
        assert!(context.contains("rejected non-array segments field"));
        assert!(context.contains("GRACE_VALID_SOURCE"));
    }

    #[test]
    fn nullable_relations_persist_and_malformed_relations_do_not() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"add", "target_index":null, "content":"Original durable fact.",
                "target_quote":null, "reasoning":"new fact", "relation":null
            }]}], "summary":"added"})
            .to_string(),
            "work",
            DAY,
            &BTreeSet::from(["ada".to_owned()]),
            &[],
        )
        .unwrap();
        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"update", "target_index":0, "content":"Updated durable fact.",
                "target_quote":"Original durable fact.", "reasoning":"correction", "relation":null
            }]}], "summary":"updated"})
            .to_string(),
            "work",
            DAY,
            &BTreeSet::from(["ada".to_owned()]),
            &[],
        )
        .unwrap();
        let before = read_test_observations(root.path(), "work", "ada");
        assert_eq!(before[0]["content"], "Updated durable fact.");

        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"add", "target_index":null, "content":"Must not persist.",
                "target_quote":null, "reasoning":"bad relation", "relation":"malformed"
            }]}], "summary":"bad"})
            .to_string(),
            "work",
            DAY,
            &BTreeSet::from(["ada".to_owned()]),
            &[],
        )
        .unwrap();
        assert_eq!(read_test_observations(root.path(), "work", "ada"), before);
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["skipped"], 1);
    }

    #[test]
    fn invalid_duplicate_and_mismatched_targets_are_guarded() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "ada",
            "Guarded durable fact.",
            Some(DAY),
            None,
        )
        .unwrap();
        let original = read_test_observations(root.path(), "work", "ada");
        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[
                {"op":"update","target_index":true,"content":"bad","target_quote":"Guarded durable fact.","reasoning":"bad index","relation":null},
                {"op":"update","target_index":0,"content":"bad","target_quote":"mismatch","reasoning":"bad quote","relation":null}
            ]}],"summary":"guards"}).to_string(),
            "work",
            DAY,
            &BTreeSet::from(["ada".to_owned()]),
            &[],
        )
        .unwrap();
        assert_eq!(read_test_observations(root.path(), "work", "ada"), original);
        let outcome_path = root
            .path()
            .join("facets/work/entities/20260101_observer_outcome.json");
        let first_outcome: Value =
            serde_json::from_slice(&fs::read(&outcome_path).unwrap()).unwrap();
        assert_eq!(first_outcome["skipped"], 2);

        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[
                {"op":"keep","target_index":0,"content":null,"target_quote":"Guarded durable fact.","reasoning":null,"relation":null},
                {"op":"drop","target_index":0,"content":null,"target_quote":"Guarded durable fact.","reasoning":"duplicate","relation":null}
            ]}],"summary":"duplicates"}).to_string(),
            "work",
            DAY,
            &BTreeSet::from(["ada".to_owned()]),
            &[],
        )
        .unwrap();
        assert_eq!(read_test_observations(root.path(), "work", "ada"), original);
        let outcome: Value = serde_json::from_slice(&fs::read(outcome_path).unwrap()).unwrap();
        assert_eq!(outcome["keep"], 1);
        assert_eq!(outcome["skipped"], 1);
    }

    #[test]
    fn shipped_schema_accepts_null_and_rejects_malformed_relation() {
        let schema_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../payload/solstone/apps/entities/talent/entity_observer.schema.json");
        let schema: Value =
            serde_json::from_str(&fs::read_to_string(schema_path).unwrap()).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let output = |relation| {
            json!({
                "entities":[{"entity_id":"ada","operations":[{
                    "op":"add", "target_index":null, "content":"Fact", "target_quote":null,
                    "reasoning":"new", "relation":relation
                }]}], "summary":"result"
            })
        };
        assert!(validator.is_valid(&output(Value::Null)));
        assert!(!validator.is_valid(&output(json!("malformed"))));
    }

    #[test]
    fn strict_snapshot_refusal_is_durable_and_never_rewrites_observations() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        let path = root
            .path()
            .join("facets/work/entities/ada/observations.jsonl");
        fs::write(&path, "{\"content\":\"valid\"}\n{}\n").unwrap();
        let before = fs::read(&path).unwrap();
        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"add", "target_index":null, "content":"Must not persist",
                "target_quote":null, "reasoning":"new", "relation":null
            }]}], "summary":"result"})
            .to_string(),
            "work",
            DAY,
            &BTreeSet::from(["ada".to_owned()]),
            &[],
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            outcome["error"]
                .as_str()
                .unwrap()
                .contains("malformed observation")
        );
    }

    #[test]
    fn strict_preflight_prevents_partial_multi_entity_writes() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        let grace_path = root
            .path()
            .join("facets/work/entities/grace/observations.jsonl");
        fs::write(&grace_path, "{}\n").unwrap();
        apply_result(
            root.path(),
            &json!({"entities":[
                {"entity_id":"ada","operations":[{
                    "op":"add", "target_index":null, "content":"Must not partially persist",
                    "target_quote":null, "reasoning":"new", "relation":null
                }]},
                {"entity_id":"grace","operations":[{
                    "op":"add", "target_index":null, "content":"Also blocked",
                    "target_quote":null, "reasoning":"new", "relation":null
                }]}
            ], "summary":"result"})
            .to_string(),
            "work",
            DAY,
            &BTreeSet::from(["ada".to_owned(), "grace".to_owned()]),
            &[],
        )
        .unwrap();
        assert!(read_test_observations(root.path(), "work", "ada").is_empty());
        assert_eq!(fs::read_to_string(grace_path).unwrap(), "{}\n");
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            outcome["error"]
                .as_str()
                .unwrap()
                .contains("malformed observation")
        );
    }

    #[test]
    fn strict_assembly_refuses_directory_and_json_valid_unusable_rows() {
        let directory_case = tempfile::tempdir().unwrap();
        attach(directory_case.path(), "Ada");
        fs::create_dir_all(
            directory_case
                .path()
                .join("facets/work/entities/20260101.jsonl"),
        )
        .unwrap();
        assert!(assemble_observer_context(directory_case.path(), "work", DAY).is_err());

        let detected_case = tempfile::tempdir().unwrap();
        attach(detected_case.path(), "Ada");
        fs::write(
            detected_case
                .path()
                .join("facets/work/entities/20260101.jsonl"),
            "{}\n",
        )
        .unwrap();
        assert!(assemble_observer_context(detected_case.path(), "work", DAY).is_err());

        for malformed in ["\"scalar\"\n", "[]\n", "{}\n", "{\"content\":\"  \"}\n"] {
            let observation_case = tempfile::tempdir().unwrap();
            attach(observation_case.path(), "Ada");
            solstone_core_facets::save_detected_entity(
                observation_case.path(),
                "work",
                DAY,
                "Person",
                "Ada",
                "summary",
            )
            .unwrap();
            fs::write(
                observation_case
                    .path()
                    .join("facets/work/entities/ada/observations.jsonl"),
                malformed,
            )
            .unwrap();
            assert!(assemble_observer_context(observation_case.path(), "work", DAY).is_err());
        }
    }

    #[test]
    fn failed_outcome_publication_returns_a_caller_visible_error() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        let observations = root
            .path()
            .join("facets/work/entities/ada/observations.jsonl");
        fs::write(&observations, "{}\n").unwrap();
        fs::create_dir_all(
            root.path()
                .join("facets/work/entities/20260101_observer_outcome.json"),
        )
        .unwrap();
        let before = fs::read(&observations).unwrap();
        let error = apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{
                "op":"add", "target_index":null, "content":"Must not persist",
                "target_quote":null, "reasoning":"new", "relation":null
            }]}], "summary":"result"})
            .to_string(),
            "work",
            DAY,
            &BTreeSet::from(["ada".to_owned()]),
            &[],
        )
        .unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(fs::read(&observations).unwrap(), before);
    }

    #[test]
    fn empty_result_is_a_successful_no_change() {
        let root = tempfile::tempdir().unwrap();
        apply_result(
            root.path(),
            r#"{"entities":[],"summary":"No durable changes."}"#,
            "work",
            DAY,
            &BTreeSet::new(),
            &[],
        )
        .unwrap();
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        for key in [
            "add",
            "update",
            "drop",
            "keep",
            "skipped",
            "excluded",
            "unselected",
        ] {
            assert_eq!(outcome[key], 0, "nonzero {key}");
        }
        assert!(outcome["error"].is_null());
    }

    #[test]
    fn entity_selection_and_context_bounds_do_not_erase_the_sixth_packet() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..7 {
            let name = format!("Person {index}");
            attach(root.path(), &name);
            solstone_core_facets::save_detected_entity(
                root.path(),
                "work",
                DAY,
                "Person",
                &name,
                &format!("SUMMARY_{index}_{}", "é😀".repeat(500)),
            )
            .unwrap();
        }
        let context = assemble_observer_context(root.path(), "work", DAY)
            .unwrap()
            .context;
        assert!(context.chars().count() <= MAX_OBSERVER_CONTEXT_CHARS);
        for index in 0..6 {
            assert!(context.contains(&format!("#### Person {index}")));
            assert!(context.contains("Evidence status:"));
        }
        assert!(!context.contains("#### Person 6"));
        assert!(context.contains("6 of 7 active"));
    }

    #[test]
    fn over_budget_observation_inventory_excludes_the_entity_and_still_generates() {
        // Criterion 8 & 9: All-excluded case: not StageFailed, not Skipped; context states budget exclusion;
        // no empty-active string; outcome names each excluded entity with measured size.
        // Operations naming any attached entity write nothing (count excluded, observations unchanged).
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        solstone_core_facets::save_detected_entity(
            root.path(),
            "work",
            DAY,
            "Person",
            "Ada",
            "summary",
        )
        .unwrap();
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "ada",
            &"é😀".repeat(2_000),
            Some(DAY),
            None,
        )
        .unwrap();
        let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert!(assembly.served_ids.is_empty());
        assert_eq!(assembly.exclusions.len(), 1);
        assert_eq!(assembly.exclusions[0].entity_id, "ada");
        assert!(assembly.exclusions[0].chars > MAX_ENTITY_CONTEXT_CHARS);
        assert!(
            !assembly
                .context
                .contains("No active entities found for this day.")
        );
        assert!(assembly.context.contains("0 of 1 active"));
        assert!(!assembly.exclusions.is_empty());
        assert!(assembly.context.lines().count() > 8);

        // Build test
        let mut prepared = PreparedTalent {
            name: "entity_observer".to_owned(),
            config: Map::from_iter([
                (
                    "prompt".to_owned(),
                    Value::String("$observer_context".to_owned()),
                ),
                ("facet".to_owned(), Value::String("work".to_owned())),
                ("day".to_owned(), Value::String(DAY.to_owned())),
            ]),
        };
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let _state = build(&mut prepared, &context).unwrap();

        // Apply test (Criterion 9: operation naming attached/excluded entity writes nothing, count excluded)
        let before_obs = read_test_observations(root.path(), "work", "ada");
        apply_result(
            root.path(),
            &json!({"entities":[{"entity_id":"ada","operations":[{"op":"add","content":"Should not persist"}]}]}).to_string(),
            "work",
            DAY,
            &assembly.served_ids,
            &assembly.exclusions,
        )
        .unwrap();
        let after_obs = read_test_observations(root.path(), "work", "ada");
        assert_eq!(after_obs, before_obs);

        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["excluded"], 1);
        assert_eq!(outcome["add"], 0);
        let exclusions = outcome["exclusions"].as_array().unwrap();
        assert_eq!(exclusions.len(), 1);
        assert_eq!(exclusions[0]["entity_id"], "ada");
        assert_eq!(exclusions[0]["chars"], assembly.exclusions[0].chars);
    }

    #[test]
    fn context_quote_is_bounded_verbatim_and_separate_from_source() {
        for content in [
            "Short observation.".to_owned(),
            "é😀\n\"quoted\" ".repeat(80),
        ] {
            let item = solstone_core_facets::ObservationPageItem {
                id: 1,
                content: content.clone(),
                observed_at: 0,
                source_day: Some("20260712".to_owned()),
                relation: None,
                by: None,
            };
            let rendered = observation_context(3, &item).unwrap();
            let mut lines = rendered.lines();
            let quote: String = serde_json::from_str(
                lines
                    .next()
                    .unwrap()
                    .strip_prefix("3. target_quote: ")
                    .unwrap(),
            )
            .unwrap();
            assert!(!quote.is_empty());
            assert!(quote.chars().count() <= 200);
            assert!(content.starts_with(&quote));
            assert_eq!(lines.next().unwrap(), "   source_day: \"20260712\"");
            let full: String =
                serde_json::from_str(lines.next().unwrap().strip_prefix("   content: ").unwrap())
                    .unwrap();
            assert_eq!(full, content.trim());
            assert!(lines.next().is_none());
        }
        let empty_item = solstone_core_facets::ObservationPageItem {
            id: 1,
            content: "  ".to_owned(),
            observed_at: 0,
            source_day: None,
            relation: None,
            by: None,
        };
        assert!(observation_context(0, &empty_item).is_none());
    }

    #[test]
    fn rendered_quote_supports_guarded_keep_and_update_of_long_observation() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::attach_or_reactivate_entity(root.path(), "work", "Person", "Ada", "")
            .unwrap();
        let content = "Long observation with Unicode é😀. ".repeat(30);
        let apply = |operation: Value| {
            apply_result(
                root.path(),
                &json!({"entities":[{"entity_id":"ada","operations":[operation]}]}).to_string(),
                "work",
                "20260101",
                &BTreeSet::from(["ada".to_owned()]),
                &[],
            )
            .unwrap();
        };
        apply(json!({"op":"add","content":content}));
        let original = read_test_observations(root.path(), "work", "ada");
        let page = solstone_core_facets::read_live_observations(
            root.path(),
            "work",
            "ada",
            solstone_core_facets::ObservationReadQuery::default(),
        )
        .unwrap();
        let rendered = observation_context(0, &page.items[0]).unwrap();
        let quote: String = serde_json::from_str(
            rendered
                .lines()
                .next()
                .unwrap()
                .strip_prefix("0. target_quote: ")
                .unwrap(),
        )
        .unwrap();
        apply(json!({"op":"keep","target_index":0,"target_quote":quote}));
        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["keep"], 1);
        assert_eq!(read_test_observations(root.path(), "work", "ada"), original);
        apply(
            json!({"op":"update","target_index":0,"target_quote":"does not match","content":"Wrong."}),
        );
        assert_eq!(read_test_observations(root.path(), "work", "ada"), original);
        apply(
            json!({"op":"update","target_index":0,"target_quote":quote,"content":"Updated fact."}),
        );
        let updated = read_test_observations(root.path(), "work", "ada");
        assert_eq!(updated[0]["content"], "Updated fact.");
    }

    #[test]
    fn missing_scope_skips_the_stage() {
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let cases = [
            (Map::new(), "no_day"),
            (
                Map::from_iter([("facet".to_owned(), Value::String("work".to_owned()))]),
                "no_day",
            ),
            (
                Map::from_iter([("day".to_owned(), Value::String("20260101".to_owned()))]),
                "no_facet",
            ),
        ];
        for (config, reason) in cases {
            let mut prepared = PreparedTalent {
                name: "entity_observer".to_owned(),
                config: Map::from_iter([(
                    "prompt".to_owned(),
                    Value::String("$observer_context".to_owned()),
                )])
                .into_iter()
                .chain(config)
                .collect(),
            };
            match gate(&prepared, &context) {
                Ok(GateDecision::Skip(skipped)) => assert_eq!(skipped, reason),
                other => panic!("expected skip {reason}, got {other:?}"),
            }
            match build(&mut prepared, &context) {
                Err(RuntimeOutcome::Skipped {
                    stage,
                    talent,
                    reason: skipped,
                }) => {
                    assert_eq!(stage, "entities:entity_observer");
                    assert_eq!(talent, "entity_observer");
                    assert_eq!(skipped, reason);
                }
                other => panic!("expected skipped {reason}, got {other:?}"),
            }
        }
    }

    #[test]
    fn relation_cleaning_uses_the_shared_relation_set() {
        // Derived from solstone/apps/entities/talent/entity_observer.py:82-139.
        let root = tempfile::tempdir().unwrap();
        let entities = Vec::new();
        let context = OperationContext {
            read_only: false,
            journal: root.path(),
            facet: "work",
            day: "20260101",
            entity_id: "ada",
            entities: &entities,
        };
        let (accepted, status) = clean_operation(&json!({"op":"add","content":"Works with Ada.","relation":{"kind":"works-with","target_name":"Unknown","note":"collaboration"}}), &mut Vec::new(), &context).unwrap();
        assert_eq!(status, Some("relation_unresolved"));
        assert!(accepted.is_some());
        let (rejected, status) = clean_operation(&json!({"op":"add","content":"x","relation":{"kind":"not-a-relation","target_name":"Ada","note":"x"}}), &mut Vec::new(), &context).unwrap();
        assert!(rejected.is_none());
        assert_eq!(status, Some("skipped"));
    }

    #[test]
    fn operation_cleaning_and_target_boundaries_match_reference() {
        // Derived from solstone/apps/entities/talent/entity_observer.py:67-80,141-211.
        assert_eq!(target_index(Some(&json!(4))), Some(4));
        assert_eq!(target_index(Some(&json!(true))), None);
        assert_eq!(
            target_quote(Some(&json!("  quote  "))),
            Some("quote".to_owned())
        );
        assert_eq!(target_quote(Some(&json!("  "))), None);
        let root = tempfile::tempdir().unwrap();
        let entities = Vec::new();
        let context = OperationContext {
            read_only: false,
            journal: root.path(),
            facet: "work",
            day: "20260101",
            entity_id: "ada",
            entities: &entities,
        };
        let (accepted, status) = clean_operation(
            &json!({"op":"add","content":"  durable fact  "}),
            &mut Vec::new(),
            &context,
        )
        .unwrap();
        assert_eq!(status, None);
        assert_eq!(accepted.unwrap()["content"], "durable fact");
        let (rejected, status) = clean_operation(
            &json!({"op":"update","target_index":true,"content":"x"}),
            &mut Vec::new(),
            &context,
        )
        .unwrap();
        assert!(rejected.is_none());
        assert_eq!(status, Some("skipped"));
    }

    #[test]
    fn malformed_result_writes_an_error_outcome_sidecar() {
        // Derived from solstone/apps/entities/talent/entity_observer.py:55-64,225-242.
        let root = tempfile::tempdir().unwrap();
        apply_result(
            root.path(),
            "not json",
            "work",
            "20260101",
            &BTreeSet::new(),
            &[],
        )
        .unwrap();
        let sidecar: Value = serde_json::from_str(
            &fs::read_to_string(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sidecar["error"], "could not parse result as JSON");
        assert_eq!(sidecar["add"], 0);
    }

    #[test]
    fn clean_operations_persist_observations_and_a_success_outcome() {
        // Derived from solstone/apps/entities/talent/entity_observer.py:218-318.
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::attach_or_reactivate_entity(root.path(), "work", "Person", "Ada", "")
            .unwrap();
        apply_result(
            root.path(),
            r#"{"entities":[{"entity_id":"ada","operations":[{"op":"add","content":"Prefers concise updates."}]}]}"#,
            "work",
            "20260101",
            &BTreeSet::from(["ada".to_owned()]),
            &[],
        )
        .unwrap();
        let observations = read_test_observations(root.path(), "work", "ada");
        assert_eq!(observations[0]["content"], "Prefers concise updates.");
        let sidecar: Value = serde_json::from_str(
            &fs::read_to_string(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(sidecar["add"], 1);
        assert!(sidecar["error"].is_null());
    }

    #[test]
    fn apply_result_writes_under_the_relationship_dir() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "blue", "💼", None)
            .unwrap();
        let identity = root.path().join("entities/dir-ada");
        fs::create_dir_all(&identity).unwrap();
        fs::write(
            identity.join("entity.json"),
            br#"{"id":"effective-ada","name":"Ada"}"#,
        )
        .unwrap();
        let relationship = root.path().join("facets/work/entities/legacy-ada");
        fs::create_dir_all(&relationship).unwrap();
        fs::write(
            relationship.join("entity.json"),
            br#"{"entity_id":"effective-ada"}"#,
        )
        .unwrap();

        apply_result(
            root.path(),
            r#"{"entities":[{"entity_id":"effective-ada","operations":[{"op":"add","content":"from apply"}]}]}"#,
            "work",
            "20260101",
            &BTreeSet::from(["effective-ada".to_owned()]),
            &[],
        )
        .unwrap();
        let observations = read_test_observations(root.path(), "work", "legacy-ada");
        assert_eq!(observations[0]["content"], "from apply");
        assert!(read_test_observations(root.path(), "work", "effective-ada").is_empty());
    }

    #[test]
    fn criterion_1_one_over_budget_and_two_fitting_entities() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        attach(root.path(), "Person 0");
        for name in ["Ada", "Grace", "Person 0"] {
            solstone_core_facets::save_detected_entity(
                root.path(),
                "work",
                DAY,
                "Person",
                name,
                "summary",
            )
            .unwrap();
        }
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "ada",
            &"é😀".repeat(2_000),
            Some(DAY),
            None,
        )
        .unwrap();
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "grace",
            "Short fact.",
            Some(DAY),
            None,
        )
        .unwrap();
        let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert_eq!(
            assembly.served_ids,
            BTreeSet::from(["grace".to_owned(), "person_0".to_owned()])
        );
        assert_eq!(assembly.exclusions.len(), 1);
        assert_eq!(assembly.exclusions[0].entity_id, "ada");
        assert!(assembly.exclusions[0].chars > MAX_ENTITY_CONTEXT_CHARS);
        assert!(assembly.context.contains("2 of 3 active"));
        assert!(assembly.context.contains("#### Grace (grace)"));
        assert!(assembly.context.contains("#### Person 0 (person_0)"));
        assert!(!assembly.context.contains("#### Ada (ada)"));
    }

    #[test]
    fn criterion_2_boundary_just_under_and_just_over_multibyte() {
        let chunk = "é😀".repeat(20);
        let mut ada_content = "é😀".repeat(1_200);
        loop {
            let root = tempfile::tempdir().unwrap();
            attach(root.path(), "Ada");
            solstone_core_facets::save_detected_entity(
                root.path(),
                "work",
                DAY,
                "Person",
                "Ada",
                "s",
            )
            .unwrap();
            solstone_core_facets::add_observation(
                root.path(),
                "work",
                "ada",
                &ada_content,
                Some(DAY),
                None,
            )
            .unwrap();
            let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
            if assembly.served_ids.contains("ada") {
                let start = assembly.context.find("#### Ada").expect("Ada in context");
                let rendered_packet = &assembly.context[start..];
                let chars = rendered_packet.chars().count();
                if chars > MAX_ENTITY_CONTEXT_CHARS - 500 && chars <= MAX_ENTITY_CONTEXT_CHARS {
                    break;
                }
            }
            ada_content.push_str(&chunk);
        }

        let mut grace_content = ada_content.clone();
        loop {
            grace_content.push_str(&chunk);
            let root = tempfile::tempdir().unwrap();
            attach(root.path(), "Grace");
            solstone_core_facets::save_detected_entity(
                root.path(),
                "work",
                DAY,
                "Person",
                "Grace",
                "s",
            )
            .unwrap();
            solstone_core_facets::add_observation(
                root.path(),
                "work",
                "grace",
                &grace_content,
                Some(DAY),
                None,
            )
            .unwrap();
            let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
            if !assembly.exclusions.is_empty() {
                let chars = assembly.exclusions[0].chars;
                if chars > MAX_ENTITY_CONTEXT_CHARS && chars <= MAX_ENTITY_CONTEXT_CHARS + 500 {
                    break;
                }
            }
        }

        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        solstone_core_facets::save_detected_entity(root.path(), "work", DAY, "Person", "Ada", "s")
            .unwrap();
        solstone_core_facets::save_detected_entity(
            root.path(),
            "work",
            DAY,
            "Person",
            "Grace",
            "s",
        )
        .unwrap();

        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "ada",
            &ada_content,
            Some(DAY),
            None,
        )
        .unwrap();
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "grace",
            &grace_content,
            Some(DAY),
            None,
        )
        .unwrap();

        let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert!(assembly.served_ids.contains("ada"));
        assert!(!assembly.served_ids.contains("grace"));
        assert_eq!(assembly.exclusions.len(), 1);
        assert_eq!(assembly.exclusions[0].entity_id, "grace");

        let ada_start = assembly.context.find("#### Ada").expect("Ada in context");
        let ada_packet_chars = assembly.context[ada_start..].chars().count();
        assert!(ada_packet_chars > MAX_ENTITY_CONTEXT_CHARS - 500);
        assert!(ada_packet_chars <= MAX_ENTITY_CONTEXT_CHARS);

        let grace_exclusion_chars = assembly.exclusions[0].chars;
        assert!(grace_exclusion_chars > MAX_ENTITY_CONTEXT_CHARS);
        assert!(grace_exclusion_chars <= MAX_ENTITY_CONTEXT_CHARS + 500);

        assert!(assembly.context.contains("#### Ada (ada)"));
        assert!(!assembly.context.contains("#### Grace (grace)"));
        assert!(assembly.context.contains(&ada_content));
    }

    #[test]
    fn criterion_3_and_4_zero_exclusion_header_byte_identical_and_observations_rendered() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        let origin = "20260101/field/090000_60";
        write_source_segment(
            root.path(),
            origin,
            "ADA_EVIDENCE",
            "percept",
            "activity",
            "context",
        );
        save_detection(root.path(), "Ada", "Ada summary text", origin);
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "ada",
            "Numbered durable observation text.",
            Some(DAY),
            None,
        )
        .unwrap();

        let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert!(assembly.exclusions.is_empty());
        assert_eq!(assembly.served_ids, BTreeSet::from(["ada".to_owned()]));

        let expected_header = [
            "# Entity Observer Context",
            "",
            "## Facet: work",
            "## Day: 20260101",
            "## Active Entities: 1 of 1 active",
            "",
            "### Entities",
            "",
        ]
        .join("\n");
        assert!(assembly.context.starts_with(&expected_header));
        assert!(assembly.context.contains("Ada summary text"));
        assert!(assembly.context.contains("ADA_EVIDENCE"));
        assert!(
            assembly
                .context
                .contains("Numbered durable observation text.")
        );
        assert!(assembly.context.contains("0. target_quote: "));
    }

    #[test]
    fn criterion_5_6_11_18_eight_active_six_selected_one_excluded_gating_and_counts() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..8 {
            let name = format!("Person {index}");
            attach(root.path(), &name);
            solstone_core_facets::save_detected_entity(
                root.path(),
                "work",
                DAY,
                "Person",
                &name,
                "summary",
            )
            .unwrap();
        }
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "person_0",
            &"é😀".repeat(2_000),
            Some(DAY),
            None,
        )
        .unwrap();

        let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert_eq!(assembly.served_ids.len(), 5);
        for index in 1..=5 {
            assert!(assembly.served_ids.contains(&format!("person_{index}")));
        }
        assert!(!assembly.served_ids.contains("person_0"));
        assert!(!assembly.served_ids.contains("person_6"));
        assert!(!assembly.served_ids.contains("person_7"));
        assert_eq!(assembly.exclusions.len(), 1);
        assert_eq!(assembly.exclusions[0].entity_id, "person_0");

        assert!(assembly.context.contains("5 of 8 active"));
        assert!(!assembly.context.contains("person_0"));
        assert!(!assembly.context.contains("person_6"));
        assert!(!assembly.context.contains("person_7"));
        assert!(!assembly.context.contains("#### Person 0"));
        assert!(!assembly.context.contains("#### Person 6"));

        let payload = json!({
            "entities": [
                {"entity_id": "person_0", "operations": [{"op": "add", "content": "Excluded write"}]},
                {"entity_id": "person_1", "operations": [{"op": "add", "content": "Served write"}]},
                {"entity_id": "person_6", "operations": [{"op": "add", "content": "Unselected write"}]},
                {"entity_id": "unattached", "operations": [{"op": "add", "content": "Unattached write"}]}
            ],
            "summary": "mixed test"
        });
        apply_result(
            root.path(),
            &payload.to_string(),
            "work",
            DAY,
            &assembly.served_ids,
            &assembly.exclusions,
        )
        .unwrap();

        let p0_obs = read_test_observations(root.path(), "work", "person_0");
        assert!(!p0_obs.iter().any(|o| o["content"] == "Excluded write"));
        let p1_obs = read_test_observations(root.path(), "work", "person_1");
        assert!(p1_obs.iter().any(|o| o["content"] == "Served write"));
        let p6_obs = read_test_observations(root.path(), "work", "person_6");
        assert!(p6_obs.is_empty());

        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["add"], 1);
        assert_eq!(outcome["excluded"], 1);
        assert_eq!(outcome["unselected"], 1);
        assert_eq!(outcome["skipped"], 1);
    }

    #[test]
    fn criterion_7_excluded_entity_with_corrupted_file_does_not_block_served_writes() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        solstone_core_facets::save_detected_entity(root.path(), "work", DAY, "Person", "Ada", "s")
            .unwrap();
        solstone_core_facets::save_detected_entity(
            root.path(),
            "work",
            DAY,
            "Person",
            "Grace",
            "s",
        )
        .unwrap();

        let grace_path = root
            .path()
            .join("facets/work/entities/grace/observations.jsonl");
        fs::write(&grace_path, "{}\n").unwrap();

        let served_ids = BTreeSet::from(["ada".to_owned()]);
        let exclusions = vec![EntityBudgetExclusion {
            entity_id: "grace".to_owned(),
            chars: 4000,
        }];

        let payload = json!({
            "entities": [
                {"entity_id": "ada", "operations": [{"op": "add", "content": "Valid Ada observation"}]},
                {"entity_id": "grace", "operations": [{"op": "add", "content": "Grace write should be excluded"}]}
            ],
            "summary": "preflight isolation"
        });
        apply_result(
            root.path(),
            &payload.to_string(),
            "work",
            DAY,
            &served_ids,
            &exclusions,
        )
        .unwrap();

        let ada_obs = read_test_observations(root.path(), "work", "ada");
        assert_eq!(ada_obs[0]["content"], "Valid Ada observation");
        assert_eq!(fs::read_to_string(grace_path).unwrap(), "{}\n");

        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["add"], 1);
        assert_eq!(outcome["excluded"], 1);
        assert!(outcome["error"].is_null());
    }

    #[test]
    fn criterion_10_empty_active_closed_by_default() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");

        let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert_eq!(assembly.context, "No active entities found for this day.");
        assert!(assembly.served_ids.is_empty());
        assert!(assembly.exclusions.is_empty());

        let payload = json!({
            "entities": [
                {"entity_id": "ada", "operations": [{"op": "add", "content": "Hallucinated write"}]}
            ],
            "summary": "empty active"
        });
        apply_result(
            root.path(),
            &payload.to_string(),
            "work",
            DAY,
            &assembly.served_ids,
            &assembly.exclusions,
        )
        .unwrap();

        let ada_obs = solstone_core_facets::read_live_observations(
            root.path(),
            "work",
            "ada",
            Default::default(),
        )
        .unwrap()
        .items;
        assert!(ada_obs.is_empty());

        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["unselected"], 1);
        assert_eq!(outcome["add"], 0);
    }

    #[test]
    fn criterion_13_large_evidence_status_alone_excludes_entity() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        let detected_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(detected_path.parent().unwrap()).unwrap();

        let long_invalid_segments = (0..50)
            .map(|i| {
                format!(
                    "20260101/invalid_stream_name_that_is_very_long_{:0>70}_{i}",
                    "x"
                )
            })
            .collect::<Vec<_>>();
        let row = json!({
            "id": "ada",
            "type": "Person",
            "name": "Ada",
            "description": null,
            "segments": long_invalid_segments,
        });
        fs::write(detected_path, format!("{row}\n")).unwrap();

        let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert!(assembly.served_ids.is_empty());
        assert_eq!(assembly.exclusions.len(), 1);
        assert_eq!(assembly.exclusions[0].entity_id, "ada");
        assert!(assembly.exclusions[0].chars > MAX_ENTITY_CONTEXT_CHARS);
    }

    #[test]
    fn criterion_14_header_allowance_boundary_table_test() {
        assert_eq!(HEADER_ALLOWANCE, 1_165);
        let bound_check = |n: usize| {
            n * MAX_ENTITY_CONTEXT_CHARS + n.saturating_sub(1) * 7 + HEADER_ALLOWANCE
                <= MAX_OBSERVER_CONTEXT_CHARS
        };
        assert!(bound_check(MAX_ACTIVE_ENTITIES));
        assert!(!bound_check(MAX_ACTIVE_ENTITIES + 1));
    }

    #[test]
    fn criterion_15_real_thread_build_commit_apply() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        for name in ["Ada", "Grace"] {
            solstone_core_facets::save_detected_entity(
                root.path(),
                "work",
                DAY,
                "Person",
                name,
                "s",
            )
            .unwrap();
        }
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "grace",
            &"é😀".repeat(2_000),
            Some(DAY),
            None,
        )
        .unwrap();

        let mut prepared = PreparedTalent {
            name: "entity_observer".to_owned(),
            config: Map::from_iter([
                (
                    "prompt".to_owned(),
                    Value::String("$observer_context".to_owned()),
                ),
                ("facet".to_owned(), Value::String("work".to_owned())),
                ("day".to_owned(), Value::String(DAY.to_owned())),
            ]),
        };
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let state = build(&mut prepared, &context).unwrap();

        let payload = json!({
            "entities": [
                {"entity_id": "ada", "operations": [{"op": "add", "content": "Ada durable note"}]},
                {"entity_id": "grace", "operations": [{"op": "add", "content": "Grace durable note"}]}
            ],
            "summary": "thread test"
        });
        let parsed = parse(&payload.to_string(), &prepared, &state).unwrap();
        let plan = commit(parsed, &prepared, &state).unwrap();

        let exec_context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        crate::writers::apply(plan, &exec_context).unwrap();

        let ada_obs = read_test_observations(root.path(), "work", "ada");
        assert_eq!(ada_obs[0]["content"], "Ada durable note");
        let grace_obs = read_test_observations(root.path(), "work", "grace");
        assert!(
            !grace_obs
                .iter()
                .any(|o| o["content"] == "Grace durable note")
        );

        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["add"], 1);
        assert_eq!(outcome["excluded"], 1);
    }

    #[test]
    fn criterion_16_seven_active_zero_exclusions_served_set_exact_six() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..7 {
            let name = format!("Person {index}");
            attach(root.path(), &name);
            solstone_core_facets::save_detected_entity(
                root.path(),
                "work",
                DAY,
                "Person",
                &name,
                "summary",
            )
            .unwrap();
        }

        let mut prepared = PreparedTalent {
            name: "entity_observer".to_owned(),
            config: Map::from_iter([
                (
                    "prompt".to_owned(),
                    Value::String("$observer_context".to_owned()),
                ),
                ("facet".to_owned(), Value::String("work".to_owned())),
                ("day".to_owned(), Value::String(DAY.to_owned())),
            ]),
        };
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let state = build(&mut prepared, &context).unwrap();
        let PrePostState::EntityObserver(observer_state) = &state else {
            panic!("expected EntityObserver state");
        };
        assert_eq!(observer_state.served_ids.len(), 6);
        assert!(observer_state.exclusions.is_empty());
        assert!(!observer_state.served_ids.contains("person_6"));

        let payload = json!({
            "entities": [
                {"entity_id": "person_6", "operations": [{"op": "add", "content": "Should be unselected"}]}
            ],
            "summary": "person 6 test"
        });
        let parsed = parse(&payload.to_string(), &prepared, &state).unwrap();
        let plan = commit(parsed, &prepared, &state).unwrap();
        crate::writers::apply(plan, &context).unwrap();

        let p6_obs = read_test_observations(root.path(), "work", "person_6");
        assert!(p6_obs.is_empty());

        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["unselected"], 1);
        assert_eq!(outcome["add"], 0);
    }

    #[test]
    fn criterion_17_relation_to_budget_excluded_attached_entity_resolves() {
        let root = tempfile::tempdir().unwrap();
        attach(root.path(), "Ada");
        attach(root.path(), "Grace");
        for name in ["Ada", "Grace"] {
            solstone_core_facets::save_detected_entity(
                root.path(),
                "work",
                DAY,
                "Person",
                name,
                "s",
            )
            .unwrap();
        }
        solstone_core_facets::add_observation(
            root.path(),
            "work",
            "grace",
            &"é😀".repeat(2_000),
            Some(DAY),
            None,
        )
        .unwrap();

        let assembly = assemble_observer_context(root.path(), "work", DAY).unwrap();
        assert!(assembly.served_ids.contains("ada"));
        assert!(!assembly.served_ids.contains("grace"));
        assert_eq!(assembly.exclusions.len(), 1);

        let payload = json!({
            "entities": [
                {"entity_id": "ada", "operations": [{
                    "op": "add",
                    "content": "Collaborates with Grace.",
                    "target_index": null,
                    "target_quote": null,
                    "reasoning": "collaboration",
                    "relation": {
                        "kind": "works-with",
                        "target_name": "Grace",
                        "note": "team project"
                    }
                }]}
            ],
            "summary": "relation test"
        });
        apply_result(
            root.path(),
            &payload.to_string(),
            "work",
            DAY,
            &assembly.served_ids,
            &assembly.exclusions,
        )
        .unwrap();

        let ada_obs = read_test_observations(root.path(), "work", "ada");
        assert_eq!(ada_obs[0]["content"], "Collaborates with Grace.");
        let rel = &ada_obs[0]["relation"];
        assert_eq!(rel["kind"], "works-with");
        assert_eq!(rel["target_entity_id"], "grace");

        let outcome: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/20260101_observer_outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["add"], 1);
        assert_eq!(outcome["relation_unresolved"], 0);
    }
}
