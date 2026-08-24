// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Target-resolution front half of discovery-cluster identify planning.

use std::path::{Path, PathBuf};

use serde_json::json;
use solstone_core_entity::{
    EntityResolutionError, EntityResolutionOutcome, EntityTrustLockError, JournalEntity,
    entity_identity_destination_occupied, hold_entity_trust_lock, is_admissible_person,
    is_valid_entity_type, load_all_journal_entities, load_entity_voiceprints_file,
    read_entity_identity, read_identity_map, record_entity_resolution_from_name_evidence,
};
use solstone_core_entity_matching::{MatchTier, entity_slug, token_sort};
use thiserror::Error;

use crate::eligibility::{
    EligibilityError, current_principal_id, eligible_speaker_attach_entities,
    principal_name_collision, speaker_attach_rejection_reason,
};

const RESOLUTION_FUZZY_THRESHOLD: f64 = 90.0;

/// Inputs used before identify planning constructs any write-side artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifyTargetRequest {
    pub journal_root: PathBuf,
    pub cluster_id: i64,
    pub name: Option<String>,
    pub entity_id: Option<String>,
    pub resolve_only: bool,
    pub create_new: bool,
    pub entity_type: String,
    pub reviewed_near_match_entity_ids: Vec<String>,
}

/// A visible ambiguous name-resolution candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentifyCandidateRow {
    pub id: String,
    pub name: String,
    pub tier: i64,
    pub score: f64,
    pub has_voice: bool,
}

/// A Person target resolved or reserved for later identify-plan assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetResolution {
    pub entity_id: String,
    pub entity_name: String,
    pub entity_type: String,
    pub will_create: bool,
    /// Visible near-match IDs that a create request must explicitly review.
    pub visible_candidate_ids: Vec<String>,
}

/// Result of target resolution before any identify execution can occur.
#[derive(Debug, Clone, PartialEq)]
pub enum IdentifyTargetOutcome {
    Ready(TargetResolution),
    Resolved {
        entity_id: String,
        entity_name: String,
        has_voice: bool,
    },
    Ambiguous {
        ambiguity_id: Option<String>,
        candidates: Vec<IdentifyCandidateRow>,
    },
    NoMatch {
        candidates: Vec<IdentifyCandidateRow>,
    },
    PrincipalMatch,
    NameUnavailable,
    NameRequired,
    DestinationOccupied {
        entity_id: String,
    },
    EntityNotFound {
        entity_id: String,
    },
    NonPersonEntity {
        entity_id: String,
        entity_type: Option<String>,
    },
    InvalidEntityType {
        entity_type: String,
    },
    NonPersonCreateType {
        entity_type: String,
    },
}

/// Failure reading or resolving durable journal state during target selection.
#[derive(Debug, Error)]
pub enum IdentifyTargetError {
    #[error("entity lookup failed: {0}")]
    Entity(#[from] solstone_core_entity::EntityStoreError),
    #[error("principal lookup failed: {0}")]
    Eligibility(#[from] EligibilityError),
    #[error("entity resolution failed: {0}")]
    Resolution(#[from] EntityResolutionError),
    #[error("trust lock failed: {0}")]
    TrustLock(#[from] EntityTrustLockError),
}

/// Resolve an identify target without constructing or mutating an operation plan.
pub fn resolve_identify_target(
    request: &IdentifyTargetRequest,
) -> Result<IdentifyTargetOutcome, IdentifyTargetError> {
    let entity_id = request.entity_id.as_deref().unwrap_or_default().trim();
    if !entity_id.is_empty() {
        return resolve_entity_id_target(&request.journal_root, entity_id, request.resolve_only);
    }

    let name = request.name.as_deref().unwrap_or_default().trim();
    if name.is_empty() {
        return Ok(IdentifyTargetOutcome::NameRequired);
    }

    let entities = load_all_journal_entities(&request.journal_root)?;
    let principal_id = current_principal_id(&request.journal_root)?;
    if principal_name_collision(name, &entities, &principal_id) {
        return Ok(IdentifyTargetOutcome::PrincipalMatch);
    }
    if crate::eligibility::blocked_person_name_collision(name, &entities) {
        return Ok(IdentifyTargetOutcome::NameUnavailable);
    }

    let eligible = eligible_speaker_attach_entities(&entities, &principal_id);
    let resolution_entities = eligible
        .iter()
        .map(|entity| entity.resolution_entity())
        .collect::<Vec<_>>();
    let resolution = record_entity_resolution_from_name_evidence(
        &request.journal_root,
        name,
        &resolution_entities,
        json!({"kind": "journal"}),
        json!({
            "lane": "speaker_resolve.identify_cluster",
            "record_id": request.cluster_id.to_string(),
            "field": "name",
        }),
        RESOLUTION_FUZZY_THRESHOLD,
        true,
    )?;

    if resolution.outcome == EntityResolutionOutcome::Resolved {
        let entity = resolution
            .entity_index
            .and_then(|index| eligible.get(index))
            .expect("resolved entity index is supplied by the entity resolver");
        let target = target_from_entity(entity);
        return Ok(resolve_or_ready(
            target,
            request.resolve_only,
            &request.journal_root,
        ));
    }

    let candidate_source = if resolution.outcome == EntityResolutionOutcome::NoMatch {
        closest_resolution_candidates(name, &eligible)
    } else {
        resolution.candidates.clone()
    };
    let candidates = visible_near_match_candidate_rows(
        &candidate_source,
        &entities,
        &principal_id,
        &request.journal_root,
    );
    if request.resolve_only || !request.create_new {
        return Ok(match resolution.outcome {
            EntityResolutionOutcome::Ambiguous => IdentifyTargetOutcome::Ambiguous {
                ambiguity_id: resolution.ambiguity_id,
                candidates,
            },
            EntityResolutionOutcome::NoMatch => IdentifyTargetOutcome::NoMatch { candidates },
            EntityResolutionOutcome::Resolved => unreachable!("resolved result returned above"),
        });
    }

    if !is_valid_entity_type(&request.entity_type) {
        return Ok(IdentifyTargetOutcome::InvalidEntityType {
            entity_type: request.entity_type.clone(),
        });
    }
    // AC3 create-new admission: syntax validation alone accepts e.g. "Tool".
    if request.entity_type != "Person" {
        return Ok(IdentifyTargetOutcome::NonPersonCreateType {
            entity_type: request.entity_type.clone(),
        });
    }
    let proposed_id = entity_slug(name);
    let _trust = hold_entity_trust_lock(&request.journal_root)?;
    let occupied = read_identity_map(&request.journal_root)?
        .resolved
        .contains_key(&proposed_id)
        || entity_identity_destination_occupied(&request.journal_root, &proposed_id)?;
    if occupied {
        return Ok(IdentifyTargetOutcome::DestinationOccupied {
            entity_id: proposed_id,
        });
    }
    Ok(IdentifyTargetOutcome::Ready(TargetResolution {
        entity_id: proposed_id,
        entity_name: name.to_owned(),
        entity_type: request.entity_type.clone(),
        will_create: true,
        visible_candidate_ids: candidate_ids(&candidates),
    }))
}

fn resolve_entity_id_target(
    journal_root: &Path,
    entity_id: &str,
    resolve_only: bool,
) -> Result<IdentifyTargetOutcome, IdentifyTargetError> {
    let Some(snapshot) = read_entity_identity(journal_root, entity_id)? else {
        return Ok(IdentifyTargetOutcome::EntityNotFound {
            entity_id: entity_id.to_owned(),
        });
    };
    let entity = JournalEntity {
        id: snapshot.entity_id().to_owned(),
        value: snapshot.value().clone(),
    };
    // AC3 direct-id admission: Python previously lacked this Person guard.
    if !is_admissible_person(&entity) {
        return Ok(IdentifyTargetOutcome::NonPersonEntity {
            entity_id: entity.id.clone(),
            entity_type: entity.entity_type().map(str::to_owned),
        });
    }
    Ok(resolve_or_ready(
        target_from_entity(&entity),
        resolve_only,
        journal_root,
    ))
}

fn resolve_or_ready(
    target: TargetResolution,
    resolve_only: bool,
    journal_root: &Path,
) -> IdentifyTargetOutcome {
    if resolve_only {
        return IdentifyTargetOutcome::Resolved {
            has_voice: has_voice_at(journal_root, &target.entity_id),
            entity_id: target.entity_id,
            entity_name: target.entity_name,
        };
    }
    IdentifyTargetOutcome::Ready(target)
}

fn target_from_entity(entity: &JournalEntity) -> TargetResolution {
    TargetResolution {
        entity_id: entity.id.clone(),
        entity_name: entity
            .value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&entity.id)
            .to_owned(),
        entity_type: entity.entity_type().unwrap_or_default().to_owned(),
        will_create: false,
        visible_candidate_ids: Vec::new(),
    }
}

fn visible_near_match_candidate_rows(
    candidates: &[solstone_core_entity::ResolutionCandidate],
    entities: &[JournalEntity],
    principal_id: &str,
    journal_root: &Path,
) -> Vec<IdentifyCandidateRow> {
    let mut rows = candidates
        .iter()
        .filter(|candidate| {
            speaker_attach_rejection_reason(&candidate.id, entities, "", None, principal_id)
                .is_none()
        })
        .map(|candidate| IdentifyCandidateRow {
            id: candidate.id.clone(),
            name: candidate.name.clone(),
            tier: i64::from(candidate.tier as u8),
            score: candidate.score,
            has_voice: has_voice_at(journal_root, &candidate.id),
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
    rows
}

fn closest_resolution_candidates(
    query: &str,
    entities: &[&JournalEntity],
) -> Vec<solstone_core_entity::ResolutionCandidate> {
    let sorted_query = token_sort(query);
    let mut candidates = entities
        .iter()
        .filter_map(|entity| {
            let resolution_entity = entity.resolution_entity();
            if resolution_entity.name.is_empty() {
                return None;
            }
            let score = std::iter::once(resolution_entity.name.as_str())
                .chain(resolution_entity.aka.iter().map(String::as_str))
                .filter(|choice| !choice.is_empty())
                .map(|choice| {
                    rapidfuzz::fuzz::ratio(sorted_query.chars(), token_sort(choice).chars()) * 100.0
                })
                .max_by(f64::total_cmp)
                .unwrap_or(0.0);
            Some(solstone_core_entity::ResolutionCandidate {
                id: entity.id.clone(),
                name: resolution_entity.name,
                tier: MatchTier::Fuzzy,
                score,
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates.truncate(3);
    candidates
}

/// Return the visible candidate IDs in display order.
pub fn candidate_ids(candidate_rows: &[IdentifyCandidateRow]) -> Vec<String> {
    candidate_rows.iter().map(|row| row.id.clone()).collect()
}

fn has_voice_at(journal_root: &Path, entity_id: &str) -> bool {
    load_entity_voiceprints_file(journal_root, entity_id).is_some()
}
