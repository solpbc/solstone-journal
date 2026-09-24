// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Structural speaker-attribution heuristics, the second attribution layer.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde_json::json;
use solstone_core_entity::{
    EntityResolutionEntity, EntityResolutionError, EntityResolutionOutcome, JournalEntity,
    record_entity_resolution_from_name_evidence,
};

use crate::admission::{
    admissible_person_pool, admissible_resolution_entities, saved_choice_excluded_by_admission,
};
use crate::evidence::{
    CandidateEvidence, assemble_candidate_evidence, candidate_name_channels, ordered_dedup,
};
use crate::layer1::Label;
use solstone_core_speaker_id::calibration::RESOLUTION_FUZZY_THRESHOLD;

/// Inputs loaded once by the attribution orchestrator for structural attribution.
pub struct Layer2Inputs<'a> {
    pub speakers: &'a [String],
    pub setting_names: &'a [String],
    pub screen_names: &'a [String],
    pub meeting_names: &'a [String],
    pub entities: &'a [JournalEntity],
    pub all_entities: &'a [JournalEntity],
    pub non_owner_sids: &'a [i64],
    pub margin_declined_sids: &'a HashSet<i64>,
    pub journal_root: &'a Path,
    pub day: &'a str,
    pub segment_key: &'a str,
    pub read_only: bool,
}

/// Structural-layer state consumed by acoustic attribution and the final output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer2Result {
    pub labels: BTreeMap<i64, Label>,
    pub candidate_entity_ids: BTreeSet<String>,
    pub candidate_evidence: Vec<CandidateEvidence>,
    pub resolved_candidate_names: Vec<String>,
}

/// Apply candidate resolution and the single-speaker structural heuristics.
pub fn apply_structural_heuristics(
    mut labels: BTreeMap<i64, Label>,
    inputs: Layer2Inputs<'_>,
) -> Result<Layer2Result, EntityResolutionError> {
    let name_channels = candidate_name_channels(
        inputs.speakers,
        inputs.setting_names,
        inputs.screen_names,
        inputs.meeting_names,
    );
    let candidate_names = ordered_dedup(
        inputs
            .speakers
            .iter()
            .chain(inputs.setting_names)
            .chain(inputs.screen_names)
            .chain(inputs.meeting_names),
    );
    let available_entities = inputs
        .entities
        .iter()
        .filter(|entity| !entity.is_blocked())
        .collect::<Vec<_>>();
    let all_entity_refs = inputs.all_entities.iter().collect::<Vec<_>>();
    let pool = admissible_person_pool(&available_entities);
    let resolution_entities = admissible_resolution_entities(&pool);
    let mut candidate_entity_ids = BTreeSet::new();
    let mut name_entity_ids = HashMap::new();

    for name in &candidate_names {
        if let Some(entity) = resolve_entity(
            &inputs,
            &all_entity_refs,
            &pool,
            &resolution_entities,
            name,
            "candidate_name",
        )? {
            candidate_entity_ids.insert(entity.id.clone());
            name_entity_ids.insert(name.clone(), entity.id.clone());
        }
    }

    let candidate_evidence = assemble_candidate_evidence(&name_channels, &name_entity_ids);
    let resolved_candidate_names = candidate_names
        .iter()
        .filter(|name| name_entity_ids.contains_key(*name))
        .cloned()
        .collect();

    if inputs.speakers.len() == 1 {
        if let Some(entity) = resolve_entity(
            &inputs,
            &all_entity_refs,
            &pool,
            &resolution_entities,
            &inputs.speakers[0],
            "structural_single_speaker",
        )? {
            apply_replacement_labels(
                &mut labels,
                inputs.non_owner_sids,
                inputs.margin_declined_sids,
                &entity.id,
                "structural_single_speaker",
            );
        }
    } else if inputs.speakers.is_empty()
        && inputs.setting_names.len() == 1
        && let Some(entity) = resolve_entity(
            &inputs,
            &all_entity_refs,
            &pool,
            &resolution_entities,
            &inputs.setting_names[0],
            "structural_setting",
        )?
    {
        apply_replacement_labels(
            &mut labels,
            inputs.non_owner_sids,
            inputs.margin_declined_sids,
            &entity.id,
            "structural_setting",
        );
    }

    Ok(Layer2Result {
        labels,
        candidate_entity_ids,
        candidate_evidence,
        resolved_candidate_names,
    })
}

fn resolve_entity<'a>(
    inputs: &Layer2Inputs<'_>,
    all_entities: &[&JournalEntity],
    pool: &[&'a JournalEntity],
    resolution_entities: &[EntityResolutionEntity],
    name: &str,
    field: &str,
) -> Result<Option<&'a JournalEntity>, EntityResolutionError> {
    let scope = json!({"kind": "journal"});
    if saved_choice_excluded_by_admission(inputs.journal_root, &scope, name, all_entities)? {
        return Ok(None);
    }
    let resolution = record_entity_resolution_from_name_evidence(
        inputs.journal_root,
        name,
        resolution_entities,
        scope,
        json!({
            "lane": "apps.speakers.attribution",
            "day": inputs.day,
            "segment_id": inputs.segment_key,
            "field": field,
        }),
        RESOLUTION_FUZZY_THRESHOLD,
        inputs.read_only,
    )?;
    if resolution.outcome != EntityResolutionOutcome::Resolved {
        return Ok(None);
    }
    Ok(resolution
        .entity_index
        .and_then(|index| pool.get(index).copied()))
}

fn apply_replacement_labels(
    labels: &mut BTreeMap<i64, Label>,
    non_owner_sids: &[i64],
    margin_declined_sids: &HashSet<i64>,
    entity_id: &str,
    method: &str,
) {
    for sentence_id in non_owner_sids {
        let Some(label) = labels.get_mut(sentence_id) else {
            continue;
        };
        if label.speaker.is_some() {
            continue;
        }
        let margin_declined = margin_declined_sids.contains(sentence_id);
        label.speaker = Some(entity_id.to_owned());
        label.confidence = Some(if margin_declined { "medium" } else { "high" }.to_owned());
        label.method = Some(method.to_owned());
        if margin_declined {
            label.owner_margin_declined = Some(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    #[test]
    fn resolve_entity_speaker_attribution_lane_rules() {
        let temporary = TempDir::new().unwrap();
        let person_entity = JournalEntity {
            id: "dr_marcus_vance".to_owned(),
            value: json!({
                "id": "dr_marcus_vance",
                "name": "Dr. Marcus Vance",
                "type": "Person",
                "aka": ["Marc Vance"],
                "blocked": false
            }),
        };

        let tool_entity = JournalEntity {
            id: "quantum_compiler_tool".to_owned(),
            value: json!({
                "id": "quantum_compiler_tool",
                "name": "Quantum Compiler Tool",
                "type": "Tool",
                "aka": ["QCompiler"],
                "blocked": false
            }),
        };

        let all_entities = vec![&person_entity, &tool_entity];
        let pool = vec![&person_entity];
        let resolution_entities = vec![person_entity.resolution_entity()];

        let empty_names: Vec<String> = Vec::new();
        let entity_slice = vec![person_entity.clone(), tool_entity.clone()];
        let sids: Vec<i64> = Vec::new();
        let margin_declined: HashSet<i64> = HashSet::new();

        let inputs = Layer2Inputs {
            speakers: &empty_names,
            setting_names: &empty_names,
            screen_names: &empty_names,
            meeting_names: &empty_names,
            entities: &entity_slice,
            all_entities: &entity_slice,
            non_owner_sids: &sids,
            margin_declined_sids: &margin_declined,
            journal_root: temporary.path(),
            day: "20260804",
            segment_key: "s1",
            read_only: false,
        };

        // 1. Person exact resolves
        let res1 = resolve_entity(&inputs, &all_entities, &pool, &resolution_entities, "Dr. Marcus Vance", "test_field").unwrap();
        assert_eq!(res1.map(|e| e.id.as_str()), Some("dr_marcus_vance"));

        // 2. Person alias resolves
        let res2 = resolve_entity(&inputs, &all_entities, &pool, &resolution_entities, "Marc Vance", "test_field").unwrap();
        assert_eq!(res2.map(|e| e.id.as_str()), Some("dr_marcus_vance"));

        // 3. Saved Person choice resolves
        let obs3 = solstone_core_entity::AmbiguityObservation {
            scope: json!({"kind": "journal"}),
            query: "Vance".to_owned(),
            normalized_query: "vance".to_owned(),
            observed_tier: 5,
            ranked_candidates: vec![json!({"id": "dr_marcus_vance", "name": "Dr. Marcus Vance", "tier": 5, "score": 80.0})],
            origin: json!({"lane": "segment", "day": "20260804", "segment_id": "s1"}),
        };
        solstone_core_entity::record_ambiguity_observation(temporary.path(), &obs3).unwrap();

        let choice_req = solstone_core_entity::AmbiguityChoiceRequest {
            scope: json!({"kind": "journal"}),
            query: "Vance".to_owned(),
            entity_id: "dr_marcus_vance".to_owned(),
            origin: None,
        };
        let eligible = vec![solstone_core_entity::AmbiguityChoiceEntity { id: "dr_marcus_vance".to_owned(), blocked: false }];
        solstone_core_entity::record_ambiguity_choice(temporary.path(), &choice_req, &eligible).unwrap();
        let res3 = resolve_entity(&inputs, &all_entities, &pool, &resolution_entities, "Vance", "test_field").unwrap();
        assert_eq!(res3.map(|e| e.id.as_str()), Some("dr_marcus_vance"));

        // 4. Non-Person exact does not resolve
        let res4 = resolve_entity(&inputs, &all_entities, &pool, &resolution_entities, "Quantum Compiler Tool", "test_field").unwrap();
        assert_eq!(res4, None);

        // 5. Non-Person alias does not resolve
        let res5 = resolve_entity(&inputs, &all_entities, &pool, &resolution_entities, "QCompiler", "test_field").unwrap();
        assert_eq!(res5, None);

        // 6. Saved Non-Person choice does not resolve
        let obs6 = solstone_core_entity::AmbiguityObservation {
            scope: json!({"kind": "journal"}),
            query: "Compiler".to_owned(),
            normalized_query: "compiler".to_owned(),
            observed_tier: 5,
            ranked_candidates: vec![json!({"id": "quantum_compiler_tool", "name": "Quantum Compiler Tool", "tier": 5, "score": 80.0})],
            origin: json!({"lane": "segment", "day": "20260804", "segment_id": "s1"}),
        };
        solstone_core_entity::record_ambiguity_observation(temporary.path(), &obs6).unwrap();

        let tool_choice = solstone_core_entity::AmbiguityChoiceRequest {
            scope: json!({"kind": "journal"}),
            query: "Compiler".to_owned(),
            entity_id: "quantum_compiler_tool".to_owned(),
            origin: None,
        };
        let tool_eligible = vec![solstone_core_entity::AmbiguityChoiceEntity { id: "quantum_compiler_tool".to_owned(), blocked: false }];
        solstone_core_entity::record_ambiguity_choice(temporary.path(), &tool_choice, &tool_eligible).unwrap();
        let res6 = resolve_entity(&inputs, &all_entities, &pool, &resolution_entities, "Compiler", "test_field").unwrap();
        assert_eq!(res6, None);
    }
}
