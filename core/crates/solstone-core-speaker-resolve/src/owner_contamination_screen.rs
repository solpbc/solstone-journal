// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only screening of a probe embedding against the journal owner tier.

use std::error::Error;
use std::fmt;
use std::path::Path;

use crate::segment_path;
use serde::{Deserialize, Serialize};
use solstone_core_entity::{EncoderIdentity, normalize_embedding};
use solstone_core_journal_io::PathError;
use solstone_core_speaker_id::calibration::OWNER_THRESHOLD;
use solstone_core_speaker_id::embeddings::load_embeddings_file;

use crate::owner_provisional::{
    OwnerProvisionalError, OwnerTierOutcome, OwnerTierReason, resolve_owner_tier,
};

/// Coordinates identifying one embedding sidecar row to screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContaminationProbe {
    pub day: String,
    pub stream: String,
    pub segment_key: String,
    pub source: String,
    pub sentence_id: i64,
}

/// The read-only owner-contamination decision for one probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContaminationScreen {
    Contaminated {
        basis: String,
        similarity: f32,
        threshold: f32,
    },
    Clear {
        basis: String,
        similarity: f32,
        threshold: f32,
    },
    Indeterminate {
        reason: String,
    },
}

/// Failure while screening a probe that is malformed rather than absent.
#[derive(Debug)]
pub enum OwnerContaminationScreenError {
    Owner(OwnerProvisionalError),
    Path(PathError),
    InvalidEncoderIdentity,
    InvalidEmbeddingWidth,
    NonFiniteEmbedding,
}

impl fmt::Display for OwnerContaminationScreenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owner(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::InvalidEncoderIdentity => formatter.write_str("invalid encoder identity"),
            Self::InvalidEmbeddingWidth => {
                formatter.write_str("probe embedding width does not match encoder")
            }
            Self::NonFiniteEmbedding => {
                formatter.write_str("probe embedding contains non-finite values")
            }
        }
    }
}

impl Error for OwnerContaminationScreenError {}

impl OwnerTierReason {
    /// Stable wire representation for an indeterminate owner tier.
    pub fn wire_str(&self) -> &'static str {
        match self {
            Self::ConfirmedAbsent => "confirmed_absent",
            Self::ConfirmedUnreadable => "confirmed_unreadable",
            Self::ConfirmedIncomplete => "confirmed_incomplete",
            Self::ConfirmedZeroNorm => "confirmed_zero_norm",
            Self::VoiceprintsAbsent => "voiceprints_absent",
            Self::VoiceprintsUnreadable => "voiceprints_unreadable",
            Self::BelowRowFloor => "below_row_floor",
            Self::BelowEmbeddingFloor => "below_embedding_floor",
            Self::ProvisionalZeroNorm => "provisional_zero_norm",
        }
    }
}

/// Classify a resolved tier into scoring material or a terminal response.
pub fn classify_tier(
    outcome: OwnerTierOutcome,
) -> Result<(String, Vec<f32>, f32), ContaminationScreen> {
    match outcome {
        OwnerTierOutcome::Confirmed(centroid) => Ok((
            "confirmed".to_owned(),
            centroid.centroid,
            centroid.threshold,
        )),
        OwnerTierOutcome::Provisional(centroid) => {
            Ok(("provisional".to_owned(), centroid, OWNER_THRESHOLD))
        }
        OwnerTierOutcome::IdentityInvalid => Err(ContaminationScreen::Indeterminate {
            reason: "speaker_owner_identity_invalid".to_owned(),
        }),
        OwnerTierOutcome::None(reason) => Err(ContaminationScreen::Indeterminate {
            reason: reason.wire_str().to_owned(),
        }),
    }
}

/// Decide whether a similarity meets the resolved owner threshold.
pub fn decide(basis: String, similarity: f32, threshold: f32) -> ContaminationScreen {
    if similarity >= threshold {
        ContaminationScreen::Contaminated {
            basis,
            similarity,
            threshold,
        }
    } else {
        ContaminationScreen::Clear {
            basis,
            similarity,
            threshold,
        }
    }
}

/// Screen one persisted probe embedding against the current resolved owner tier.
pub fn screen_owner_contamination(
    journal_root: &Path,
    probe: &ContaminationProbe,
    encoder: &EncoderIdentity,
) -> Result<ContaminationScreen, OwnerContaminationScreenError> {
    validate_encoder(encoder)?;
    let tier = resolve_owner_tier(journal_root).map_err(OwnerContaminationScreenError::Owner)?;
    let (basis, centroid, threshold) = match classify_tier(tier) {
        Ok(tier) => tier,
        Err(indeterminate) => return Ok(indeterminate),
    };
    let segment = segment_path(
        journal_root,
        &probe.day,
        &probe.segment_key,
        &probe.stream,
        false,
    )
    .map_err(OwnerContaminationScreenError::Path)?;
    let path = segment.join(format!("{}.npz", probe.source));
    let Some(raw) = load_embeddings_file(&path)
        .ok()
        .flatten()
        .and_then(|embeddings| {
            embeddings
                .statements
                .into_iter()
                .find(|(sentence_id, _)| *sentence_id == probe.sentence_id)
                .map(|(_, embedding)| embedding)
        })
    else {
        return Ok(ContaminationScreen::Indeterminate {
            reason: "probe_not_found".to_owned(),
        });
    };
    if raw.len() != encoder.width {
        return Err(OwnerContaminationScreenError::InvalidEmbeddingWidth);
    }
    if raw.iter().any(|value| !value.is_finite()) {
        return Err(OwnerContaminationScreenError::NonFiniteEmbedding);
    }
    let Some(normalized) = normalize_embedding(&raw) else {
        return Ok(ContaminationScreen::Indeterminate {
            reason: "probe_zero_norm".to_owned(),
        });
    };
    let similarity = dot(&normalized, &centroid);
    Ok(decide(basis, similarity, threshold))
}

fn validate_encoder(encoder: &EncoderIdentity) -> Result<(), OwnerContaminationScreenError> {
    if encoder.id.is_empty()
        || encoder.sha256.len() != 64
        || !encoder.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || encoder.width == 0
    {
        return Err(OwnerContaminationScreenError::InvalidEncoderIdentity);
    }
    Ok(())
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}
