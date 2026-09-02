// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exclusive operational-log create and canonical `oplog--` filename grammar.
//!
//! ```compile_fail,E0432
//! use solstone_core_journal_io::operational_log::OplogCreatePrimitive;
//! #[cfg(feature = "test-hooks")]
//! use solstone_core_journal_io::operational_log::__oplog_create_primitive_is_test_hooks_only;
//! ```
//!
//! ```compile_fail,E0432
//! use solstone_core_journal_io::operational_log::OplogNamespacePrimitive;
//! #[cfg(feature = "test-hooks")]
//! use solstone_core_journal_io::operational_log::__oplog_namespace_primitive_is_test_hooks_only;
//! ```
//!
//! ```compile_fail,E0603
//! let _ = solstone_core_journal_io::operational_log::sample_local_instant;
//! ```

mod admission;
mod catalog;
mod create;
mod follow;
mod lock;
mod name;
mod namespace;
mod reason;
mod writer;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_liveness;

use chrono::{DateTime, FixedOffset, Local};

pub use crate::lease::LeaseProbe;
pub use admission::{
    OPLOG_ADMISSION_MAX_BYTES, OplogAdmissionError, OplogAdmissionRecord, validate_oplog_admission,
};
pub use catalog::{
    OPLOG_CATALOG_CENSUS_ATTEMPTS, OPLOG_CATALOG_MAX_CANDIDATES_PER_DAY,
    OPLOG_CATALOG_MAX_COUNTABLE_ENTRIES_PER_PASS, OplogCatalogEntry, OplogCatalogError,
    OplogCatalogSnapshot, catalog_oplogs, open_oplog_catalog_entry,
    probe_oplog_catalog_entry_lease,
};
#[cfg(windows)]
pub use create::probe_oplog_identity_lease;
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub use create::run_with_oplog_parent_sync_fail;
pub use create::{
    OPLOG_CREATE_ATTEMPTS, OPLOG_FILE_ID_DRAW_BUDGET, create_oplog, create_oplog_at,
    probe_oplog_lease,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use create::{
    OplogCreatePrimitive, create_oplog_with_test_timing, run_with_oplog_create_barrier,
    run_with_oplog_create_fault, run_with_oplog_create_fault_at,
    run_with_oplog_entropy_source_fault, run_with_oplog_entropy_source_fault_at,
    run_with_oplog_file_ids, run_with_oplog_probe_indeterminate, run_with_oplog_sampled_instant,
    run_with_oplog_sampler_fault, run_with_oplog_sync_fail,
};
pub use follow::{
    OplogClock, OplogEntryReaderFactory, OplogFollowReader, OplogFollowState,
    OplogFollowTickOutcome, OplogFollower, OplogIdentityProbe, OplogInitialDiscovery,
    OplogSnapshotSource,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use lock::acquire_oplog_namespace_lock_with_test_timing;
pub use lock::{OplogNamespaceLock, OplogNamespaceLockError, acquire_oplog_namespace_lock};
pub use name::{
    OplogFormat, OplogIdentity, OplogName, OplogNameClassification, OplogNameError,
    classify_oplog_name, derive_day_key_and_opened_field, format_oplog_name,
};
pub use namespace::{OplogDayHealth, OplogNamespaceError, admit_day_health_directory};
#[cfg(any(test, feature = "test-hooks"))]
pub use namespace::{
    OplogNamespacePrimitive, run_with_oplog_namespace_barrier, run_with_oplog_namespace_fault,
};
pub use reason::{
    OplogAdmissionCause, OplogAncestorComponent, OplogCollisionOccupant, OplogCollisionRecord,
    OplogCreateError, OplogCreateLockClass, OplogCreateNamespaceClass, OplogCreateNamespaceStage,
    OplogCreateReason, OplogEvidenceCheckpoint, OplogFileIdentity, OplogGapCause,
    OplogIdentityObservation, OplogNamespaceIdentity, OplogObservationGap, OplogPublishReason,
    OplogStageCause, OplogVerifiedAt, RetainedNamespaceState,
};
pub use writer::{OplogChildCapture, OplogWriteHandle, OplogWriter, OplogWriterError};
#[cfg(any(test, feature = "test-hooks"))]
pub use writer::{run_with_oplog_capture_stderr_fault, run_with_oplog_capture_stdout_fault};

/// Sample one local instant for day-key and UTC-field derivation.
fn sample_local_instant() -> Result<DateTime<FixedOffset>, OplogCreateError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(overridden) = create::take_sampler_override() {
        return overridden;
    }
    Ok(Local::now().fixed_offset())
}
