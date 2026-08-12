// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Refuse-by-default origin retention decisions.

use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Debug)]
pub enum GuardError {
    Pins(PinsError),
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
pub(crate) fn assess_prune(
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

impl std::fmt::Display for GuardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pins(error) => error.fmt(formatter),
            Self::Refused {
                origin_key,
                assessment: PruneAssessment::PinnedBy { owners },
            } => write!(
                formatter,
                "refusing to prune {origin_key}: pinned by {}",
                owners
                    .iter()
                    .map(PinOwner::display_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Refused { origin_key, .. } => {
                write!(
                    formatter,
                    "refusing to prune {origin_key}: unknown origin key"
                )
            }
        }
    }
}

impl std::error::Error for GuardError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pins(error) => Some(error),
            Self::Refused { .. } => None,
        }
    }
}

impl From<PinsError> for GuardError {
    fn from(error: PinsError) -> Self {
        Self::Pins(error)
    }
}

impl PinOwner {
    fn display_name(&self) -> &str {
        match self {
            Self::Release(version) => version,
            Self::HeadUnreleased => "HEAD (unreleased)",
        }
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
