// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Capability-safe, read-only inventory of portable journal archive sources.
//!
//! This crate deliberately owns no archive publication, command-line surface,
//! or generic filesystem traversal API. [`ArchiveSource`] retains a
//! [`solstone_core_journal_io::JournalRoot`] and exposes only its frozen, verified
//! archive inventory plus a checked encoder for a caller-owned output file. It
//! does not implement root acquisition; [`ArchiveSource::open`] delegates to
//! [`solstone_core_journal_io::JournalRoot::open`]. It owns no output-path
//! selection, publication, command-line, HTTP, or generic filesystem traversal API.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod deny;
mod encode;
mod entry;
mod error;
mod inventory;
mod manifest;
mod publish;
mod source;
mod target;
#[cfg(feature = "test-hooks")]
mod test_hooks;
mod writer;

pub use encode::{
    DayWindow, EncodeArchiveError, EncodeArchiveFollowOn, EncodeArchiveRequest, EncodingPhase,
    encode_archive,
};
pub use entry::{
    ArchiveMemberName, IncludedRootName, Inventory, InventoryEntry, OpenedInventoryFile,
    SkippedRootName,
};
pub use error::ArchiveError;
pub use publish::{ArchivePublicationError, publish_archive};
pub use solstone_core_journal_io::JournalEntryKind;
pub use source::ArchiveSource;
pub use target::{
    ArchiveOutputTarget, ExplicitArchiveOutputRequest, ExplicitTargetError,
    acquire_explicit_output_target,
};
#[cfg(feature = "test-hooks")]
pub use test_hooks::{
    AcquisitionPrimitive, DescendantPrimitive, EncodeTruncateBeforeRead, TestBoundary,
    TestFaultKind, TestSinkOperation, run_with_acquisition_fault, run_with_descendant_barrier,
    run_with_encode_control,
};
