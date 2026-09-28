// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed system-process and task-request primitives.

pub mod activity_state;
pub mod cap;
pub mod catchup;
pub mod daily_coverage;
pub mod direct_door;
pub mod error;
pub mod lifecycle;
pub mod memory_admission;
pub mod operational_log_parse;
pub mod owner_path;
pub mod partition;
pub mod process;
#[cfg(any(unix, windows))]
pub mod provider_runtime;
pub mod queue;
pub mod queue_hold;
pub mod queue_hold_store;
pub use queue_hold_store::{
    HoldPlatform, TaskQueueHoldFinding, classify_task_queue_holds, current_boot_identity,
};
pub mod request;
pub mod schedule;
#[cfg(any(unix, windows))]
pub mod status_wire;
pub mod stt_backend_choice;

#[cfg(any(unix, windows))]
pub use solstone_core_journal_config::no_thinking_engine_chosen;

/// Task-service tokens shared with the native journal process census.
pub const TASK_VERB_TOKENS: [&str; 7] = [
    "think",
    "indexer",
    "importer",
    "brain",
    "maintenance",
    "heartbeat",
    "facet-candidates",
];
