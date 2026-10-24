// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The text the model reads and the answer it must give, ported byte for
//! byte from the evaluated prototype (`V2.build`, `V2.parse`): the prompt was
//! tuned against exactly this rendering.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use solstone_core_possible_names::CandidateEntity;

use super::policy::{MAX_ALIASES_SHOWN, NOTE_CHARS};
use super::retrieval::{Action, Statement};

const YES: &str = "yes";
const NO: &str = "no";

/// Candidate `index`'s answer key.
pub(crate) fn candidate_id(index: usize) -> String {
    format!("N{index}")
}

/// The packet for `candidates` (presentation order) over the whole
/// transcript. `facets` is the segment's own facet list.
pub(crate) fn render(
    statements: &[Statement],
    facets: &[String],
    candidates: &[Action],
    registry: &BTreeMap<&str, &CandidateEntity>,
) -> String {
    let line_of = statements
        .iter()
        .enumerate()
        .map(|(index, statement)| (statement.sentence, index + 1))
        .collect::<BTreeMap<_, _>>();
    let mut marks = BTreeMap::<u64, BTreeSet<(usize, usize)>>::new();
    for candidate in candidates {
        marks
            .entry(candidate.sentence)
            .or_default()
            .insert((candidate.start_byte, candidate.end_byte));
    }
    let lines = statements
        .iter()
        .enumerate()
        .map(|(index, statement)| {
            let mut text = statement.text.as_bytes().to_vec();
            for (start, end) in marks.get(&statement.sentence).into_iter().flatten().rev() {
                let mut marked = Vec::with_capacity(text.len() + 4);
                marked.extend_from_slice(&text[..*start]);
                marked.extend_from_slice(b"[[");
                marked.extend_from_slice(&text[*start..*end]);
                marked.extend_from_slice(b"]]");
                marked.extend_from_slice(&text[*end..]);
                text = marked;
            }
            format!("[{}] {}", index + 1, String::from_utf8_lossy(&text))
        })
        .collect::<Vec<_>>();
    let ids = (0..candidates.len()).map(candidate_id).collect::<Vec<_>>();
    let cards = candidates
        .iter()
        .zip(&ids)
        .map(|(candidate, id)| {
            let entity = registry[candidate.entity_id.as_str()];
            let mut parts = vec![format!("{id}: \"{}\"", entity.name)];
            if !entity.entity_type.is_empty() {
                parts[0].push_str(&format!(" ({})", entity.entity_type));
            }
            parts.push(format!(
                "matched [[{}]] in line {}: {}",
                candidate.heard,
                line_of
                    .get(&candidate.sentence)
                    .copied()
                    .unwrap_or_default(),
                candidate.route.basis()
            ));
            if !entity.aka.is_empty() {
                parts.push(format!(
                    "also known as: {}",
                    entity
                        .aka
                        .iter()
                        .take(MAX_ALIASES_SHOWN)
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            parts.push(format!(
                "filed under: {}",
                entity
                    .facets
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            let note = facets
                .iter()
                .filter_map(|facet| entity.facets.get(facet))
                .find(|description| !description.is_empty())
                .or_else(|| {
                    entity
                        .facets
                        .values()
                        .find(|description| !description.is_empty())
                });
            if let Some(note) = note {
                parts.push(format!(
                    "note: {}",
                    note.chars().take(NOTE_CHARS).collect::<String>()
                ));
            }
            let rivals = candidates
                .iter()
                .zip(&ids)
                .filter(|(other, other_id)| {
                    *other_id != id
                        && other.sentence == candidate.sentence
                        && other.start_byte == candidate.start_byte
                })
                .map(|(_, other_id)| other_id.as_str())
                .collect::<Vec<_>>();
            if !rivals.is_empty() {
                parts.push(format!(
                    "other candidates for the same words: {}",
                    rivals.join(", ")
                ));
            }
            parts.join("\n   ")
        })
        .collect::<Vec<_>>();
    let filed = if facets.is_empty() {
        "unknown".to_owned()
    } else {
        facets.join(", ")
    };
    format!(
        "This recording is filed under: {filed}\n\nTRANSCRIPT\n{}\n\nCANDIDATES\n{}",
        lines.join("\n"),
        cards.join("\n")
    )
}

/// One required `yes`/`no` answer per candidate, nothing else.
pub(crate) fn schema(count: usize) -> Value {
    let ids = (0..count).map(candidate_id).collect::<Vec<_>>();
    let properties = ids
        .iter()
        .map(|id| (id.clone(), json!({"type": "string", "enum": [YES, NO]})))
        .collect::<Map<_, _>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": ids,
        "additionalProperties": false,
    })
}

/// The indices answered `yes`. Every candidate must be answered, with
/// nothing else, each `yes` or `no`.
pub(crate) fn parse(output: &str, count: usize) -> Result<Vec<usize>, String> {
    let value = serde_json::from_str::<Value>(output)
        .map_err(|error| format!("answer is not JSON: {error}"))?;
    let Value::Object(answers) = value else {
        return Err("answer is not a JSON object".to_owned());
    };
    let expected = (0..count).map(candidate_id).collect::<BTreeSet<_>>();
    let given = answers.keys().cloned().collect::<BTreeSet<_>>();
    if given != expected {
        return Err(format!(
            "answer keys {:?} are not the candidates {:?}",
            given, expected
        ));
    }
    let mut accepted = Vec::new();
    for index in 0..count {
        match answers[&candidate_id(index)].as_str() {
            Some(YES) => accepted.push(index),
            Some(NO) => {}
            other => {
                return Err(format!(
                    "answer for {} is {other:?}, not \"yes\" or \"no\"",
                    candidate_id(index)
                ));
            }
        }
    }
    Ok(accepted)
}
