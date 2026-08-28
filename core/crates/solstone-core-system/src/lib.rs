// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed system-process and task-request primitives.

pub mod activity_state;
pub mod cap;
pub mod catchup;
pub mod direct_door;
pub mod error;
pub mod lifecycle;
pub mod memory_admission;
pub mod operational_log_parse;
pub mod partition;
pub mod process;
#[cfg(unix)]
pub mod provider_runtime;
pub mod queue;
pub mod request;
pub mod schedule;
#[cfg(unix)]
pub mod status_wire;
pub mod stt_backend_choice;

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
