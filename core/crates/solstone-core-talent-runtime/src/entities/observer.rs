// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-observer hook.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ObserverState {
    context: Option<String>,
}

type Counts = BTreeMap<&'static str, usize>;

fn empty_counts() -> Counts {
    BTreeMap::from([
        ("update", 0),
        ("add", 0),
        ("drop", 0),
        ("keep", 0),
        ("skipped", 0),
        ("relation_unresolved", 0),
    ])
}

fn write_outcome(journal: &Path, facet: &str, day: &str, counts: &Counts, error: Option<&str>) {
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
        "error".to_owned(),
        error.map_or(Value::Null, |error| Value::String(error.to_owned())),
    );
    payload.insert(
        "ts".to_owned(),
        Value::from(chrono::Utc::now().timestamp_millis()),
    );
    // Preserve solstone/apps/entities/talent/entity_observer.py:55-64: stage-owned outcome sidecar.
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, format!("{}\n", Value::Object(payload)));
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
    entities: &[solstone_core_entity::EntityResolutionEntity],
    journal: &Path,
    facet: &str,
    day: &str,
    entity_id: &str,
) -> Result<(Option<Value>, Option<&'static str>), String> {
    if value.is_none() || matches!(op, "drop" | "keep") {
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
    let resolution = solstone_core_entity::record_entity_resolution(
        journal,
        target_name,
        entities,
        json!({"kind":"facet","facet":facet}),
        json!({"lane":"apps.entities.entity_observer","facet":facet,"day":day,"record_id":entity_id,"field":"relation.target_name"}),
        90.0,
        false,
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
        let (relation, status) = clean_relation(
            item.get("relation"),
            op,
            context.entities,
            context.journal,
            context.facet,
            context.day,
            context.entity_id,
        )?;
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
    let (relation, status) = clean_relation(
        item.get("relation"),
        op,
        context.entities,
        context.journal,
        context.facet,
        context.day,
        context.entity_id,
    )?;
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
    let observer_context = match (facet, day) {
        (Some(facet), Some(day)) if !facet.is_empty() && !day.is_empty() => Some(
            assemble_observer_context(&context.journal, facet, day).map_err(|detail| {
                RuntimeOutcome::StageFailed(stage_error(
                    "build",
                    "entities:entity_observer",
                    prepared,
                    detail,
                ))
            })?,
        ),
        _ => return Err(skip_missing_scope(prepared)),
    };
    Ok(PrePostState::EntityObserver(ObserverState {
        context: observer_context,
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
    _: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "entities:entity_observer",
            prepared,
            "expected text output",
        ));
    };
    let (Some(facet), Some(day)) = (
        prepared.config.get("facet").and_then(Value::as_str),
        prepared.config.get("day").and_then(Value::as_str),
    ) else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::EntityObserver {
        output,
        facet: facet.to_owned(),
        day: day.to_owned(),
    }))
}

pub fn apply_result(journal: &Path, output: &str, facet: &str, day: &str) -> Result<(), String> {
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
        let (ids, entities) = attached_entities(journal, facet)?;
        for entry in entries {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let operations = entry.get("operations").and_then(Value::as_array);
            let Some(operations) = operations else {
                continue;
            };
            let Some(entity_id) = entry.get("entity_id").and_then(Value::as_str) else {
                *counts.entry("skipped").or_default() += operations.len();
                continue;
            };
            if !ids.iter().any(|id| id == entity_id) {
                *counts.entry("skipped").or_default() += operations.len();
                continue;
            }
            let mut clean = Vec::new();
            let mut seen = Vec::new();
            let operation_context = OperationContext {
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
            match solstone_core_facets::record_observation_ops(
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
    write_outcome(journal, facet, day, &counts, error.as_deref());
    Ok(())
}

fn assemble_observer_context(journal: &Path, facet: &str, day: &str) -> Result<String, String> {
    let scoped = solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false)
        .map_err(|error| error.to_string())?;
    let detected = solstone_core_facets::read_detected_entities(journal, facet, day)
        .map_err(|error| error.to_string())?;
    let mut active = Vec::new();
    for row in detected {
        let Some(name) = row.get("name").and_then(Value::as_str) else {
            continue;
        };
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
                    aka: Vec::new(),
                    emails: Vec::new(),
                },
            )
            .collect::<Vec<_>>();
        if let Some(found) =
            solstone_core_entity_matching::find_matching_entity(name, &candidates, 90.0)
        {
            active.push((scoped[found.candidate_index].clone(), row));
        }
    }
    active.sort_by(|left, right| left.0.entity_id.cmp(&right.0.entity_id));
    active.dedup_by(|left, right| left.0.entity_id == right.0.entity_id);
    if active.is_empty() {
        return Ok("No active entities found for this day.".to_owned());
    }
    let total = active.len();
    let mut lines = vec![
        "# Entity Observer Context".to_owned(),
        String::new(),
        format!("## Facet: {facet}"),
        format!("## Day: {day}"),
        format!("## Active Entities: {} of {total} active", total.min(6)),
        String::new(),
        "### Entities".to_owned(),
        String::new(),
    ];
    for (index, (entity, _)) in active.into_iter().take(6).enumerate() {
        if index > 0 {
            lines.extend([String::new(), "---".to_owned(), String::new()]);
        }
        let name = entity
            .identity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&entity.entity_id);
        lines.push(format!("#### {name} ({})", entity.entity_id));
        lines.push(format!(
            "- Type: {}",
            entity
                .identity
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ));
        lines.push(format!(
            "- Description: {}",
            entity
                .identity
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ));
        lines.extend([String::new(), "Current observations:".to_owned()]);
        let observations =
            solstone_core_facets::load_observations(journal, facet, &entity.relationship_dir)
                .unwrap_or_default();
        if observations.is_empty() {
            lines.push("No current observations.".to_owned());
        }
        for (index, observation) in observations.iter().enumerate() {
            if let Some(rendered) = observation_context(index, observation) {
                lines.push(rendered);
            }
        }
    }
    Ok(lines.join("\n").chars().take(24_000).collect())
}

// Keep the identifying quote distinct from both the full content and provenance.
// The writer still verifies it against the current observation at this index.
fn observation_context(index: usize, observation: &Value) -> Option<String> {
    let content = observation.get("content")?.as_str()?.trim();
    if content.is_empty() {
        return None;
    }
    let quote: String = content.chars().take(200).collect();
    let source = observation
        .get("source_day")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
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

    #[test]
    fn context_quote_is_bounded_verbatim_and_separate_from_source() {
        for content in [
            "Short observation.".to_owned(),
            "é😀\n\"quoted\" ".repeat(80),
        ] {
            let rendered =
                observation_context(3, &json!({"content":content, "source_day":"20260712"}))
                    .unwrap();
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
        assert!(observation_context(0, &json!({"content":"  "})).is_none());
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
            )
            .unwrap();
        };
        apply(json!({"op":"add","content":content}));
        let original = solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap();
        let rendered = observation_context(0, &original[0]).unwrap();
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
        assert_eq!(
            solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap(),
            original
        );
        apply(
            json!({"op":"update","target_index":0,"target_quote":"does not match","content":"Wrong."}),
        );
        assert_eq!(
            solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap(),
            original
        );
        apply(
            json!({"op":"update","target_index":0,"target_quote":quote,"content":"Updated fact."}),
        );
        let updated = solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap();
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
        apply_result(root.path(), "not json", "work", "20260101").unwrap();
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
        )
        .unwrap();
        let observations =
            solstone_core_facets::load_observations(root.path(), "work", "ada").unwrap();
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
        )
        .unwrap();
        let observations =
            solstone_core_facets::load_observations(root.path(), "work", "legacy-ada").unwrap();
        assert_eq!(observations[0]["content"], "from apply");
        assert!(
            solstone_core_facets::load_observations(root.path(), "work", "effective-ada")
                .unwrap()
                .is_empty()
        );
    }
}
