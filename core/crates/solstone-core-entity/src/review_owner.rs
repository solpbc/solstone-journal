// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed review-owner conflict kinds emitted at entity and facet owner boundaries.

use std::fmt;

/// Typed conflict at a review owner boundary. Callers must not classify these
/// by error prose or a `conflict:` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReviewOwnerConflictKind {
    AliasClaimed,
    AliasTargetDetached,
    PromotionOwnerState,
    IdentityMoved,
    IdentityChanged,
    IdentityDisappeared,
    IdentityBlocked,
    PromotedEntityBlocked,
    IdentityMapGroupLost,
    NameMatchesMultiple,
    NoUsableEntityId,
    IdentityUnreadable,
    RelationshipOccupied,
    RelationshipChanged,
    MergeProposalPreparation,
    MergeProposalsChanged,
    OutputArtifactChanged,
    OwningFacetChanged,
    ArtifactBeforeChanged,
    /// The id the plan would create was merged away after it was prepared.
    IdentityMerged,
}

impl ReviewOwnerConflictKind {
    pub const ALL: &[Self] = &[
        Self::AliasClaimed,
        Self::AliasTargetDetached,
        Self::PromotionOwnerState,
        Self::IdentityMoved,
        Self::IdentityChanged,
        Self::IdentityDisappeared,
        Self::IdentityBlocked,
        Self::PromotedEntityBlocked,
        Self::IdentityMapGroupLost,
        Self::NameMatchesMultiple,
        Self::NoUsableEntityId,
        Self::IdentityUnreadable,
        Self::RelationshipOccupied,
        Self::RelationshipChanged,
        Self::MergeProposalPreparation,
        Self::MergeProposalsChanged,
        Self::OutputArtifactChanged,
        Self::OwningFacetChanged,
        Self::ArtifactBeforeChanged,
        Self::IdentityMerged,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AliasClaimed => "alias_claimed",
            Self::AliasTargetDetached => "alias_target_detached",
            Self::PromotionOwnerState => "promotion_owner_state",
            Self::IdentityMoved => "identity_moved",
            Self::IdentityChanged => "identity_changed",
            Self::IdentityDisappeared => "identity_disappeared",
            Self::IdentityBlocked => "identity_blocked",
            Self::PromotedEntityBlocked => "promoted_entity_blocked",
            Self::IdentityMapGroupLost => "identity_map_group_lost",
            Self::NameMatchesMultiple => "name_matches_multiple",
            Self::NoUsableEntityId => "no_usable_entity_id",
            Self::IdentityUnreadable => "identity_unreadable",
            Self::RelationshipOccupied => "relationship_occupied",
            Self::RelationshipChanged => "relationship_changed",
            Self::MergeProposalPreparation => "merge_proposal_preparation",
            Self::MergeProposalsChanged => "merge_proposals_changed",
            Self::OutputArtifactChanged => "output_artifact_changed",
            Self::OwningFacetChanged => "owning_facet_changed",
            Self::ArtifactBeforeChanged => "artifact_before_changed",
            Self::IdentityMerged => "identity_merged",
        }
    }
}

/// Failure from a review owner boundary: a typed conflict, or an untyped I/O/malformed failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewOwnerError {
    Conflict {
        kind: ReviewOwnerConflictKind,
        detail: String,
    },
    Failed {
        detail: String,
    },
}

impl ReviewOwnerError {
    pub fn conflict(kind: ReviewOwnerConflictKind, detail: impl Into<String>) -> Self {
        Self::Conflict {
            kind,
            detail: detail.into(),
        }
    }

    pub fn failed(detail: impl Into<String>) -> Self {
        Self::Failed {
            detail: detail.into(),
        }
    }

    pub fn kind(&self) -> Option<ReviewOwnerConflictKind> {
        match self {
            Self::Conflict { kind, .. } => Some(*kind),
            Self::Failed { .. } => None,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Conflict { detail, .. } | Self::Failed { detail } => detail,
        }
    }
}

impl fmt::Display for ReviewOwnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.detail())
    }
}

impl std::error::Error for ReviewOwnerError {}

impl From<String> for ReviewOwnerError {
    fn from(detail: String) -> Self {
        Self::failed(detail)
    }
}

impl From<&str> for ReviewOwnerError {
    fn from(detail: &str) -> Self {
        Self::failed(detail)
    }
}

impl PartialEq<str> for ReviewOwnerError {
    fn eq(&self, other: &str) -> bool {
        self.detail() == other
    }
}

impl PartialEq<&str> for ReviewOwnerError {
    fn eq(&self, other: &&str) -> bool {
        self.detail() == *other
    }
}

#[cfg(test)]
mod tests {
    use super::ReviewOwnerConflictKind;

    #[test]
    fn closed_kind_strings_are_unique_and_exhaustive() {
        let mut seen = std::collections::BTreeSet::new();
        for kind in ReviewOwnerConflictKind::ALL {
            assert!(seen.insert(kind.as_str()), "{}", kind.as_str());
        }
        assert_eq!(seen.len(), ReviewOwnerConflictKind::ALL.len());
        let _ = ReviewOwnerConflictKind::AliasClaimed.as_str();
    }
}
