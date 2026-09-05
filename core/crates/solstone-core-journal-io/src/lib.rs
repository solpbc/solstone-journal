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
#[cfg(any(unix, windows))]
pub mod atomic;
pub mod bounded_read;
#[cfg(unix)]
pub mod claim_remove;
pub mod cortex_use;
mod create_only_retry;
pub mod deconflict;
pub mod entry;
pub mod errors;
mod exclusive_copy;
#[cfg(unix)]
pub mod flat_directory;
pub mod health_marker;
mod install_retry;
pub mod inventory_budget;
pub mod journal_root;
#[cfg(any(unix, windows))]
pub mod lease;
pub mod legacy_log_alias;
#[cfg(any(unix, windows))]
pub mod locking;
pub mod name_admission;
pub mod observation;
#[cfg(any(unix, windows))]
pub mod operational_log;
pub mod paths;
pub mod readers;
pub mod removal;
pub mod snapshot;
#[cfg(any(unix, windows))]
pub mod staged;
pub mod strict_segment;
#[cfg(windows)]
mod windows_disk_space;
#[cfg(windows)]
mod windows_identity;
#[cfg(windows)]
pub mod windows_inventory;
#[cfg(windows)]
mod windows_lock;
#[cfg(windows)]
mod windows_ntcreate;
// Parser is host-neutral (AC1); Windows prepare/revalidate are cfg'd inside.
mod windows_publication_path;
#[cfg(windows)]
pub mod windows_sync_dir;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(any(unix, windows))]
pub use append::append_jsonl;
#[cfg(any(unix, windows))]
pub use append::append_text;
#[cfg(any(unix, windows))]
pub use atomic::install_file;
#[cfg(unix)]
pub use atomic::write_bytes_exclusive_bound_detailed;
#[cfg(any(unix, windows))]
pub use atomic::{AtomicWriteOptions, JsonWriteOptions, atomic_replace, write_json};
#[cfg(unix)]
pub use atomic::{BoundAtomicOutcome, atomic_replace_bound, write_bytes_exclusive_bound};
#[cfg(all(unix, feature = "test-hooks"))]
pub use atomic::{
    BoundPublicationPrimitive, run_with_bound_publication_barrier,
    run_with_bound_publication_fault, run_with_two_bound_publication_barriers,
};
#[cfg(any(unix, windows))]
pub use atomic::{DetailedAtomicError, DetailedAtomicOutcome, atomic_replace_detailed};
#[cfg(any(unix, windows))]
pub use atomic::{
    ExclusivePublication, FinalNameConfirmation, MetadataDurability, StageCleanup,
    write_bytes_exclusive_detailed, write_reader_exclusive_detailed,
};
#[cfg(all(windows, feature = "test-hooks"))]
pub use atomic::{
    WindowsCreateOnlyPrimitive, WindowsCreateOnlyTrace, run_with_windows_create_only_barrier,
    run_with_windows_create_only_faults, run_with_windows_create_only_faults_and_barrier,
};
#[cfg(all(windows, feature = "test-hooks"))]
pub use atomic::{
    WindowsInstallPrimitive, WindowsInstallTrace, run_with_windows_install_barrier,
    run_with_windows_install_faults, run_with_windows_install_faults_and_barrier,
};
#[cfg(all(unix, feature = "test-hooks"))]
pub use atomic::{
    run_with_bound_publication_faults, run_with_bound_publication_faults_and_barrier,
};
#[cfg(any(unix, windows))]
pub use atomic::{write_bytes_exclusive, write_reader_exclusive};
#[cfg(any(unix, windows))]
pub use atomic::{write_jsonl, write_text};
pub use bounded_read::{JournalReadError, MAX_BYTES, resolve_read_path};
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
pub use entry::{Removed, remove_file, rename_within, sync_dir};
pub use errors::{
    AppendError, AtomicWriteError, ClaimDurability, ClaimRemovalError, ClaimRemovalOutcome,
    ClaimUnchangedReason, ExistingParentLockError, FlatDirectoryError, IdentityChangeDisposition,
    LeaseError, LockError, LockTimeout, MalformedDataError, NoReplacePrimitive, PathError,
    PathEscapeError, ReadError, SegmentIdentityError, SnapshotError,
};
#[cfg(unix)]
pub use flat_directory::{
    FlatDirectory, create_or_open_flat_directory_bound, list_flat_directory,
    open_flat_directory_bound, read_observed_file, read_observed_file_bounded,
    read_observed_root_file_bounded,
};
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
pub use lease::probe_exclusive_flock_no_release;
#[cfg(any(unix, windows))]
pub use lease::{
    DEFAULT_LEASE_ATTEMPTS, DEFAULT_LEASE_MODE, DEFAULT_LEASE_RETRY_MAX, FileLease, LeaseOptions,
    acquire_file_lease,
};
pub use legacy_log_alias::{
    LegacyAliasCleanupError, LegacyAliasCleanupReport, LegacyAliasDisposition,
    LegacyAliasObservation, LegacyAliasObservationResult, LegacyAliasRefusal, LegacyAliasRefused,
    LegacyAliasTarget, cleanup_legacy_log_aliases, observe_legacy_alias_symlink,
    remove_observed_legacy_alias_symlink,
};
#[cfg(any(unix, windows))]
pub use locking::lock_is_held;
#[cfg(any(unix, windows))]
pub use locking::{BoundParentLock, acquire_existing_parent_lock_bound};
#[cfg(any(unix, windows))]
pub use locking::{
    DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT, ExistingParentLock, FileLock, LockOptions,
    acquire_existing_parent_lock, hold_lock,
};
pub use name_admission::{
    ClaimName, ConflictEntry, ConflictKind, NameAdmissionError, NameAdmissionReason,
    NoFollowEntryKind, StreamName, check_portable_component,
};
pub use observation::{FileObservation, FlatDirectoryEntry, NativeMtime};
pub use paths::{
    DEFAULT_STREAM, DirEntry, DirEntryKind, PathOrDay, RecordIdentity, Segment, SegmentLayout,
    SegmentLocatorIdentity, StreamLocation, check_record_identities, check_unique_record_keys,
    contained_path, create_directory_with_mode, day_dirs, day_path, ensure_directory, is_day_key,
    iter_segments, list_dir_entries, list_dir_entries_bounded, path_lexists, realpath_non_strict,
    resolve_configured_journal, resolve_journal_path, segment_path, utf8_identities,
};
#[cfg(unix)]
pub use readers::read_bytes_bound;
#[cfg(all(unix, feature = "test-hooks"))]
pub use readers::{
    BoundReadPrimitive, run_with_bound_read_barrier, run_with_bound_read_fault,
    run_with_bound_read_fault_trace, run_with_two_bound_read_barriers,
};
pub use readers::{
    JsonlReadReport, JsonlRecord, MalformedPolicy, read_bytes, read_json, read_jsonl,
    read_jsonl_with_report, read_text,
};
pub use removal::{remove_contained_tree, remove_dir_all};
pub use snapshot::{
    JournalSnapshot, SnapshotDirectory, SnapshotFile, capture_snapshot, restore_snapshot,
};
#[cfg(any(unix, windows))]
pub use staged::{StagedDirOptions, StagedWriteError, publish_staged_dir};
pub use strict_segment::{
    ExactLookupError, StrictCreateError, create_segment_strict, preflight_segment_admission,
    resolve_segment_exact, resolve_segment_locator_exact, resolve_stream_exact,
};
#[cfg(windows)]
pub use windows_disk_space::{WindowsDiskSpace, windows_available_disk_bytes, windows_disk_space};
#[cfg(windows)]
pub use windows_identity::{WindowsFileIdentity, windows_file_identity};
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
#[cfg(all(windows, feature = "test-hooks"))]
pub use windows_lock::{
    WindowsLockFileExSubstitution, WindowsUnlockFileExObservation,
    run_with_forced_post_lock_identity_mismatch, run_with_windows_lock_file_ex_substitution,
    run_with_windows_lock_file_ex_trace, run_with_windows_unlock_file_ex_observation,
};
#[cfg(windows)]
pub use windows_sync_dir::{
    WindowsFlatDirectory, create_or_open_windows_flat_directory_bound, list_windows_flat_directory,
    open_windows_flat_directory_bound, open_windows_regular_file_from_bound_parent,
    read_windows_observed_file_bounded,
};
