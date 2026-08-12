// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure, byte-preserving classification for the Python steward log pruner.

mod classify;
mod coerce;
mod rows;
mod syntax;
mod unicode;

pub use classify::{Disposition, PruneClassification, WholeNoopReason, classify_prune};
pub use rows::{Row, RowSplitter, Terminator};

/// Python's steward parser is bounded to one MiB of row content, excluding its
/// line terminator.
pub const MAX_ROW_CONTENT_BYTES: usize = 1_048_576;
