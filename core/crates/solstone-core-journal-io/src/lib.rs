// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable file-I/O primitives for caller-owned journal paths.
//!
//! ```compile_fail,E0425
//! let _ = solstone_core_journal_io::read_journal_config;
//! ```
//!
#[cfg(not(any(unix, windows)))]
compile_error!(
    "solstone-core-journal-io requires a Unix or Windows target: atomic write, locking, and lease durability guarantees have no portable backend"
);

#[cfg(any(unix, windows))]
pub mod append;
#[cfg(unix)]
pub mod atomic;
#[cfg(unix)]
pub mod claim_remove;
pub mod deconflict;
pub mod entry;
pub mod errors;
#[cfg(unix)]
pub mod flat_directory;
#[cfg(unix)]
pub mod health_marker;
pub mod inventory_budget;
pub mod journal_root;
#[cfg(unix)]
pub mod lease;
#[cfg(unix)]
pub mod locking;
pub mod name_admission;
pub mod paths;
pub mod readers;
pub mod removal;
#[cfg(unix)]
pub mod snapshot;
#[cfg(unix)]
pub mod staged;
pub mod strict_segment;
#[cfg(windows)]
pub mod windows_inventory;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(any(unix, windows))]
pub use append::append_jsonl;
#[cfg(unix)]
pub use append::append_text;
#[cfg(unix)]
pub use atomic::{
    AtomicWriteOptions, DetailedAtomicError, DetailedAtomicOutcome, JsonWriteOptions,
    atomic_replace, atomic_replace_detailed, install_file, write_bytes_exclusive, write_json,
    write_jsonl, write_reader_exclusive, write_text,
};
#[cfg(unix)]
pub use atomic::{BoundAtomicOutcome, atomic_replace_bound, write_bytes_exclusive_bound};
#[cfg(all(unix, feature = "test-hooks"))]
pub use atomic::{
    BoundPublicationPrimitive, run_with_bound_publication_barrier,
    run_with_bound_publication_fault, run_with_two_bound_publication_barriers,
};
#[cfg(unix)]
pub use claim_remove::claim_and_remove_observed;
#[cfg(all(unix, feature = "test-hooks"))]
pub use claim_remove::{
    ClaimRemovalPrimitive, run_with_claim_removal_barrier, run_with_claim_removal_fault,
    run_with_two_claim_removal_barriers,
};
pub use deconflict::{
    SegmentDeconflictError, find_available_segment, find_available_segment_with_occupied,
};
#[cfg(unix)]
pub use entry::sync_dir_bound;
pub use entry::{Removed, remove_file, rename_within, sync_dir};
pub use errors::{
    AppendError, AtomicWriteError, ClaimDurability, ClaimRemovalError, ClaimRemovalOutcome,
    ClaimUnchangedReason, ExistingParentLockError, FlatDirectoryError, IdentityChangeDisposition,
    LeaseError, LockError, LockTimeout, MalformedDataError, NoReplacePrimitive, PathError,
    PathEscapeError, ReadError, SegmentIdentityError, SnapshotError,
};
#[cfg(unix)]
pub use flat_directory::{
    FileObservation, FlatDirectory, FlatDirectoryEntry, NativeMtime,
    create_or_open_flat_directory_bound, list_flat_directory, open_flat_directory_bound,
    read_observed_file, read_observed_file_bounded,
};
#[cfg(unix)]
pub use health_marker::{
    DayMarkerPairStatus, HealthMarker, HealthMarkerError, HealthMarkerKind, HealthMarkerState,
    PublishOutcome, bump_stream_marker, day_marker_pair_status, health_marker_path,
    publish_daily_marker_if_current, read_health_marker,
};
pub use inventory_budget::{InventoryBudget, InventoryBudgetLimit};
#[cfg(all(unix, feature = "test-hooks"))]
pub use journal_root::{AcquisitionPrimitive, run_with_acquisition_fault};
pub use journal_root::{
    JournalEntryKind, JournalRoot, JournalRootError, ObjectIdentity, WindowsRefusalCategory,
};
#[cfg(all(windows, feature = "test-hooks"))]
pub use journal_root::{
    WindowsAcquisitionPrimitive, WindowsAcquisitionTrace, run_with_windows_acquisition_fault,
    run_with_windows_acquisition_trace,
};
#[cfg(unix)]
pub use lease::{
    DEFAULT_LEASE_ATTEMPTS, DEFAULT_LEASE_MODE, DEFAULT_LEASE_RETRY_MAX, FileLease, LeaseOptions,
    acquire_file_lease,
};
#[cfg(unix)]
pub use locking::lock_is_held;
#[cfg(unix)]
pub use locking::{BoundParentLock, acquire_existing_parent_lock_bound};
#[cfg(unix)]
pub use locking::{
    DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT, ExistingParentLock, FileLock, LockOptions,
    acquire_existing_parent_lock, hold_lock,
};
pub use name_admission::{
    ClaimName, ConflictEntry, ConflictKind, NameAdmissionError, NameAdmissionReason,
    NoFollowEntryKind, StreamName, check_portable_component,
};
#[cfg(unix)]
pub use paths::create_directory_bound;
pub use paths::{
    DEFAULT_STREAM, DirEntry, DirEntryKind, PathOrDay, RecordIdentity, Segment, SegmentLayout,
    SegmentLocatorIdentity, StreamLocation, check_record_identities, check_unique_record_keys,
    contained_path, create_directory_with_mode, day_dirs, day_path, ensure_directory, is_day_key,
    iter_segments, list_dir_entries, list_dir_entries_bounded, path_lexists, realpath_non_strict,
    resolve_configured_journal, resolve_journal_path, segment_path, utf8_identities,
};
#[cfg(unix)]
pub use readers::read_bytes_bound;
pub use readers::{
    JsonlReadReport, JsonlRecord, MalformedPolicy, read_bytes, read_json, read_jsonl,
    read_jsonl_with_report, read_text,
};
pub use removal::{remove_contained_tree, remove_dir_all};
#[cfg(unix)]
pub use snapshot::{
    JournalSnapshot, SnapshotDirectory, SnapshotFile, capture_snapshot, restore_snapshot,
};
#[cfg(unix)]
pub use staged::{StagedDirOptions, StagedWriteError, publish_staged_dir};
pub use strict_segment::{
    ExactLookupError, StrictCreateError, create_segment_strict, preflight_segment_admission,
    resolve_segment_exact, resolve_segment_locator_exact, resolve_stream_exact,
};
#[cfg(windows)]
pub use windows_inventory::{
    WindowsCheckedReadSession, WindowsInventory, WindowsInventoryEntry, WindowsInventoryError,
    enumerate_windows_inventory, read_windows_inventory_file,
};
#[cfg(all(windows, feature = "test-hooks"))]
pub use windows_inventory::{
    WindowsInventoryPrimitive, WindowsInventoryTrace, run_with_windows_inventory_fault,
    run_with_windows_inventory_trace,
};
