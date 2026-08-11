// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native importer source registry and source-specific seams.

use std::fmt;

pub mod apple_health;
pub mod archive;
pub mod chatgpt;
pub mod claude;
pub mod document;
pub mod gemini;
pub mod ics;
pub mod image;
pub mod kindle;
pub mod obsidian;
pub mod oura;
pub mod registry;

/// Error returned by a source seam that has no implementation yet.
#[derive(Debug, Eq, PartialEq)]
pub enum ImportSourcesError {
    Unimplemented { module: &'static str },
}

impl fmt::Display for ImportSourcesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unimplemented { module } => {
                write!(formatter, "import-sources: unimplemented: {module}")
            }
        }
    }
}

impl std::error::Error for ImportSourcesError {}

/// One source module name and its reserved skeleton seam.
pub type ModuleStub = (&'static str, fn() -> Result<(), ImportSourcesError>);

/// The complete skeleton importer source-module inventory.
pub const MODULE_STUBS: &[ModuleStub] = &[
    ("registry", registry::reserved_seam),
    ("ics", ics::reserved_seam),
    ("obsidian", obsidian::reserved_seam),
    ("chatgpt", chatgpt::reserved_seam),
    ("claude", claude::reserved_seam),
    ("gemini", gemini::reserved_seam),
    ("kindle", kindle::reserved_seam),
    ("document", document::reserved_seam),
    ("image", image::reserved_seam),
    ("archive", archive::reserved_seam),
    ("apple_health", apple_health::reserved_seam),
    ("oura", oura::reserved_seam),
];
