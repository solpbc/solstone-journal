// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-review hook.
use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};
use serde_json::{Map, Value};
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewState {
    packet: String,
}
pub fn gate(prepared: &PreparedTalent, _: &ExecutionContext) -> Result<GateDecision, StageError> {
    if prepared.config.get("day").and_then(Value::as_str).is_none() {
        return Ok(GateDecision::Skip("no_day".to_owned()));
    }
    if prepared
        .config
        .get("facet")
        .and_then(Value::as_str)
        .is_none()
    {
        return Ok(GateDecision::Skip("no_facet".to_owned()));
    }
    Ok(GateDecision::Proceed)
}
pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let facet = prepared
        .config
        .get("facet")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let day = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let rows = solstone_core_facets::read_detected_entities(&context.journal, facet, day).map_err(
        |e| {
            RuntimeOutcome::StageFailed(stage_error(
                "build",
                "entities:entities_review",
                prepared,
                e.to_string(),
            ))
        },
    )?;
    Ok(PrePostState::EntitiesReview(ReviewState {
        packet: serde_json::to_string_pretty(&rows).expect("values serialize"),
    }))
}
pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::EntitiesReview(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "entities:entities_review",
            prepared,
            "missing review state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([(
            "review_packet".to_owned(),
            Value::String(state.packet.clone()),
        )]),
    );
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
            "entities:entities_review",
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
    Ok(CommitPlan::Write(WriteIntent::EntitiesReview {
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
    for row in data
        .get("promotions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        if row.get("promote").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let (Some(name), Some(description)) = (
            row.get("name").and_then(Value::as_str),
            row.get("description").and_then(Value::as_str),
        ) else {
            continue;
        };
        let typ = "Other";
        let outcome = solstone_core_facets::attach_or_reactivate_entity(
            journal,
            facet,
            typ,
            name,
            description,
        )
        .map_err(|e| e.to_string())?;
        let Some(entity_id) = outcome
            .relationship
            .get("entity_id")
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Some(aliases) = row.get("aliases").and_then(Value::as_array) {
            for alias in aliases.iter().filter_map(Value::as_str) {
                let _ = solstone_core_facets::add_entity_aka(journal, facet, entity_id, alias);
            }
        }
    }
    for row in data
        .get("merges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        let (Some(source), Some(target), Some(evidence)) = (
            row.get("source").and_then(Value::as_str),
            row.get("canonical").and_then(Value::as_str),
            row.get("evidence").and_then(Value::as_str),
        ) else {
            continue;
        };
        solstone_core_entity::record_merge_candidate(
            journal,
            facet,
            day,
            source,
            &solstone_core_entity_matching::entity_slug(source),
            target,
            &solstone_core_entity_matching::entity_slug(target),
            evidence,
            Some("name-variant"),
            None,
            None,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
