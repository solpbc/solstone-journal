// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only access to durable journal entity state.

mod ambiguity;
mod error;
mod history;
mod identity;
mod map;
mod paths;
mod reconcile;

pub use ambiguity::{load_resolved_ambiguity_choice, read_ambiguities};
pub use error::EntityStoreError;
pub use history::{
    HistoryEvent, PreparedHistoryEvent, guard_restore_does_not_cross_merge,
    guard_visible_event_collision, read_prepared_history, read_visible_history,
};
pub use identity::{IdentitySnapshot, read_entity_identity};
pub use map::{EntityIdentityMap, IdentityMapLoser, IdentityMapLoserReason, read_identity_map};
pub use reconcile::{PreparedHistoryOutcome, classify_prepared_history};
