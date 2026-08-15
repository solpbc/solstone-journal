// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-observer hook.
use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};
use serde_json::{Map, Value};
#[derive(Clone, Debug, PartialEq)]
pub struct ObserverState {
    context: Option<String>,
}
pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let facet = prepared.config.get("facet").and_then(Value::as_str);
    let day = prepared.config.get("day").and_then(Value::as_str);
    if let (Some(facet), Some(day)) = (facet, day) {
        return Ok(PrePostState::EntityObserver(ObserverState {
            context: Some(
                assemble_observer_context(&context.journal, facet, day).map_err(|e| {
                    RuntimeOutcome::StageFailed(stage_error(
                        "build",
                        "entities:entity_observer",
                        prepared,
                        e,
                    ))
                })?,
            ),
        }));
    } // Preserve solstone/apps/entities/talent/entity_observer.py:36-38: bare None leaves template unchanged.
    Ok(PrePostState::EntityObserver(ObserverState {
        context: None,
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
pub fn apply_result(
    journal: &std::path::Path,
    output: &str,
    facet: &str,
    day: &str,
) -> Result<(), String> {
    let Ok(Value::Object(data)) = serde_json::from_str(output) else {
        return Ok(());
    };
    let attached = solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false)
        .map_err(|e| e.to_string())?;
    for entry in data
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        let Some(id) = entry.get("entity_id").and_then(Value::as_str) else {
            continue;
        };
        if !attached.iter().any(|entity| entity.entity_id == id) {
            continue;
        }
        let operations = entry
            .get("operations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let cleaned = operations
            .into_iter()
            .filter(operation_is_valid)
            .collect::<Vec<_>>();
        let _ =
            solstone_core_facets::record_observation_ops(journal, facet, id, &cleaned, Some(day));
    }
    Ok(())
}

fn operation_is_valid(operation: &Value) -> bool {
    let Some(op) = operation.get("op").and_then(Value::as_str) else {
        return false;
    };
    if !matches!(op, "add" | "update" | "drop" | "keep") {
        return false;
    }
    operation
        .get("relation")
        .and_then(Value::as_object)
        .is_none_or(|relation| {
            relation
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| crate::story::RELATIONS.contains(&kind))
        })
}
fn assemble_observer_context(
    journal: &std::path::Path,
    facet: &str,
    day: &str,
) -> Result<String, String> {
    let entities = solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false)
        .map_err(|e| e.to_string())?;
    if entities.is_empty() {
        return Ok("No active entities found for this day.".to_owned());
    }
    let mut lines = vec![
        "# Entity Observer Context".to_owned(),
        String::new(),
        format!("## Facet: {facet}"),
        format!("## Day: {day}"),
        String::new(),
        "### Entities".to_owned(),
    ];
    for entity in entities.into_iter().take(12) {
        let name = entity
            .identity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let observations =
            solstone_core_facets::load_observations(journal, facet, &entity.entity_id)
                .map_err(|e| e.to_string())?;
        lines.push(format!("\n## {} ({})", name, entity.entity_id));
        for (index, observation) in observations.iter().enumerate() {
            lines.push(format!(
                "{index}. {}",
                observation
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ));
        }
    }
    Ok(lines.join("\n"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_scope_keeps_observer_placeholder() {
        // Derived from solstone/apps/entities/talent/entity_observer.py:36-38 and entity_observer.md:30.
        let root = tempfile::tempdir().unwrap();
        let mut prepared = PreparedTalent {
            name: "entity_observer".to_owned(),
            config: Map::from_iter([(
                "prompt".to_owned(),
                Value::String("Before $observer_context After".to_owned()),
            )]),
        };
        let state = build(
            &mut prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        )
        .unwrap();
        apply_prompt_override(&mut prepared, &state).unwrap();
        assert!(
            prepared.config["prompt"]
                .as_str()
                .unwrap()
                .contains("$observer_context")
        );
    }
}
