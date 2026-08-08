// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Capability-safe, read-only inventory of portable journal archive sources.
//!
//! This crate deliberately owns no archive encoding, publication, command-line
//! surface, or generic filesystem traversal API. [`ArchiveSource`] retains a
//! descriptor for one acquired journal root and exposes only its frozen,
//! verified archive inventory.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod entry;
mod error;
mod inventory;
mod source;

pub use entry::{
    ArchiveMemberName, Inventory, InventoryEntry, OpenedInventoryFile, SkippedRootName,
};
pub use error::{ArchiveError, JournalEntryKind};
pub use source::ArchiveSource;
