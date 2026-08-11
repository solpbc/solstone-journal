// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Resolution boundary that records entity-name ambiguities.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

use serde_json::{Value, json};

use solstone_core_entity_matching::{
    EntityNameCandidate, EntityNameMatchOutcome, MatchTier, char_len,
    find_matching_entity_detailed, first_word_match, matchable_resolution_query,
    normalize_resolution_query, prefix_token_match, single_token_first_word_match, token_sort,
    token_subset_match,
};

use crate::{
    AmbiguityObservation, EntityStoreError, EntityTrustLockError, EntityWriteError,
    hold_entity_trust_lock, load_resolved_ambiguity_choice, record_ambiguity_observation,
};

/// One caller-supplied entity available for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityResolutionEntity {
    pub id: Option<String>,
    pub name: String,
    pub aka: Vec<String>,
    pub emails: Vec<String>,
    pub blocked: bool,
}

/// One ranked candidate retained in an ambiguous resolution result.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolutionCandidate {
    pub id: String,
    pub name: String,
    pub tier: MatchTier,
    pub score: f64,
}

impl ResolutionCandidate {
    fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "tier": i64::from(self.tier as u8),
            "score": self.score,
        })
    }
}

/// Result category for an entity-resolution attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityResolutionOutcome {
    Resolved,
    Ambiguous,
    NoMatch,
}

/// Result of resolving one query against the caller-supplied entity slice.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityResolution {
    pub outcome: EntityResolutionOutcome,
    pub entity_index: Option<usize>,
    pub tier: Option<MatchTier>,
    pub candidates: Vec<ResolutionCandidate>,
    pub ambiguity_id: Option<String>,
}

/// Failure while resolving an entity name or recording an ambiguous result.
#[derive(Debug)]
pub enum EntityResolutionError {
    TrustLock(EntityTrustLockError),
    Read(EntityStoreError),
    Write(EntityWriteError),
    /// A recorded choice names an ID absent from the entities slice supplied to
    /// this call; it does not mean that the entity is absent from the journal.
    ResolvedChoiceEntityAbsent {
        ambiguity_id: String,
        entity_id: String,
    },
    ResolvedChoiceEntityBlocked {
        ambiguity_id: String,
        entity_id: String,
    },
}

impl fmt::Display for EntityResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::ResolvedChoiceEntityAbsent {
                ambiguity_id,
                entity_id,
            } => write!(
                formatter,
                "resolved ambiguity {ambiguity_id} names entity {entity_id:?} absent from this resolution call"
            ),
            Self::ResolvedChoiceEntityBlocked {
                ambiguity_id,
                entity_id,
            } => write!(
                formatter,
                "resolved ambiguity {ambiguity_id} names blocked entity {entity_id:?}"
            ),
        }
    }
}

impl Error for EntityResolutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::ResolvedChoiceEntityAbsent { .. } | Self::ResolvedChoiceEntityBlocked { .. } => {
                None
            }
        }
    }
}

impl From<EntityTrustLockError> for EntityResolutionError {
    fn from(error: EntityTrustLockError) -> Self {
        Self::TrustLock(error)
    }
}

impl From<EntityStoreError> for EntityResolutionError {
    fn from(error: EntityStoreError) -> Self {
        Self::Read(error)
    }
}

impl From<EntityWriteError> for EntityResolutionError {
    fn from(error: EntityWriteError) -> Self {
        Self::Write(error)
    }
}

/// Rank retained candidates by similarity, then raw name and ID.
pub(crate) fn rank_resolution_candidates(
    query: &str,
    tier: MatchTier,
    entities: &[&EntityResolutionEntity],
) -> Vec<ResolutionCandidate> {
    let mut by_id = BTreeMap::new();
    for entity in entities {
        let Some(id) = entity.id.as_deref().filter(|id| !id.is_empty()) else {
            continue;
        };
        if entity.name.is_empty() {
            continue;
        }
        by_id.entry(id).or_insert(*entity);
    }

    let mut candidates: Vec<_> = by_id
        .into_iter()
        .map(|(id, entity)| ResolutionCandidate {
            id: id.to_owned(),
            name: entity.name.clone(),
            tier,
            score: candidate_similarity_score(query, entity),
        })
        .collect();
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates
}

/// Collect all low-confidence candidates without the matcher's uniqueness guards.
pub(crate) fn collect_low_confidence_candidates(
    match_query: &str,
    entities: &[EntityResolutionEntity],
    fuzzy_threshold: f64,
) -> (Option<MatchTier>, Vec<ResolutionCandidate>) {
    if match_query.is_empty() || entities.is_empty() {
        return (None, Vec::new());
    }

    let normalized_query = normalize_resolution_query(match_query);
    if char_len(match_query) >= 3 {
        let first_word_matches: Vec<_> = entities
            .iter()
            .filter(|entity| first_word_match(&normalized_query, &entity.name))
            .collect();
        if !first_word_matches.is_empty() {
            return (
                Some(MatchTier::FirstWord),
                rank_resolution_candidates(match_query, MatchTier::FirstWord, &first_word_matches),
            );
        }

        let query_first = match_query
            .split_whitespace()
            .next()
            .map(normalize_resolution_query);
        if let Some(query_first) = query_first
            && query_first != normalized_query
            && char_len(&query_first) >= 3
        {
            let long_to_short_matches: Vec<_> = entities
                .iter()
                .filter(|entity| single_token_first_word_match(&query_first, &entity.name))
                .collect();
            if !long_to_short_matches.is_empty() {
                return (
                    Some(MatchTier::FirstWord),
                    rank_resolution_candidates(
                        match_query,
                        MatchTier::FirstWord,
                        &long_to_short_matches,
                    ),
                );
            }
        }
    }

    let subset_matches: Vec<_> = entities
        .iter()
        .filter(|entity| {
            !entity.name.is_empty()
                && token_subset_match(&normalized_query, &normalize_resolution_query(&entity.name))
        })
        .collect();
    if !subset_matches.is_empty() {
        return (
            Some(MatchTier::TokenSubset),
            rank_resolution_candidates(match_query, MatchTier::TokenSubset, &subset_matches),
        );
    }

    let prefix_matches: Vec<_> = entities
        .iter()
        .filter(|entity| {
            !entity.name.is_empty()
                && prefix_token_match(&normalized_query, &normalize_resolution_query(&entity.name))
        })
        .collect();
    if !prefix_matches.is_empty() {
        return (
            Some(MatchTier::Prefix),
            rank_resolution_candidates(match_query, MatchTier::Prefix, &prefix_matches),
        );
    }

    if char_len(match_query) >= 4 {
        let fuzzy_matches: Vec<_> = entities
            .iter()
            .filter(|entity| candidate_similarity_score(match_query, entity) >= fuzzy_threshold)
            .collect();
        if !fuzzy_matches.is_empty() {
            return (
                Some(MatchTier::Fuzzy),
                rank_resolution_candidates(match_query, MatchTier::Fuzzy, &fuzzy_matches),
            );
        }
    }

    (None, Vec::new())
}

/// Resolve a query, recording an ambiguity in mutation mode.
pub fn record_entity_resolution(
    journal_root: &Path,
    query: &str,
    entities: &[EntityResolutionEntity],
    scope: Value,
    origin: Value,
    fuzzy_threshold: f64,
    read_only: bool,
) -> Result<EntityResolution, EntityResolutionError> {
    record_entity_resolution_impl(
        journal_root,
        query,
        entities,
        scope,
        origin,
        fuzzy_threshold,
        read_only,
        false,
    )
}

/// Resolve a query using names, aliases, emails, and name-derived slugs only.
///
/// Written entity IDs are never treated as match evidence in this mode. This
/// prevents an unrelated entity ID from resolving merely because it equals the
/// query's derived slug.
pub fn record_entity_resolution_from_name_evidence(
    journal_root: &Path,
    query: &str,
    entities: &[EntityResolutionEntity],
    scope: Value,
    origin: Value,
    fuzzy_threshold: f64,
    read_only: bool,
) -> Result<EntityResolution, EntityResolutionError> {
    record_entity_resolution_impl(
        journal_root,
        query,
        entities,
        scope,
        origin,
        fuzzy_threshold,
        read_only,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn record_entity_resolution_impl(
    journal_root: &Path,
    query: &str,
    entities: &[EntityResolutionEntity],
    scope: Value,
    origin: Value,
    fuzzy_threshold: f64,
    read_only: bool,
    name_evidence_only: bool,
) -> Result<EntityResolution, EntityResolutionError> {
    if query.trim().is_empty() {
        return Ok(no_match());
    }

    let _trust = (!read_only)
        .then(|| hold_entity_trust_lock(journal_root))
        .transpose()?;
    let normalized_query = normalize_resolution_query(query);
    let match_query = matchable_resolution_query(query);

    if let Some(row) = load_resolved_ambiguity_choice(journal_root, &scope, &normalized_query)? {
        let ambiguity_id = row
            .get("ambiguity_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let entity_id = row
            .get("resolved_entity_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let Some(entity_index) = entities
            .iter()
            .position(|entity| entity.id.as_deref() == Some(entity_id.as_str()))
        else {
            return Err(EntityResolutionError::ResolvedChoiceEntityAbsent {
                ambiguity_id,
                entity_id,
            });
        };
        if entities[entity_index].blocked {
            return Err(EntityResolutionError::ResolvedChoiceEntityBlocked {
                ambiguity_id,
                entity_id,
            });
        }
        return Ok(EntityResolution {
            outcome: EntityResolutionOutcome::Resolved,
            entity_index: Some(entity_index),
            tier: None,
            candidates: Vec::new(),
            ambiguity_id: None,
        });
    }

    if entities.is_empty() {
        return Ok(no_match());
    }

    let candidates: Vec<_> = entities
        .iter()
        .map(|entity| EntityNameCandidate {
            id: if name_evidence_only {
                None
            } else {
                entity.id.clone()
            },
            name: entity.name.clone(),
            aka: entity.aka.clone(),
            emails: entity.emails.clone(),
        })
        .collect();
    match find_matching_entity_detailed(&match_query, &candidates, fuzzy_threshold) {
        EntityNameMatchOutcome::Matched {
            candidate_index,
            tier,
        } if tier.is_high_confidence() => {
            return Ok(EntityResolution {
                outcome: EntityResolutionOutcome::Resolved,
                entity_index: Some(candidate_index),
                tier: Some(tier),
                candidates: Vec::new(),
                ambiguity_id: None,
            });
        }
        EntityNameMatchOutcome::Ambiguous {
            tier,
            candidate_indices,
        } if tier.is_high_confidence() => {
            let colliding_entities = candidate_indices
                .iter()
                .map(|index| &entities[*index])
                .collect::<Vec<_>>();
            let candidates = equalize_ambiguous_candidate_scores(rank_resolution_candidates(
                &match_query,
                tier,
                &colliding_entities,
            ));
            return record_resolution_ambiguity(
                AmbiguityRecordContext {
                    journal_root,
                    query,
                    normalized_query,
                    scope,
                    origin,
                    read_only,
                },
                tier,
                candidates,
            );
        }
        EntityNameMatchOutcome::Matched { .. }
        | EntityNameMatchOutcome::Ambiguous { .. }
        | EntityNameMatchOutcome::NoMatch => {}
    }

    let (tier, candidates) =
        collect_low_confidence_candidates(&match_query, entities, fuzzy_threshold);
    if let Some(tier) = tier
        && !candidates.is_empty()
    {
        return record_resolution_ambiguity(
            AmbiguityRecordContext {
                journal_root,
                query,
                normalized_query,
                scope,
                origin,
                read_only,
            },
            tier,
            candidates,
        );
    }

    Ok(no_match())
}

struct AmbiguityRecordContext<'a> {
    journal_root: &'a Path,
    query: &'a str,
    normalized_query: String,
    scope: Value,
    origin: Value,
    read_only: bool,
}

fn record_resolution_ambiguity(
    context: AmbiguityRecordContext<'_>,
    tier: MatchTier,
    candidates: Vec<ResolutionCandidate>,
) -> Result<EntityResolution, EntityResolutionError> {
    let ambiguity_id = if context.read_only {
        String::new()
    } else {
        let observation = AmbiguityObservation {
            scope: context.scope,
            query: context.query.to_owned(),
            normalized_query: context.normalized_query,
            observed_tier: i64::from(tier as u8),
            ranked_candidates: candidates
                .iter()
                .map(ResolutionCandidate::to_value)
                .collect(),
            origin: context.origin,
        };
        record_ambiguity_observation(context.journal_root, &observation)?
            .get("ambiguity_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    Ok(EntityResolution {
        outcome: EntityResolutionOutcome::Ambiguous,
        entity_index: None,
        tier: Some(tier),
        candidates,
        ambiguity_id: Some(ambiguity_id),
    })
}

fn equalize_ambiguous_candidate_scores(
    mut candidates: Vec<ResolutionCandidate>,
) -> Vec<ResolutionCandidate> {
    // The matcher refused to rank these candidates, so differing similarity scores
    // would re-assert a ranking it deliberately declined to make.
    let Some(max_score) = candidates
        .iter()
        .map(|candidate| candidate.score)
        .max_by(f64::total_cmp)
    else {
        return candidates;
    };
    for candidate in &mut candidates {
        candidate.score = max_score;
    }
    candidates.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates
}

fn candidate_similarity_score(query: &str, entity: &EntityResolutionEntity) -> f64 {
    let sorted_query = token_sort(query);
    std::iter::once(entity.name.as_str())
        .chain(entity.aka.iter().map(String::as_str))
        .filter(|choice| !choice.is_empty())
        .map(|choice| {
            rapidfuzz::fuzz::ratio(sorted_query.chars(), token_sort(choice).chars()) * 100.0
        })
        .max_by(f64::total_cmp)
        .unwrap_or(0.0)
}

fn no_match() -> EntityResolution {
    EntityResolution {
        outcome: EntityResolutionOutcome::NoMatch,
        entity_index: None,
        tier: None,
        candidates: Vec::new(),
        ambiguity_id: None,
    }
}
