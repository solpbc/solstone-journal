// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retroactive candidate confirmation and voiceprint backfill.

use std::collections::HashSet;
use std::path::Path;

use crate::segment_path;
use serde_json::Value;
use solstone_core_entity::{
    EncoderIdentity, VoiceprintItem, is_admissible_person, load_all_journal_entities,
    load_entity_voiceprints_file, save_voiceprints_batch,
};
use solstone_core_speaker_id::calibration::{
    NOISY_FLYWHEEL_OVERLAP_MAX, VP_OUTLIER_MIN_SAMPLES, VP_OUTLIER_MIN_SIMILARITY,
};
use solstone_core_speaker_id::embeddings::load_embeddings_file;

use crate::candidate_tracker::{
    CandidateProfile, CandidateTracker, MERGE_THRESHOLD, retroactive_voiceprint_metadata,
};
use crate::owner_admission::{OwnerAdmission, admitted_owner_id};
use crate::owner_centroid::load_owner_centroid;
use crate::voiceprint_accumulation::read_overlap_fraction;

#[derive(Debug, Clone, PartialEq)]
pub struct RetroactiveConfirmPlan {
    pub matched: bool,
    pub candidate_id: Option<i64>,
    pub entity_id: String,
    pub items: Vec<VoiceprintItem>,
}
#[derive(Debug, thiserror::Error)]
pub enum RetroactiveConfirmError {
    #[error("entity lookup failed: {0}")]
    Entity(#[from] solstone_core_entity::EntityStoreError),
    #[error("voiceprint write failed: {0}")]
    Voiceprint(#[from] solstone_core_entity::VoiceprintOperationError),
    #[error("target entity is not an admissible Person")]
    NonPerson,
    #[error("candidate pool update failed: {0}")]
    Tracker(#[from] crate::candidate_tracker::CandidateTrackerError),
}

pub fn plan_retroactive_confirm(
    journal: &Path,
    candidate: &CandidateProfile,
    centroid: &[f32],
    entity_id: &str,
    added_at: i64,
) -> RetroactiveConfirmPlan {
    let score = dot(centroid, &candidate.centroid);
    if score < MERGE_THRESHOLD {
        return empty(entity_id);
    }
    let owner_id = match admitted_owner_id(journal) {
        OwnerAdmission::Admitted(id) => id,
        OwnerAdmission::Invalid => return matched_empty(candidate, entity_id),
    };
    if owner_id == entity_id {
        return matched_empty(candidate, entity_id);
    }
    let Ok(Some(owner)) = load_owner_centroid(journal, &owner_id) else {
        return matched_empty(candidate, entity_id);
    };
    let (existing_keys, existing_count, existing_centroid) =
        voiceprint_snapshot(journal, entity_id);
    let mut keys = existing_keys;
    let mut items = Vec::new();
    for source in &candidate.source_segments {
        let Some(o) = source.as_object() else {
            continue;
        };
        let (Some(day), Some(segment_key), Some(stream), Some(kind)) = (
            o.get("day").and_then(Value::as_str),
            o.get("segment_key").and_then(Value::as_str),
            o.get("stream").and_then(Value::as_str),
            o.get("source").and_then(Value::as_str),
        ) else {
            continue;
        };
        let Ok(dir) = segment_path(journal, day, segment_key, stream, false) else {
            continue;
        };
        if !dir.exists()
            || read_overlap_fraction(&dir.join(format!("{kind}.jsonl")))
                > NOISY_FLYWHEEL_OVERLAP_MAX
        {
            continue;
        };
        let Ok(Some(file)) = load_embeddings_file(&dir.join(format!("{kind}.npz"))) else {
            continue;
        };
        let ids = o
            .get("sentence_ids")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for id in ids.iter().filter_map(Value::as_i64) {
            let Some((_, embedding)) = file.statements.iter().find(|(sid, _)| *sid == id) else {
                continue;
            };
            let Some(normal) = solstone_core_entity::normalize_embedding(embedding) else {
                continue;
            };
            if dot(&normal, &owner.centroid) >= owner.threshold {
                continue;
            };
            let key = format!("{day}|{segment_key}|{kind}|{id}");
            if keys.contains(&key)
                || (existing_count >= VP_OUTLIER_MIN_SAMPLES
                    && existing_centroid
                        .as_ref()
                        .is_some_and(|c| dot(&normal, c) < VP_OUTLIER_MIN_SIMILARITY))
            {
                continue;
            };
            keys.insert(key);
            items.push(VoiceprintItem {
                embedding: normal,
                metadata: retroactive_voiceprint_metadata(
                    day,
                    stream,
                    segment_key,
                    kind,
                    id,
                    added_at,
                    added_at,
                ),
            });
        }
    }
    RetroactiveConfirmPlan {
        matched: true,
        candidate_id: Some(candidate.cand_id),
        entity_id: entity_id.into(),
        items,
    }
}
pub fn apply_retroactive_confirm_plan(
    tracker: &mut CandidateTracker,
    journal: &Path,
    plan: &RetroactiveConfirmPlan,
    encoder: &EncoderIdentity,
) -> Result<usize, RetroactiveConfirmError> {
    if !plan.matched {
        return Ok(0);
    }
    let entities = load_all_journal_entities(journal)?;
    let entity = entities.iter().find(|entity| entity.id == plan.entity_id);
    if !entity.is_some_and(is_admissible_person) {
        return Err(RetroactiveConfirmError::NonPerson);
    }
    let existing = voiceprint_snapshot(journal, &plan.entity_id).0;
    let items = plan
        .items
        .iter()
        .filter(|item| key_for_metadata(&item.metadata).is_some_and(|k| !existing.contains(&k)))
        .cloned()
        .collect::<Vec<_>>();
    let saved = save_voiceprints_batch(journal, &plan.entity_id, &items, encoder)?;
    if let Some(id) = plan.candidate_id {
        tracker.mark_confirmed(id, &plan.entity_id)?;
    }
    Ok(saved)
}
fn empty(entity: &str) -> RetroactiveConfirmPlan {
    RetroactiveConfirmPlan {
        matched: false,
        candidate_id: None,
        entity_id: entity.into(),
        items: vec![],
    }
}
fn matched_empty(c: &CandidateProfile, entity: &str) -> RetroactiveConfirmPlan {
    RetroactiveConfirmPlan {
        matched: true,
        candidate_id: Some(c.cand_id),
        entity_id: entity.into(),
        items: vec![],
    }
}
fn voiceprint_snapshot(journal: &Path, entity: &str) -> (HashSet<String>, usize, Option<Vec<f32>>) {
    let Some(archive) = load_entity_voiceprints_file(journal, entity) else {
        return (HashSet::new(), 0, None);
    };
    let keys = archive
        .metadata
        .iter()
        .filter_map(|raw| serde_json::from_str::<Value>(raw).ok())
        .filter_map(|value| key_for_metadata(&value))
        .collect::<HashSet<_>>();
    let width = archive.envelope.encoder.as_ref().map_or(256, |e| e.width);
    let rows = archive
        .embeddings
        .chunks(width)
        .filter_map(solstone_core_entity::normalize_embedding)
        .collect::<Vec<_>>();
    let center = centroid(&rows);
    (keys, archive.rows, center)
}
fn key_for_metadata(value: &Value) -> Option<String> {
    Some(format!(
        "{}|{}|{}|{}",
        value.get("day")?.as_str()?,
        value.get("segment_key")?.as_str()?,
        value.get("source")?.as_str()?,
        value.get("sentence_id")?.as_i64()?
    ))
}
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn centroid(rows: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = rows.first()?;
    let mut sum = vec![0.; first.len()];
    for row in rows {
        for (a, b) in sum.iter_mut().zip(row) {
            *a += b
        }
    }
    solstone_core_entity::normalize_embedding(&sum)
}
