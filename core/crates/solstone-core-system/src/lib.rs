// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed system-process and task-request primitives.

pub mod cap;
pub mod error;
pub mod lifecycle;
pub mod partition;
pub mod process;
pub mod provider_runtime;
pub mod queue;
pub mod request;
pub mod schedule;

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
