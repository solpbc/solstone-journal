// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Capability-safe portable archive inventory, encoding and create-only publication.
//!
//! [`ArchiveSource`] retains a [`solstone_core_journal_io::JournalRoot`] for
//! verified inventory and member reads. [`ArchiveOutputTarget`] retains the
//! selected existing output parent. Callers select an explicit path or supply
//! an owned admitted parent and one free leaf; publication stays descriptor-relative.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

#[cfg(not(any(unix, windows)))]
compile_error!(
    "solstone-core-journal-archive requires a Unix or Windows target: archive source traversal has no portable backend"
);

mod deny;
mod eligibility;
mod encode;
mod entry;
#[cfg(any(unix, windows))]
mod error;
#[cfg(unix)]
mod inventory;
mod live;
mod manifest;
#[cfg(unix)]
mod publish;
#[cfg(unix)]
mod source;
#[cfg(unix)]
mod target;
#[cfg(all(unix, feature = "test-hooks"))]
mod test_hooks;
#[cfg(windows)]
mod windows_publish;
#[cfg(windows)]
mod windows_source;
#[cfg(windows)]
mod windows_target;
mod writer;

pub use eligibility::{
    EligibilityError, EstimateProgress, ExportBreakdown, ExportMode, LighterExportPlan,
    OffloadCatalogCounts, PopulationBucket, evaluate_lighter_export, summarize_offload_catalog,
};
pub use encode::{
    DayWindow, EncodeArchiveError, EncodeArchiveFollowOn, EncodeArchiveRequest, EncodeOptions,
    EncodeProgress, EncodingPhase, encode_archive, encode_archive_with_options,
};
pub use entry::{
    ArchiveMemberName, IncludedRootName, Inventory, InventoryEntry, OpenedInventoryFile,
    SkippedRootName,
};
#[cfg(any(unix, windows))]
pub use error::ArchiveError;
pub use live::SnapshotMode;
#[cfg(unix)]
pub use publish::{ArchivePublicationError, publish_archive, publish_archive_with_options};
#[cfg(any(unix, windows))]
pub use solstone_core_journal_io::JournalEntryKind;
#[cfg(unix)]
pub use source::ArchiveSource;
#[cfg(unix)]
pub use target::{
    ArchiveOutputTarget, ExplicitArchiveOutputRequest, ExplicitTargetError,
    acquire_bound_output_target, acquire_explicit_output_target,
};
#[cfg(all(unix, feature = "test-hooks"))]
pub use test_hooks::{
    AcquisitionPrimitive, DescendantPrimitive, EncodeTruncateBeforeRead, TestBoundary,
    TestFaultKind, TestSinkOperation, run_with_acquisition_fault, run_with_descendant_barrier,
    run_with_encode_control,
};
#[cfg(windows)]
pub use windows_publish::{ArchivePublicationError, publish_archive, publish_archive_with_options};
#[cfg(windows)]
pub use windows_source::ArchiveSource;
#[cfg(windows)]
pub use windows_target::{
    ArchiveOutputTarget, ExplicitArchiveOutputRequest, ExplicitTargetError,
    acquire_explicit_output_target,
};
