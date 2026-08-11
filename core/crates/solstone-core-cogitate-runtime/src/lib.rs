// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded, synchronous cogitate tool-conversation runtime.
//!
//! ## Conversation persistence
//!
//! This crate deliberately builds no conversation-persistence format. The
//! reason is not that nothing would read one: the retention crate will
//! eventually read a persistence directory in order to delete it under
//! retention policy. The retention read is real deferred R6 work, not evidence
//! that persistence has no consumers.
//! This wave creates no such directory, so retention has nothing to dangle
//! over yet. When this crate or R5b begins persisting conversations, retention
//! must be updated in lockstep.

pub mod config;
pub mod divergence;
pub mod events;
pub mod outcome;
pub mod provider;
pub mod runtime;
pub mod tools;
pub mod usage;

mod ladders;
mod stuck;

pub use config::{RunConfig, RunInput};
pub use events::{EventSink, NoopEventSink, RecordingEventSink, RuntimeEvent};
pub use outcome::{RunOutcome, SOL_SLOT_REACQUIRE_FAILED, TOOL_BINDING_SETUP_FAILED};
pub use provider::{ConverseProvider, ProviderResponse};
pub use runtime::run_cogitate;
pub use tools::{CogitateToolExecutor, ToolExecution, ToolExecutor};
pub use usage::Usage;

#[cfg(test)]
mod tests;
