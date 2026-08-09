// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Eligibility predicates for attaching speaker evidence to journal entities.

use std::collections::HashSet;
use std::path::Path;

use solstone_core_entity::{JournalEntity, read_journal_principal};
use solstone_core_entity_matching::normalize_resolution_query;
use thiserror::Error;

/// Failure while resolving the journal principal used by speaker eligibility.
#[derive(Debug, Error)]
pub enum EligibilityError {
    #[error("principal lookup failed: {0}")]
    Principal(#[from] solstone_core_entity::EntityLifecycleError),
}

/// Why an entity may not be authorized as a reviewed speaker near-match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerAttachRejectionReason {
    SelfTarget,
    Nonexistent,
    Principal,
    NonPerson,
    Blocked,
    Unshown,
}

impl SpeakerAttachRejectionReason {
    /// Python-compatible machine-readable reason string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SelfTarget => "self",
            Self::Nonexistent => "nonexistent",
            Self::Principal => "principal",
            Self::NonPerson => "non_person",
            Self::Blocked => "blocked",
            Self::Unshown => "unshown",
        }
    }
}

/// Return the journal principal id, or an empty string when no principal exists.
pub fn current_principal_id(journal_root: &Path) -> Result<String, EligibilityError> {
    Ok(read_journal_principal(journal_root)?
        .and_then(|principal| {
            principal
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default())
}

/// Return whether one journal entity can receive speaker-cluster evidence.
pub fn is_speaker_attach_candidate(entity: &JournalEntity, principal_id: &str) -> bool {
    !entity.id.is_empty()
        && !entity
            .value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .is_empty()
        && entity.entity_type() == Some("Person")
        && !entity.is_blocked()
        && !entity.is_principal()
        && entity.id != principal_id
}

/// Return the entities eligible for speaker-cluster attachment.
pub fn eligible_speaker_attach_entities<'a>(
    entities: &'a [JournalEntity],
    principal_id: &str,
) -> Vec<&'a JournalEntity> {
    entities
        .iter()
        .filter(|entity| is_speaker_attach_candidate(entity, principal_id))
        .collect()
}

/// Return why an entity cannot be accepted as a reviewed near-match, if any.
pub fn speaker_attach_rejection_reason(
    entity_id: &str,
    entities: &[JournalEntity],
    target_id: &str,
    visible_candidate_ids: Option<&HashSet<String>>,
    principal_id: &str,
) -> Option<SpeakerAttachRejectionReason> {
    if !target_id.is_empty() && entity_id == target_id {
        return Some(SpeakerAttachRejectionReason::SelfTarget);
    }
    let Some(entity) = entities.iter().find(|entity| entity.id == entity_id) else {
        return Some(SpeakerAttachRejectionReason::Nonexistent);
    };
    if entity_id.is_empty()
        || entity.id.is_empty()
        || entity
            .value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .is_empty()
    {
        return Some(SpeakerAttachRejectionReason::Nonexistent);
    }
    if entity.is_principal() || entity_id == principal_id {
        return Some(SpeakerAttachRejectionReason::Principal);
    }
    if entity.entity_type() != Some("Person") {
        return Some(SpeakerAttachRejectionReason::NonPerson);
    }
    if entity.is_blocked() {
        return Some(SpeakerAttachRejectionReason::Blocked);
    }
    if visible_candidate_ids.is_some_and(|ids| !ids.contains(entity_id)) {
        return Some(SpeakerAttachRejectionReason::Unshown);
    }
    None
}

/// Return whether a query collides with the principal's name or aliases.
pub fn principal_name_collision(
    name: &str,
    entities: &[JournalEntity],
    principal_id: &str,
) -> bool {
    !principal_id.is_empty()
        && entities
            .iter()
            .find(|entity| entity.id == principal_id)
            .is_some_and(|entity| name_or_aka_collision(name, entity))
}

/// Return whether a query collides with a blocked Person's name or aliases.
pub fn blocked_person_name_collision(name: &str, entities: &[JournalEntity]) -> bool {
    entities.iter().any(|entity| {
        entity.entity_type() == Some("Person")
            && entity.is_blocked()
            && name_or_aka_collision(name, entity)
    })
}

fn name_or_aka_collision(name: &str, entity: &JournalEntity) -> bool {
    let query = normalize_resolution_query(name);
    !query.is_empty()
        && name_values(entity)
            .into_iter()
            .any(|value| normalize_resolution_query(&value) == query)
}

fn name_values(entity: &JournalEntity) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(name) = entity.value.get("name").and_then(serde_json::Value::as_str)
        && !name.trim().is_empty()
    {
        values.push(name.to_owned());
    }
    if let Some(aliases) = entity
        .value
        .get("aka")
        .and_then(serde_json::Value::as_array)
    {
        values.extend(
            aliases
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|alias| !alias.trim().is_empty())
                .map(str::to_owned),
        );
    }
    values
}
