// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded raw filesystem reads for cogitate evidence.
//!
//! This crate binds the bounded cogitate tool surface to access tiers.

mod binding;
mod budget;
mod denylist;
mod glob;
mod grep_search;
mod list_directory;
mod paths;
mod patterns;
mod read_file;
mod refusals;
mod slot_lease;
mod sol_budget;
mod sol_execution;
mod tool_metadata;
mod types;
mod walk;

pub use binding::{KNOWN_TOOL_NAMES, bound_tools};
pub use budget::ReadBudget;
pub use denylist::{
    DENIED_CREDENTIAL_PATTERNS, DENIED_PATH_COMPONENTS, GLOB_MAX_MATCHES, GREP_MAX_BYTES_PER_FILE,
    GREP_MAX_FILES, GREP_MAX_MATCHES, LIST_DIRECTORY_MAX_ENTRIES, READ_FILE_MAX_BYTES,
    READ_FILE_MAX_LINES,
};
pub use glob::glob;
pub use grep_search::grep_search;
pub use list_directory::list_directory;
pub use read_file::read_file;
pub use refusals::*;
pub use slot_lease::{NoopSlotLease, SlotLease, SlotReacquireError};
pub use sol_budget::{BudgetExhaustedEvent, SolCallBudget};
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use sol_execution::test_hooks as sol_execution_test_hooks;
pub use sol_execution::{
    SHELL_STDERR_CAP, SHELL_STDOUT_CAP, SHELL_TIMEOUT_SECONDS, SolObservation, SolToolResult,
    format_shell_output, run_command, run_sol_command, truncate_output,
};
pub use tool_metadata::{
    EMIT_FINAL_TOOL, FINISH_TOOL, GLOB_TOOL, GREP_SEARCH_TOOL, LIST_DIRECTORY_TOOL, READ_FILE_TOOL,
    ToolArgumentSpec, ToolSpec, resolve_tool_spec, sol_tool,
};
pub use types::{
    Entry, GlobOptions, GrepMatch, GrepSearchOptions, ListDirectoryOptions, ReadFileOptions,
    ReadPayload, ReadResult, ToolName,
};

#[cfg(all(test, unix))]
mod bed;
#[cfg(all(test, unix))]
mod conformance;
#[cfg(all(test, unix))]
mod oracle;
#[cfg(all(test, unix))]
mod runtime_conformance;
