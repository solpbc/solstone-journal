// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner of the observer registry and sync-history files.

mod command;
mod service;
pub mod store;

#[cfg(test)]
pub(crate) mod test_support;

pub use command::{ObserverCommand, parse_observer_args};
pub use service::{
    CREATE_RETIRED_MESSAGE, ObserverError, PruneOutcome, execute, execute_prune, system_now_ms,
};
pub use store::delivery::{
    AssessedObserverFact, DeliveryAssessment, DeliveryInspection, OBSERVER_ACTIVE_MS,
    OBSERVER_DELIVERY_LONG_STOP_MS, OBSERVER_DELIVERY_STALL_MS, OBSERVER_STALE_MS,
    ObserverDeliveryFacts, OwnerState, Reach, RegistryState, UnassessedObserver, UnassessedReason,
    inspect_loaded, rollup_owner_states,
};
pub use store::prune::{
    HistoryPruneFailure, HistoryPruneReport, has_history_for_stream, observer_prefix_for_stream,
    remove_history_rows_for_stream,
};
pub use store::{
    RemoteObserverMigrationError, RemoteObserverMigrationReport, SyncEventKind, SyncPersistResult,
    migrate_remote_observer_storage, persist_sync,
};
