// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity identity, matching primitives, durable-store access, and mutation-support plumbing.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod ambiguity;
mod matcher;
mod normalize;
mod slug;
mod store;
mod trust_lock;

pub use ambiguity::ambiguity_id;
pub use matcher::{EntityNameCandidate, EntityNameMatch, MatchTier, find_matching_entity};
pub use normalize::normalize_resolution_query;
pub use slug::{MAX_ENTITY_SLUG_LENGTH, entity_slug};
pub use store::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation, EntityIdentityMap,
    EntityIdentityRepairError, EntityIdentityRepairGuard, EntityIdentityRepairRefusal,
    EntityIdentityRepairReport, EntityIdentityRepairSkip, EntityIdentityRepairSkipReason,
    EntityOperationContext, EntityOperationKind, EntitySaveResult, EntityStoreError,
    EntityWriteError, HistoryEvent, IdentityMapCacheLoad, IdentityMapLoser, IdentityMapLoserReason,
    IdentitySnapshot, PreparedHistoryEvent, PreparedHistoryOutcome, classify_prepared_history,
    guard_restore_does_not_cross_merge, guard_visible_event_collision,
    load_resolved_ambiguity_choice, read_ambiguities, read_entity_identity, read_identity_map,
    read_prepared_history, read_visible_history, record_ambiguity_choice,
    record_ambiguity_observation, refresh_identity_map_cache, repair_entity_identities,
    save_entity_identity,
};
pub use trust_lock::{EntityTrustLock, EntityTrustLockError, hold_entity_trust_lock};

#[cfg(test)]
pub(crate) use store::{
    save_entity_identity_with_timeout, set_forced_identity_write_failure,
    set_repair_identity_write_failure_on_attempt, write_history_event_json_for_test,
};

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod trust_lock_tests;
