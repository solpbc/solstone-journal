// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Capability-safe, read-only inventory of portable journal archive sources.
//!
//! This crate deliberately owns no archive publication, command-line surface,
//! or generic filesystem traversal API. [`ArchiveSource`] retains a descriptor
//! for one acquired journal root and exposes only its frozen, verified archive
//! inventory plus a checked encoder for a caller-owned output file. It owns no
//! output-path selection, publication, command-line, HTTP, or generic
//! filesystem traversal API.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod encode;
mod entry;
mod error;
mod inventory;
mod manifest;
mod source;
mod target;
mod writer;

pub use encode::{
    EncodeArchiveError, EncodeArchiveFollowOn, EncodeArchiveRequest, EncodingPhase, encode_archive,
};
pub use entry::{
    ArchiveMemberName, Inventory, InventoryEntry, OpenedInventoryFile, SkippedRootName,
};
pub use error::{ArchiveError, JournalEntryKind};
pub use source::ArchiveSource;
pub use target::{
    ArchiveOutputTarget, ExplicitArchiveOutputRequest, ExplicitTargetError,
    acquire_explicit_output_target,
};
