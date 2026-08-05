// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable read and write access to journal entity state.

mod ambiguity;
mod error;
mod history;
mod identity;
mod map;
mod paths;
mod reconcile;
mod write;

pub use ambiguity::{load_resolved_ambiguity_choice, read_ambiguities};
pub use error::EntityStoreError;
pub use history::{
    HistoryEvent, PreparedHistoryEvent, guard_restore_does_not_cross_merge,
    guard_visible_event_collision, read_prepared_history, read_visible_history,
};
pub use identity::{IdentitySnapshot, read_entity_identity};
pub use map::{EntityIdentityMap, IdentityMapLoser, IdentityMapLoserReason, read_identity_map};
pub use reconcile::{PreparedHistoryOutcome, classify_prepared_history};
pub use write::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation, EntityOperationContext,
    EntityOperationKind, EntitySaveResult, EntityWriteError, IdentityMapCacheLoad,
    record_ambiguity_choice, record_ambiguity_observation, refresh_identity_map_cache,
    save_entity_identity,
};

#[cfg(test)]
pub(crate) use write::{
    save_entity_identity_with_timeout, set_forced_identity_write_failure,
    write_history_event_json_for_test,
};
