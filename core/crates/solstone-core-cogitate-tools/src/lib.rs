// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded raw filesystem reads for cogitate evidence.
//!
//! This crate binds no tools to any talent or tier. Per-tier tool binding,
//! including the guarantee that no write tool is ever registered, is outside
//! this crate's scope and belongs to a later wave.

mod budget;
mod denylist;
mod glob;
mod grep_search;
mod list_directory;
mod paths;
mod patterns;
mod read_file;
mod refusals;
mod types;
mod walk;

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
pub use types::{
    Entry, GlobOptions, GrepMatch, GrepSearchOptions, ListDirectoryOptions, ReadFileOptions,
    ReadPayload, ReadResult, ToolName,
};

#[cfg(test)]
mod bed;
#[cfg(test)]
mod conformance;
#[cfg(test)]
mod oracle;
