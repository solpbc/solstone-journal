// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-description pre-hook.

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_indexer_query::{SearchRequest, search};

use crate::contract::{GateDecision, PrePostState};
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

const NO_EVIDENCE: &str = "No journal evidence found for this entity.";

#[derive(Clone, Debug, PartialEq)]
pub struct EntityDescribePreState {
    entity_type: String,
    entity_name: String,
    facet: String,
    current_description: String,
    evidence: String,
}

pub fn gate(
    prepared: &PreparedTalent,
    _context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    let fields = parse_prompt(
        prepared
            .config
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    if fields.entity_name.is_empty() {
        Ok(GateDecision::Skip("missing entity name".to_owned()))
    } else {
        Ok(GateDecision::Proceed)
    }
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let mut fields = parse_prompt(
        prepared
            .config
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    fields.evidence = render_evidence(&fields.entity_name, &fields.facet, &context.journal);
    Ok(PrePostState::EntityDescribe(fields))
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::EntityDescribe(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "entities:entity_describe",
            prepared,
            "missing entity describe state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([
            (
                "entity_type".to_owned(),
                Value::String(if state.entity_type.is_empty() {
                    "Entity".to_owned()
                } else {
                    state.entity_type.clone()
                }),
            ),
            (
                "entity_name".to_owned(),
                Value::String(state.entity_name.clone()),
            ),
            (
                "facet".to_owned(),
                Value::String(if state.facet.is_empty() {
                    "(none)".to_owned()
                } else {
                    state.facet.clone()
                }),
            ),
            (
                "current_description".to_owned(),
                Value::String(if state.current_description.is_empty() {
                    "(none)".to_owned()
                } else {
                    state.current_description.clone()
                }),
            ),
            ("evidence".to_owned(), Value::String(state.evidence.clone())),
        ]),
    );
    Ok(())
}

fn parse_prompt(prompt: &str) -> EntityDescribePreState {
    let mut fields = EntityDescribePreState {
        entity_type: String::new(),
        entity_name: String::new(),
        facet: String::new(),
        current_description: String::new(),
        evidence: String::new(),
    };
    for line in prompt.lines() {
        for (prefix, target) in [
            ("Entity Type:", 0),
            ("Entity Name:", 1),
            ("Facet:", 2),
            ("Current Description:", 3),
        ] {
            if let Some(value) = line.strip_prefix(prefix) {
                match target {
                    0 => fields.entity_type = value.trim().to_owned(),
                    1 => fields.entity_name = value.trim().to_owned(),
                    2 => fields.facet = value.trim().to_owned(),
                    _ => fields.current_description = value.trim().to_owned(),
                }
                break;
            }
        }
    }
    if fields.current_description == "(none)" {
        fields.current_description.clear();
    }
    fields
}

fn render_evidence(entity_name: &str, facet: &str, journal: &std::path::Path) -> String {
    let mut request = SearchRequest::new(entity_name, Default::default());
    request.limit = 5;
    request.facet = (!facet.is_empty()).then_some(facet.to_owned());
    match search(journal, &request, Utc::now().date_naive()) {
        Ok(response) if response.results.is_empty() => NO_EVIDENCE.to_owned(),
        Ok(response) => response
            .results
            .into_iter()
            .map(|result| {
                format!(
                    "- {} [{}, {}]: {}",
                    result.id,
                    result.metadata.day,
                    result.metadata.facet,
                    single_line(&result.text)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        // Preserve solstone/apps/entities/talent/entity_describe.py:53-58.
        Err(error) => format!("Journal evidence unavailable: {error}"),
    }
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_labeled_prompt_lines() {
        // Derived from solstone/apps/entities/talent/entity_describe.py:31-48.
        let fields = parse_prompt("Entity Name: Ada\nCurrent Description: (none)\nFacet: work");
        assert_eq!(fields.entity_name, "Ada");
        assert_eq!(fields.current_description, "");
        assert_eq!(fields.facet, "work");
    }
}
