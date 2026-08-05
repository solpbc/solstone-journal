// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;

use crate::{
    EntityNameCandidate, MatchTier, ambiguity_id, entity_slug, find_matching_entity,
    normalize_resolution_query,
};

use super::test_support::{
    SweepVerificationError, entity_identity_fixture, entity_matching_fixture,
    normalization_divergences, normalization_divergences_fixture, slug_divergences,
    slug_divergences_fixture, verify_raw_sweep_digest, verify_sweep_digest,
};

#[test]
fn non_test_sources_do_not_use_host_io_or_embedded_assets() {
    for (path, source) in [
        ("lib.rs", include_str!("lib.rs")),
        ("slug.rs", include_str!("slug.rs")),
        ("matcher.rs", include_str!("matcher.rs")),
        ("normalize.rs", include_str!("normalize.rs")),
        ("ambiguity.rs", include_str!("ambiguity.rs")),
    ] {
        for forbidden in ["std::fs", "std::path", "include_str!"] {
            assert!(
                !source.contains(forbidden),
                "{path} must not reference {forbidden}"
            );
        }
    }
}

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
        crate::MAX_ENTITY_SLUG_LENGTH
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
    for vector in fixture.vectors {
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

        if !vector.outcome.matched {
            assert_eq!(result, None, "{:?}", vector.query);
            continue;
        }

        let result = result.expect("matched fixture vector resolves a candidate");
        assert_eq!(
            result.candidate_index,
            vector
                .outcome
                .candidate_index
                .expect("matched fixture vector has candidate index"),
            "{:?}",
            vector.query
        );
        assert_eq!(
            result.tier as u8,
            vector
                .outcome
                .tier
                .expect("matched fixture vector has tier"),
            "{:?}",
            vector.query
        );
        assert_eq!(
            result.tier.is_high_confidence(),
            vector
                .outcome
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
