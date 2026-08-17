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
    DeliveryDivergence, OBSERVER_DELIVERY_STALL_MS, OBSERVER_STALE_MS, delivery_divergence,
};
pub use store::{
    RemoteObserverMigrationError, RemoteObserverMigrationReport, migrate_remote_observer_storage,
};
