// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;

use serde::Deserialize;
use sha2::{Digest, Sha256};

const ENTITY_IDENTITY_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_identity.json"
));
const ENTITY_MATCHING_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_matching.json"
));
const ENTITY_SLUG_DIVERGENCES_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_slug_native_divergences.json"
));
const ENTITY_NORMALIZATION_DIVERGENCES_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_normalization_native_divergences.json"
));

#[derive(Debug, Deserialize)]
pub(crate) struct EntityIdentityFixture {
    pub(crate) entity_slug: SlugIdentityFixture,
    pub(crate) normalize_resolution_query: NormalizationIdentityFixture,
    pub(crate) ambiguity_id: AmbiguityIdentityFixture,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SlugIdentityFixture {
    pub(crate) max_length: usize,
    pub(crate) vector_count: usize,
    pub(crate) sweep: SweepFixture,
    pub(crate) vectors: Vec<SlugVector>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NormalizationIdentityFixture {
    pub(crate) vector_count: usize,
    pub(crate) sweep: SweepFixture,
    pub(crate) vectors: Vec<NormalizationVector>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SweepFixture {
    pub(crate) scalar_values: usize,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SlugVector {
    pub(crate) name: String,
    pub(crate) slug: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NormalizationVector {
    pub(crate) query: String,
    pub(crate) normalized: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AmbiguityIdentityFixture {
    pub(crate) vector_count: usize,
    pub(crate) vectors: Vec<AmbiguityVector>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AmbiguityVector {
    pub(crate) ambiguity_id: String,
    pub(crate) key: String,
    pub(crate) normalized_query: String,
    pub(crate) query: String,
    pub(crate) scope: ScopeFixture,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ScopeFixture {
    pub(crate) kind: String,
    pub(crate) facet: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EntityMatchingFixture {
    pub(crate) fuzzy_threshold: f64,
    pub(crate) vector_count: usize,
    pub(crate) matched_count: usize,
    pub(crate) refusal_count: usize,
    pub(crate) high_confidence_max_tier: u8,
    pub(crate) vectors: Vec<MatchingVector>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MatchingVector {
    pub(crate) candidates: Vec<MatchingCandidate>,
    pub(crate) outcome: MatchingOutcome,
    pub(crate) query: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MatchingCandidate {
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) aka: Vec<String>,
    #[serde(default)]
    pub(crate) emails: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MatchingOutcome {
    pub(crate) matched: bool,
    pub(crate) candidate_index: Option<usize>,
    pub(crate) tier: Option<u8>,
    pub(crate) high_confidence: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SlugDivergencesFixture {
    pub(crate) counts: DivergenceCounts,
    pub(crate) entries: Vec<SlugDivergenceRecord>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NormalizationDivergencesFixture {
    pub(crate) counts: DivergenceCounts,
    pub(crate) entries: Vec<NormalizationDivergenceRecord>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DivergenceCounts {
    pub(crate) total: usize,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SlugDivergenceRecord {
    pub(crate) codepoint: String,
    pub(crate) native_slug: String,
    pub(crate) reference_slug: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NormalizationDivergenceRecord {
    pub(crate) codepoint: String,
    pub(crate) native_normalized: String,
    pub(crate) reference_normalized: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SweepDivergence {
    pub(crate) codepoint: u32,
    pub(crate) native: String,
    pub(crate) reference: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SweepVerificationError {
    DivergenceCountMismatch { expected: usize, actual: usize },
    DuplicateOrInvalidCodepoint { codepoint: String },
    NativeValueMismatch { codepoint: u32 },
    ListedCodepointDoesNotDiverge { codepoint: u32 },
    ScalarCountMismatch { expected: usize, actual: usize },
    DigestMismatch { expected: String, actual: String },
}

pub(crate) fn entity_identity_fixture() -> EntityIdentityFixture {
    serde_json::from_str(ENTITY_IDENTITY_FIXTURE).expect("parse entity identity fixture")
}

pub(crate) fn entity_matching_fixture() -> EntityMatchingFixture {
    serde_json::from_str(ENTITY_MATCHING_FIXTURE).expect("parse entity matching fixture")
}

pub(crate) fn slug_divergences_fixture() -> SlugDivergencesFixture {
    serde_json::from_str(ENTITY_SLUG_DIVERGENCES_FIXTURE)
        .expect("parse entity slug divergences fixture")
}

pub(crate) fn normalization_divergences_fixture() -> NormalizationDivergencesFixture {
    serde_json::from_str(ENTITY_NORMALIZATION_DIVERGENCES_FIXTURE)
        .expect("parse entity normalization divergences fixture")
}

pub(crate) fn slug_divergences(
    fixture: &SlugDivergencesFixture,
) -> Result<Vec<SweepDivergence>, SweepVerificationError> {
    fixture
        .entries
        .iter()
        .map(|entry| {
            Ok(SweepDivergence {
                codepoint: parse_codepoint(&entry.codepoint)?,
                native: entry.native_slug.clone(),
                reference: entry.reference_slug.clone(),
            })
        })
        .collect()
}

pub(crate) fn normalization_divergences(
    fixture: &NormalizationDivergencesFixture,
) -> Result<Vec<SweepDivergence>, SweepVerificationError> {
    fixture
        .entries
        .iter()
        .map(|entry| {
            Ok(SweepDivergence {
                codepoint: parse_codepoint(&entry.codepoint)?,
                native: entry.native_normalized.clone(),
                reference: entry.reference_normalized.clone(),
            })
        })
        .collect()
}

pub(crate) fn verify_sweep_digest<F>(
    producer: F,
    divergences: &[SweepDivergence],
    expected_divergence_count: usize,
    expected_scalar_count: usize,
    expected_digest: &str,
) -> Result<(), SweepVerificationError>
where
    F: Fn(&str) -> String,
{
    if divergences.len() != expected_divergence_count {
        return Err(SweepVerificationError::DivergenceCountMismatch {
            expected: expected_divergence_count,
            actual: divergences.len(),
        });
    }

    let mut divergence_map = HashMap::with_capacity(divergences.len());
    for divergence in divergences {
        if char::from_u32(divergence.codepoint).is_none()
            || divergence_map
                .insert(divergence.codepoint, divergence)
                .is_some()
        {
            return Err(SweepVerificationError::DuplicateOrInvalidCodepoint {
                codepoint: format!("U+{:04X}", divergence.codepoint),
            });
        }
    }

    let mut digest = Sha256::new();
    let mut scalar_count = 0;
    let mut visited_divergences = 0;
    for codepoint in 0u32..=0x10FFFF {
        let Some(scalar) = char::from_u32(codepoint) else {
            continue;
        };
        scalar_count += 1;
        let input = format!("A{scalar}B");
        let native = producer(&input);
        let line = if let Some(divergence) = divergence_map.get(&codepoint) {
            visited_divergences += 1;
            if native != divergence.native {
                return Err(SweepVerificationError::NativeValueMismatch { codepoint });
            }
            if divergence.native == divergence.reference {
                return Err(SweepVerificationError::ListedCodepointDoesNotDiverge { codepoint });
            }
            divergence.reference.as_str()
        } else {
            native.as_str()
        };
        digest.update(line.as_bytes());
        digest.update(b"\n");
    }

    if visited_divergences != expected_divergence_count {
        return Err(SweepVerificationError::DivergenceCountMismatch {
            expected: expected_divergence_count,
            actual: visited_divergences,
        });
    }
    if scalar_count != expected_scalar_count {
        return Err(SweepVerificationError::ScalarCountMismatch {
            expected: expected_scalar_count,
            actual: scalar_count,
        });
    }

    let actual = format!("{:x}", digest.finalize());
    if actual != expected_digest {
        return Err(SweepVerificationError::DigestMismatch {
            expected: expected_digest.to_string(),
            actual,
        });
    }
    Ok(())
}

pub(crate) fn verify_raw_sweep_digest<F>(
    producer: F,
    expected_scalar_count: usize,
    expected_digest: &str,
) -> Result<(), SweepVerificationError>
where
    F: Fn(&str) -> String,
{
    let mut digest = Sha256::new();
    let mut scalar_count = 0;
    for codepoint in 0u32..=0x10FFFF {
        let Some(scalar) = char::from_u32(codepoint) else {
            continue;
        };
        scalar_count += 1;
        let input = format!("A{scalar}B");
        let line = producer(&input);
        digest.update(line.as_bytes());
        digest.update(b"\n");
    }

    if scalar_count != expected_scalar_count {
        return Err(SweepVerificationError::ScalarCountMismatch {
            expected: expected_scalar_count,
            actual: scalar_count,
        });
    }

    let actual = format!("{:x}", digest.finalize());
    if actual != expected_digest {
        return Err(SweepVerificationError::DigestMismatch {
            expected: expected_digest.to_string(),
            actual,
        });
    }
    Ok(())
}

fn parse_codepoint(value: &str) -> Result<u32, SweepVerificationError> {
    let Some(hex) = value.strip_prefix("U+") else {
        return Err(SweepVerificationError::DuplicateOrInvalidCodepoint {
            codepoint: value.to_string(),
        });
    };
    let Ok(codepoint) = u32::from_str_radix(hex, 16) else {
        return Err(SweepVerificationError::DuplicateOrInvalidCodepoint {
            codepoint: value.to_string(),
        });
    };
    if char::from_u32(codepoint).is_none() {
        return Err(SweepVerificationError::DuplicateOrInvalidCodepoint {
            codepoint: value.to_string(),
        });
    }
    Ok(codepoint)
}
