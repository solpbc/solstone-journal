// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure entity identity and matching primitives.

mod ambiguity;
mod matcher;
mod normalize;
mod slug;

pub use ambiguity::ambiguity_id;
pub use matcher::{EntityNameCandidate, EntityNameMatch, MatchTier, find_matching_entity};
pub use normalize::normalize_resolution_query;
pub use slug::{MAX_ENTITY_SLUG_LENGTH, entity_slug};

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod test_support;
