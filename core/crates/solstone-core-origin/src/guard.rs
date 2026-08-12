// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Refuse-by-default origin retention decisions.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::pins::{
    PinsError, head_origin_pins, historical_origin_pins, supported_release_versions,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOwner {
    Release(String),
    HeadUnreleased,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneAssessment {
    Prunable,
    PinnedBy { owners: Vec<PinOwner> },
    Unknown,
}

#[derive(Debug, Error)]
pub enum GuardError {
    #[error(transparent)]
    Pins(#[from] PinsError),
    #[error("refusing to prune {origin_key}: {assessment:?}")]
    Refused {
        origin_key: String,
        assessment: PruneAssessment,
    },
}

/// Production composition: every transparency-log version is supported. The
/// split lets tests exercise `Prunable` without inventing an EOL policy.
pub fn assess_prune_with_current_support(origin_key: &str) -> Result<PruneAssessment, GuardError> {
    assess_prune(origin_key, &supported_release_versions()?)
}

/// Assess a caller-supplied release set. Production reaches this only through
/// `assess_prune_with_current_support`; it is exposed for policy tests.
pub fn assess_prune(
    origin_key: &str,
    supported_releases: &BTreeSet<String>,
) -> Result<PruneAssessment, GuardError> {
    let historical = historical_origin_pins()?;
    let mut universe = BTreeMap::<String, Vec<PinOwner>>::new();
    for (release, pins) in historical {
        for pin in pins {
            let owners = universe.entry(pin.origin_key).or_default();
            if supported_releases.contains(&release) {
                owners.push(PinOwner::Release(release.clone()));
            }
        }
    }
    for pin in head_origin_pins()? {
        universe
            .entry(pin.origin_key)
            .or_default()
            .push(PinOwner::HeadUnreleased);
    }
    match universe.get(origin_key) {
        None => Ok(PruneAssessment::Unknown),
        Some(owners) if owners.is_empty() => Ok(PruneAssessment::Prunable),
        Some(owners) => Ok(PruneAssessment::PinnedBy {
            owners: owners.clone(),
        }),
    }
}

pub fn require_prunable(origin_key: &str) -> Result<(), GuardError> {
    let assessment = assess_prune_with_current_support(origin_key)?;
    if assessment == PruneAssessment::Prunable {
        Ok(())
    } else {
        Err(GuardError::Refused {
            origin_key: origin_key.to_owned(),
            assessment,
        })
    }
}
