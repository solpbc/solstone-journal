// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity identity, matching primitives, and read-only durable-store access.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod ambiguity;
mod matcher;
mod normalize;
mod slug;
mod store;

pub use ambiguity::ambiguity_id;
pub use matcher::{EntityNameCandidate, EntityNameMatch, MatchTier, find_matching_entity};
pub use normalize::normalize_resolution_query;
pub use slug::{MAX_ENTITY_SLUG_LENGTH, entity_slug};
pub use store::{
    EntityIdentityMap, EntityStoreError, HistoryEvent, IdentityMapLoser, IdentityMapLoserReason,
    IdentitySnapshot, PreparedHistoryEvent, PreparedHistoryOutcome, classify_prepared_history,
    guard_restore_does_not_cross_merge, guard_visible_event_collision,
    load_resolved_ambiguity_choice, read_ambiguities, read_entity_identity, read_identity_map,
    read_prepared_history, read_visible_history,
};

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod test_support;
