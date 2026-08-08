// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Structural speaker-attribution heuristics, the second attribution layer.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use serde_json::json;
use solstone_core_entity::{
    EntityResolutionEntity, EntityResolutionError, EntityResolutionOutcome, JournalEntity,
    record_entity_resolution,
};

use crate::calibration::RESOLUTION_FUZZY_THRESHOLD;
use crate::evidence::{
    CandidateEvidence, assemble_candidate_evidence, candidate_name_channels, ordered_dedup,
};
use crate::layer1::Label;
use crate::person_guard::is_admissible_person;

/// Inputs loaded once by the attribution orchestrator for structural attribution.
pub struct Layer2Inputs<'a> {
    pub speakers: &'a [String],
    pub setting_names: &'a [String],
    pub screen_names: &'a [String],
    pub meeting_names: &'a [String],
    pub entities: &'a [JournalEntity],
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
    pub candidate_names: Vec<String>,
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
    let resolution_entities = available_entities
        .iter()
        .map(|entity| entity.resolution_entity())
        .collect::<Vec<EntityResolutionEntity>>();
    let mut candidate_entity_ids = BTreeSet::new();
    let mut name_entity_ids = HashMap::new();

    for name in &candidate_names {
        if let Some(entity) = resolve_entity(
            &inputs,
            &resolution_entities,
            &available_entities,
            name,
            "candidate_name",
        )? && is_admissible_person(entity.entity_type())
        {
            candidate_entity_ids.insert(entity.id.clone());
            name_entity_ids.insert(name.clone(), entity.id.clone());
        }
    }

    let candidate_evidence = assemble_candidate_evidence(&name_channels, &name_entity_ids);

    if inputs.speakers.len() == 1 {
        if let Some(entity) = resolve_entity(
            &inputs,
            &resolution_entities,
            &available_entities,
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
            &resolution_entities,
            &available_entities,
            &inputs.setting_names[0],
            "structural_setting",
        )?
        && is_admissible_person(entity.entity_type())
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
        candidate_names,
    })
}

fn resolve_entity<'a>(
    inputs: &Layer2Inputs<'_>,
    resolution_entities: &[EntityResolutionEntity],
    available_entities: &[&'a JournalEntity],
    name: &str,
    field: &str,
) -> Result<Option<&'a JournalEntity>, EntityResolutionError> {
    let resolution = record_entity_resolution(
        inputs.journal_root,
        name,
        resolution_entities,
        json!({"kind": "journal"}),
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
        .and_then(|index| available_entities.get(index).copied()))
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
