// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct cluster-member voiceprint planning and crash-safe replay.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::segment_path;
use serde_json::{Value, json};
use solstone_core_entity::{
    EncoderIdentity, VoiceprintItem, VoiceprintRemoval, VoiceprintRemovalReport,
    load_entity_voiceprints_file, load_existing_voiceprint_keys, normalize_embedding,
    remove_voiceprints_by_key, save_voiceprints_batch,
};
use solstone_core_speaker_id::embeddings::load_embeddings_file;
use thiserror::Error;

use crate::identify_operations::{ForwardPhase, MemberProvenance};
use crate::owner_admission::{OWNER_IDENTITY_INVALID_REASON, OwnerAdmission, admitted_owner_id};
use crate::owner_centroid::{OwnerCentroid, OwnerCentroidError, load_owner_centroid};
use crate::voiceprint_metadata::VoiceprintMetadata;

/// The four metadata values that identify a direct voiceprint row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DirectVoiceprintKey {
    pub day: String,
    pub segment_key: String,
    pub source: String,
    pub sentence_id: i64,
}

impl DirectVoiceprintKey {
    /// Serialize this key in the durable direct-voiceprint checkpoint shape.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "day": self.day,
            "segment_key": self.segment_key,
            "source": self.source,
            "sentence_id": self.sentence_id,
        })
    }
}

/// One planned direct voiceprint, retaining its source for a replay after a crash.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectVoiceprintEntry {
    pub key: DirectVoiceprintKey,
    pub metadata: Value,
    pub source_member: MemberProvenance,
}

/// Durable direct-voiceprint portion of a prepared identify plan.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectVoiceprintsPlan {
    pub target_entity_id: String,
    pub preexisting_keys: Vec<DirectVoiceprintKey>,
    pub entries_to_add: Vec<DirectVoiceprintEntry>,
}

/// Planning output, including immediate items while retaining a serializable plan for replay.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectVoiceprintsPlanning {
    pub plan: DirectVoiceprintsPlan,
    pub items: Vec<VoiceprintItem>,
}

/// Completed direct-voiceprint phase facts for the ledger checkpoint payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectVoiceprintsPhaseResult {
    pub saved_keys: Vec<DirectVoiceprintKey>,
    pub saved_count: usize,
    pub skipped_existing_count: usize,
}

impl DirectVoiceprintsPhaseResult {
    /// Return the phase-specific checkpoint fields validated by the identify ledger.
    #[must_use]
    pub fn checkpoint_fields(&self) -> Value {
        json!({
            "saved_keys": self.saved_keys.iter().map(DirectVoiceprintKey::to_json).collect::<Vec<_>>(),
            "saved_count": self.saved_count,
            "skipped_existing_count": self.skipped_existing_count,
            "counts": {"saved": self.saved_count},
            "skipped_reasons": {"existing": self.skipped_existing_count},
        })
    }
}

/// Failure while planning or replaying direct voiceprints.
#[derive(Debug, Error)]
pub enum DirectVoiceprintsError {
    #[error("speaker_owner_identity_invalid")]
    OwnerIdentityInvalid,
    #[error("owner centroid lookup failed: {0}")]
    Owner(#[from] OwnerCentroidError),
    #[error("segment path failed: {0}")]
    Path(#[from] solstone_core_journal_io::PathError),
    #[error("voiceprint operation failed: {0}")]
    Voiceprint(#[from] solstone_core_entity::VoiceprintOperationError),
    #[error("repair required during {phase:?}: {code}")]
    RepairRequired {
        phase: ForwardPhase,
        code: &'static str,
        categories: BTreeMap<String, usize>,
        /// `None` lets the identify orchestrator record pending phases; `Some` is verbatim.
        partial_report: Option<Value>,
    },
}

/// Failure while applying an authenticated, unguarded single-row voiceprint mutation.
#[derive(Debug, Error)]
pub enum DirectVoiceprintMutationError {
    #[error("voiceprint metadata must contain day, segment_key, source, and sentence_id")]
    InvalidMetadata,
    #[error("voiceprint operation failed: {0}")]
    Voiceprint(#[from] solstone_core_entity::VoiceprintOperationError),
}

/// Append exactly one caller-supplied voiceprint without identify/accumulation guards.
pub fn write_voiceprint(
    journal_root: &Path,
    entity_id: &str,
    embedding: Vec<f32>,
    metadata: Value,
    encoder: &EncoderIdentity,
) -> Result<(), DirectVoiceprintMutationError> {
    if !has_direct_key(&metadata) {
        return Err(DirectVoiceprintMutationError::InvalidMetadata);
    }
    save_voiceprints_batch(
        journal_root,
        entity_id,
        &[VoiceprintItem {
            embedding,
            metadata,
        }],
        encoder,
    )?;
    Ok(())
}

/// Remove matching rows for one direct voiceprint key without identify guards.
pub fn remove_voiceprint(
    journal_root: &Path,
    entity_id: &str,
    key: Value,
    encoder: &EncoderIdentity,
) -> Result<VoiceprintRemovalReport, DirectVoiceprintMutationError> {
    if !has_direct_key(&key) {
        return Err(DirectVoiceprintMutationError::InvalidMetadata);
    }
    Ok(remove_voiceprints_by_key(
        journal_root,
        entity_id,
        &[VoiceprintRemoval {
            key,
            expected_metadata: None,
        }],
        encoder,
    )?)
}

fn has_direct_key(metadata: &Value) -> bool {
    let Some(object) = metadata.as_object() else {
        return false;
    };
    object.get("day").and_then(Value::as_str).is_some()
        && object.get("segment_key").and_then(Value::as_str).is_some()
        && object.get("source").and_then(Value::as_str).is_some()
        && object.get("sentence_id").and_then(Value::as_i64).is_some()
}

/// Snapshot direct cluster-member voiceprints that are not already on the target.
pub fn plan_direct_voiceprints(
    journal_root: &Path,
    target_entity_id: &str,
    cluster_members: &[MemberProvenance],
    added_at: i64,
) -> Result<DirectVoiceprintsPlanning, DirectVoiceprintsError> {
    let existing_keys = load_existing_voiceprint_keys(journal_root, target_entity_id)
        .into_iter()
        .filter_map(|key| direct_key_from_voiceprint_key(&key))
        .collect::<BTreeSet<_>>();
    let mut working_keys = existing_keys.clone();
    let owner = current_owner_centroid(journal_root)?;
    let mut members = cluster_members.to_vec();
    members.sort();
    let mut entries_to_add = Vec::new();
    let mut items = Vec::new();

    for member in members {
        let key = direct_key(&member);
        if working_keys.contains(&key) {
            continue;
        }
        let Some(embedding) = load_member_embedding(journal_root, &member, owner.as_ref())? else {
            continue;
        };
        let metadata = VoiceprintMetadata::new(
            &member.day,
            &member.segment_key,
            &member.source,
            &member.stream,
            member.sentence_id,
            added_at,
            added_at,
        )
        .to_json();
        entries_to_add.push(DirectVoiceprintEntry {
            key: key.clone(),
            metadata: metadata.clone(),
            source_member: member,
        });
        items.push(VoiceprintItem {
            embedding,
            metadata,
        });
        working_keys.insert(key);
    }

    let preexisting_keys = existing_keys.into_iter().collect::<Vec<_>>();
    Ok(DirectVoiceprintsPlanning {
        plan: DirectVoiceprintsPlan {
            target_entity_id: target_entity_id.to_owned(),
            preexisting_keys,
            entries_to_add,
        },
        items,
    })
}

/// Replay a direct-voiceprint phase without trusting a missing ledger checkpoint.
pub fn execute_direct_voiceprints_phase(
    journal_root: &Path,
    plan: &DirectVoiceprintsPlan,
    encoder: &EncoderIdentity,
) -> Result<DirectVoiceprintsPhaseResult, DirectVoiceprintsError> {
    let existing_metadata = entity_voiceprint_metadata(journal_root, &plan.target_entity_id);
    let mut saved_keys = Vec::new();
    let mut to_save = Vec::new();
    let owner = match current_owner_centroid(journal_root) {
        Ok(owner) => owner,
        Err(DirectVoiceprintsError::OwnerIdentityInvalid) => {
            return Err(DirectVoiceprintsError::RepairRequired {
                phase: ForwardPhase::DirectVoiceprints,
                code: OWNER_IDENTITY_INVALID_REASON,
                categories: BTreeMap::from([("owner_identity".to_owned(), 1)]),
                partial_report: None,
            });
        }
        Err(error) => return Err(error),
    };

    for entry in &plan.entries_to_add {
        if let Some(rows) = existing_metadata.get(&entry.key) {
            if rows.iter().any(|row| row == &entry.metadata) {
                saved_keys.push(entry.key.clone());
                continue;
            }
            return Err(repair_required(
                "voiceprint_metadata_mismatch",
                "voiceprint",
                &saved_keys,
            ));
        }
        let Some(embedding) =
            load_member_embedding(journal_root, &entry.source_member, owner.as_ref())?
        else {
            return Err(repair_required(
                "source_embedding_unavailable",
                "direct_voiceprint",
                &saved_keys,
            ));
        };
        to_save.push(VoiceprintItem {
            embedding,
            metadata: entry.metadata.clone(),
        });
    }

    if !to_save.is_empty() {
        save_voiceprints_batch(journal_root, &plan.target_entity_id, &to_save, encoder)?;
        saved_keys.extend(
            plan.entries_to_add
                .iter()
                .filter(|entry| !existing_metadata.contains_key(&entry.key))
                .map(|entry| entry.key.clone()),
        );
    }
    saved_keys.sort();
    saved_keys.dedup();
    let skipped_existing_count = plan.entries_to_add.len().saturating_sub(saved_keys.len());
    Ok(DirectVoiceprintsPhaseResult {
        saved_count: saved_keys.len(),
        saved_keys,
        skipped_existing_count,
    })
}

fn current_owner_centroid(
    journal_root: &Path,
) -> Result<Option<OwnerCentroid>, DirectVoiceprintsError> {
    let owner_id = match admitted_owner_id(journal_root) {
        OwnerAdmission::Admitted(id) => id,
        OwnerAdmission::Invalid => return Err(DirectVoiceprintsError::OwnerIdentityInvalid),
    };
    match load_owner_centroid(journal_root, &owner_id) {
        Ok(owner) => Ok(owner),
        Err(OwnerCentroidError::IdentityInvalid | OwnerCentroidError::TargetMismatch { .. }) => {
            Err(DirectVoiceprintsError::OwnerIdentityInvalid)
        }
        Err(error) => Err(DirectVoiceprintsError::Owner(error)),
    }
}

fn load_member_embedding(
    journal_root: &Path,
    member: &MemberProvenance,
    owner: Option<&OwnerCentroid>,
) -> Result<Option<Vec<f32>>, DirectVoiceprintsError> {
    let segment = segment_path(
        journal_root,
        &member.day,
        &member.segment_key,
        &member.stream,
        false,
    )?;
    let Ok(Some(embeddings)) =
        load_embeddings_file(&segment.join(format!("{}.npz", member.source)))
    else {
        return Ok(None);
    };
    let Some((_, embedding)) = embeddings
        .statements
        .iter()
        .find(|(sentence_id, _)| *sentence_id == member.sentence_id)
    else {
        return Ok(None);
    };
    let Some(embedding) = normalize_embedding(embedding) else {
        return Ok(None);
    };
    if owner.is_some_and(|owner| dot(&embedding, &owner.centroid) >= owner.threshold) {
        return Ok(None);
    }
    Ok(Some(embedding))
}

fn entity_voiceprint_metadata(
    journal_root: &Path,
    entity_id: &str,
) -> BTreeMap<DirectVoiceprintKey, Vec<Value>> {
    let mut rows = BTreeMap::<DirectVoiceprintKey, Vec<Value>>::new();
    let Some(archive) = load_entity_voiceprints_file(journal_root, entity_id) else {
        return rows;
    };
    for metadata in archive.metadata {
        let Ok(metadata) = serde_json::from_str::<Value>(&metadata) else {
            continue;
        };
        let Some(key) = direct_key_from_metadata(&metadata) else {
            continue;
        };
        rows.entry(key).or_default().push(metadata);
    }
    rows
}

fn direct_key(member: &MemberProvenance) -> DirectVoiceprintKey {
    DirectVoiceprintKey {
        day: member.day.clone(),
        segment_key: member.segment_key.clone(),
        source: member.source.clone(),
        sentence_id: member.sentence_id,
    }
}

fn direct_key_from_metadata(metadata: &Value) -> Option<DirectVoiceprintKey> {
    Some(DirectVoiceprintKey {
        day: metadata.get("day")?.as_str()?.to_owned(),
        segment_key: metadata.get("segment_key")?.as_str()?.to_owned(),
        source: metadata.get("source")?.as_str()?.to_owned(),
        sentence_id: metadata.get("sentence_id")?.as_i64()?,
    })
}

fn direct_key_from_voiceprint_key(
    key: &solstone_core_entity::VoiceprintKey,
) -> Option<DirectVoiceprintKey> {
    use solstone_core_entity::CanonicalKeyField;

    let [day, segment_key, source, sentence_id] = &key.0;
    Some(DirectVoiceprintKey {
        day: match day {
            CanonicalKeyField::Str(value) => value.clone(),
            _ => return None,
        },
        segment_key: match segment_key {
            CanonicalKeyField::Str(value) => value.clone(),
            _ => return None,
        },
        source: match source {
            CanonicalKeyField::Str(value) => value.clone(),
            _ => return None,
        },
        sentence_id: match sentence_id {
            CanonicalKeyField::Int(value) => i64::try_from(*value).ok()?,
            _ => return None,
        },
    })
}

fn repair_required(
    code: &'static str,
    category: &str,
    saved_keys: &[DirectVoiceprintKey],
) -> DirectVoiceprintsError {
    DirectVoiceprintsError::RepairRequired {
        phase: ForwardPhase::DirectVoiceprints,
        code,
        categories: BTreeMap::from([(category.to_owned(), 1)]),
        partial_report: Some(json!({
            "saved_keys": saved_keys.iter().map(DirectVoiceprintKey::to_json).collect::<Vec<_>>(),
            "saved_count": saved_keys.len(),
        })),
    }
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}
