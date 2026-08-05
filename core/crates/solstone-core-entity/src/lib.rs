// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity identity, matching primitives, durable-store access, and mutation-support plumbing.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod ambiguity;
mod resolution;
mod store;
mod trust_lock;

pub use ambiguity::ambiguity_id;
pub use resolution::{
    EntityResolution, EntityResolutionEntity, EntityResolutionError, EntityResolutionOutcome,
    ResolutionCandidate, record_entity_resolution,
};
pub use store::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation,
    EntityAmbiguityRescopeError, EntityAmbiguityRescopeReport, EntityIdentityGroupMap,
    EntityIdentityMap, EntityIdentityRepairError, EntityIdentityRepairGuard,
    EntityIdentityRepairRefusal, EntityIdentityRepairReport, EntityIdentityRepairSkip,
    EntityIdentityRepairSkipReason, EntityMergeError, EntityMergeOptions, EntityMergePreview,
    EntityMergeReport, EntityOperationContext, EntityOperationKind, EntitySaveResult,
    EntityStoreError, EntityUndoError, EntityUndoReport, EntityWriteError, HistoryEvent,
    IdentityMapCacheLoad, IdentityMapLoser, IdentityMapLoserReason, IdentitySnapshot,
    PreparedHistoryEvent, PreparedHistoryOutcome, classify_prepared_history, commit_entity_merge,
    guard_restore_does_not_cross_merge, guard_visible_event_collision,
    load_resolved_ambiguity_choice, preview_entity_merge, read_ambiguities, read_entity_identity,
    read_identity_group_map, read_identity_map, read_prepared_history, read_visible_history,
    record_ambiguity_choice, record_ambiguity_observation, refresh_identity_map_cache,
    repair_entity_identities, rescope_facet_ambiguities, save_entity_identity, undo_entity_merge,
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
mod merge_payload_tests;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod resolution_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod trust_lock_tests;
#[cfg(test)]
mod undo_tests;
