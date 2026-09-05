// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::collections::HashMap;

use crate::ambiguity_id;
use solstone_core_entity_matching::{
    EntityNameCandidate, MatchTier, entity_slug, find_matching_entity, normalize_resolution_query,
};

use super::test_support::{
    SweepVerificationError, entity_identity_fixture, entity_matching_fixture,
    matching_divergences_fixture, normalization_divergences, normalization_divergences_fixture,
    slug_divergences, slug_divergences_fixture, verify_raw_sweep_digest, verify_sweep_digest,
};

#[test]
fn entity_slug_vectors_match_fixture() {
    let fixture = entity_identity_fixture();
    let divergences_fixture = slug_divergences_fixture();
    let divergences = slug_divergences(&divergences_fixture).expect("parse slug divergences");
    let divergence_map: HashMap<u32, _> = divergences
        .iter()
        .map(|divergence| (divergence.codepoint, divergence))
        .collect();

    assert_eq!(
        fixture.entity_slug.max_length,
        solstone_core_entity_matching::MAX_ENTITY_SLUG_LENGTH
    );
    // A loader that parses the file, misses the array and yields nothing would
    // satisfy every assertion below perfectly. The declared count is what makes
    // these tests rather than formalities.
    assert_eq!(
        fixture.entity_slug.vectors.len(),
        fixture.entity_slug.vector_count
    );
    for vector in fixture.entity_slug.vectors {
        let native = entity_slug(&vector.name);
        if let Some(codepoint) = scalar_probe_codepoint(&vector.name)
            && let Some(divergence) = divergence_map.get(&codepoint)
        {
            assert_eq!(native, divergence.native, "{:?}", vector.name);
            assert_eq!(divergence.reference, vector.slug, "{:?}", vector.name);
        } else {
            assert_eq!(native, vector.slug, "{:?}", vector.name);
        }
    }
}

#[test]
fn normalize_resolution_query_vectors_match_fixture() {
    let fixture = entity_identity_fixture();
    // A loader that parses the file, misses the array and yields nothing would
    // satisfy every assertion below perfectly. The declared count is what makes
    // these tests rather than formalities.
    assert_eq!(
        fixture.normalize_resolution_query.vectors.len(),
        fixture.normalize_resolution_query.vector_count
    );
    for vector in fixture.normalize_resolution_query.vectors {
        assert_eq!(
            normalize_resolution_query(&vector.query),
            vector.normalized,
            "{:?}",
            vector.query
        );
    }
}

#[test]
fn ambiguity_id_vectors_match_fixture() {
    let fixture = entity_identity_fixture();
    // A loader that parses the file, misses the array and yields nothing would
    // satisfy every assertion below perfectly. The declared count is what makes
    // these tests rather than formalities.
    assert_eq!(
        fixture.ambiguity_id.vectors.len(),
        fixture.ambiguity_id.vector_count
    );
    for vector in fixture.ambiguity_id.vectors {
        let normalized_query = normalize_resolution_query(&vector.query);
        assert_eq!(
            normalized_query, vector.normalized_query,
            "{:?}",
            vector.query
        );

        let scope_key = match vector.scope.kind.as_str() {
            "journal" => "journal".to_string(),
            "facet" => format!(
                "facet:{}",
                vector
                    .scope
                    .facet
                    .as_deref()
                    .expect("facet scopes include a facet")
            ),
            kind => panic!("unknown fixture scope kind: {kind:?}"),
        };
        let key = format!("{scope_key}|{normalized_query}");
        assert_eq!(key, vector.key);
        assert_eq!(ambiguity_id(&vector.key), vector.ambiguity_id);
    }
}

#[test]
fn matching_vectors_match_fixture() {
    let fixture = entity_matching_fixture();
    let divergences_fixture = matching_divergences_fixture();
    // A loader that parses the file, misses the array and yields nothing would
    // satisfy every assertion below perfectly. The declared count is what makes
    // these tests rather than formalities.
    assert_eq!(fixture.vectors.len(), fixture.vector_count);
    // The refusal half is where the store declines to guess; without its own
    // count a corpus that lost every refusal vector would still read as full.
    assert_eq!(
        fixture.vectors.iter().filter(|v| v.outcome.matched).count(),
        fixture.matched_count
    );
    assert_eq!(
        fixture
            .vectors
            .iter()
            .filter(|v| !v.outcome.matched)
            .count(),
        fixture.refusal_count
    );
    assert_eq!(
        divergences_fixture.entries.len(),
        divergences_fixture.counts.total
    );
    assert_eq!(
        divergences_fixture.counts.tier_changes + divergences_fixture.counts.refusal_to_match,
        divergences_fixture.counts.total
    );
    let mut divergences = HashMap::with_capacity(divergences_fixture.entries.len());
    for divergence in divergences_fixture.entries {
        assert!(
            divergence.fixture_index < fixture.vectors.len(),
            "divergence fixture index is out of range: {}",
            divergence.fixture_index
        );
        assert_ne!(
            divergence.reference_outcome, divergence.native_outcome,
            "divergence fixture entry is a no-op: {}",
            divergence.fixture_index
        );
        assert!(
            divergences
                .insert(divergence.fixture_index, divergence)
                .is_none(),
            "duplicate divergence fixture index"
        );
    }
    for (index, vector) in fixture.vectors.into_iter().enumerate() {
        let expected = if let Some(divergence) = divergences.remove(&index) {
            assert_eq!(
                divergence.query, vector.query,
                "divergence query does not match matching fixture at index {index}"
            );
            let candidate_ids = vector
                .candidates
                .iter()
                .map(|candidate| {
                    candidate
                        .id
                        .as_deref()
                        .expect("divergence candidates include ids")
                })
                .collect::<Vec<_>>();
            assert_eq!(
                divergence.candidate_ids, candidate_ids,
                "divergence candidates do not match matching fixture at index {index}"
            );
            assert_eq!(
                divergence.reference_outcome, vector.outcome,
                "divergence reference does not match matching fixture at index {index}"
            );
            divergence.native_outcome
        } else {
            vector.outcome.clone()
        };
        let candidates: Vec<EntityNameCandidate> = vector
            .candidates
            .into_iter()
            .map(|candidate| EntityNameCandidate {
                id: candidate.id,
                name: candidate.name,
                aka: candidate.aka,
                emails: candidate.emails,
            })
            .collect();
        let result = find_matching_entity(&vector.query, &candidates, fixture.fuzzy_threshold);

        if !expected.matched {
            assert_eq!(result, None, "{:?}", vector.query);
            continue;
        }

        let result = result.expect("matched fixture vector resolves a candidate");
        assert_eq!(
            result.candidate_index,
            expected
                .candidate_index
                .expect("matched fixture vector has candidate index"),
            "{:?}",
            vector.query
        );
        assert_eq!(
            result.tier as u8,
            expected.tier.expect("matched fixture vector has tier"),
            "{:?}",
            vector.query
        );
        assert_eq!(
            result.tier.is_high_confidence(),
            expected
                .high_confidence
                .expect("matched fixture vector has confidence"),
            "{:?}",
            vector.query
        );
        assert_eq!(
            result.tier <= MatchTier::Slug,
            result.tier.is_high_confidence(),
            "{:?}",
            vector.query
        );
        assert!(result.tier as u8 <= 8);
        assert_eq!(
            result.tier.is_high_confidence(),
            result.tier as u8 <= fixture.high_confidence_max_tier
        );
    }
    assert!(
        divergences.is_empty(),
        "divergence fixture index was not visited"
    );
}

#[test]
fn slug_sweep_matches_fixture_with_reference_substitutions() {
    let identity = entity_identity_fixture();
    let divergences_fixture = slug_divergences_fixture();
    let divergences = slug_divergences(&divergences_fixture).expect("parse slug divergences");

    verify_sweep_digest(
        entity_slug,
        &divergences,
        divergences_fixture.counts.total,
        identity.entity_slug.sweep.scalar_values,
        &identity.entity_slug.sweep.sha256,
    )
    .expect("slug sweep matches corrected reference digest");
}

#[test]
fn normalization_sweep_matches_fixture_with_reference_substitutions() {
    let identity = entity_identity_fixture();
    let divergences_fixture = normalization_divergences_fixture();
    let divergences =
        normalization_divergences(&divergences_fixture).expect("parse normalization divergences");

    verify_sweep_digest(
        normalize_resolution_query,
        &divergences,
        divergences_fixture.counts.total,
        identity.normalize_resolution_query.sweep.scalar_values,
        &identity.normalize_resolution_query.sweep.sha256,
    )
    .expect("normalization sweep matches corrected reference digest");
}

#[test]
fn raw_slug_sweep_without_substitution_fails_digest() {
    let identity = entity_identity_fixture();
    let result = verify_raw_sweep_digest(
        entity_slug,
        identity.entity_slug.sweep.scalar_values,
        &identity.entity_slug.sweep.sha256,
    );
    assert!(matches!(
        result,
        Err(SweepVerificationError::DigestMismatch { .. })
    ));
}

#[test]
fn missing_slug_divergence_entry_fails_count_check() {
    let identity = entity_identity_fixture();
    let divergences_fixture = slug_divergences_fixture();
    let mut divergences = slug_divergences(&divergences_fixture).expect("parse slug divergences");
    divergences.pop().expect("fixture contains divergences");

    let result = verify_sweep_digest(
        entity_slug,
        &divergences,
        divergences_fixture.counts.total,
        identity.entity_slug.sweep.scalar_values,
        &identity.entity_slug.sweep.sha256,
    );
    assert!(matches!(
        result,
        Err(SweepVerificationError::DivergenceCountMismatch { .. })
    ));
}

#[test]
fn altered_slug_native_value_fails_native_assertion() {
    let identity = entity_identity_fixture();
    let divergences_fixture = slug_divergences_fixture();
    let mut divergences = slug_divergences(&divergences_fixture).expect("parse slug divergences");
    let first = divergences
        .iter_mut()
        .min_by_key(|divergence| divergence.codepoint)
        .expect("fixture contains divergences");
    first.native.push('!');

    let result = verify_sweep_digest(
        entity_slug,
        &divergences,
        divergences_fixture.counts.total,
        identity.entity_slug.sweep.scalar_values,
        &identity.entity_slug.sweep.sha256,
    );
    assert!(matches!(
        result,
        Err(SweepVerificationError::NativeValueMismatch { .. })
    ));
}

fn scalar_probe_codepoint(value: &str) -> Option<u32> {
    let mut chars = value.chars();
    if chars.next()? != 'A' {
        return None;
    }
    let scalar = chars.next()?;
    if chars.next()? != 'B' || chars.next().is_some() {
        return None;
    }
    Some(scalar as u32)
}
