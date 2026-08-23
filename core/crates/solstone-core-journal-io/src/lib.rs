// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable file-I/O primitives for caller-owned journal paths.
//!
//! ```compile_fail,E0425
//! let _ = solstone_core_journal_io::read_journal_config;
//! ```
//!
pub mod append;
pub mod atomic;
pub mod deconflict;
pub mod entry;
pub mod errors;
pub mod lease;
pub mod locking;
pub mod name_admission;
pub mod paths;
pub mod readers;
pub mod removal;
pub mod snapshot;
pub mod staged;
pub mod strict_segment;

#[cfg(test)]
pub(crate) mod test_support;

pub use append::{append_jsonl, append_text};
pub use atomic::{
    AtomicWriteOptions, DetailedAtomicError, DetailedAtomicOutcome, JsonWriteOptions,
    atomic_replace, atomic_replace_detailed, install_file, write_bytes_exclusive, write_json,
    write_jsonl, write_reader_exclusive, write_text,
};
pub use deconflict::{
    SegmentDeconflictError, find_available_segment, find_available_segment_with_occupied,
};
pub use entry::{Removed, remove_file, rename_within, sync_dir};
pub use errors::{
    AppendError, AtomicWriteError, ExistingParentLockError, LeaseError, LockError, LockTimeout,
    MalformedDataError, PathError, PathEscapeError, ReadError, SegmentIdentityError, SnapshotError,
};
pub use lease::{
    DEFAULT_LEASE_ATTEMPTS, DEFAULT_LEASE_MODE, DEFAULT_LEASE_RETRY_MAX, FileLease, LeaseOptions,
    acquire_file_lease,
};
pub use locking::lock_is_held;
pub use locking::{
    DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT, ExistingParentLock, FileLock, LockOptions,
    acquire_existing_parent_lock, hold_lock,
};
pub use name_admission::{
    ConflictEntry, ConflictKind, NameAdmissionError, NameAdmissionReason, NoFollowEntryKind,
    StreamName, check_portable_component,
};
pub use paths::{
    DEFAULT_STREAM, DirEntry, DirEntryKind, PathOrDay, RecordIdentity, Segment, StreamLocation,
    check_record_identities, check_unique_record_keys, contained_path, create_directory_with_mode,
    day_dirs, day_path, ensure_directory, iter_segments, list_dir_entries,
    list_dir_entries_bounded, path_lexists, realpath_non_strict, resolve_configured_journal,
    resolve_journal_path, segment_path, utf8_identities,
};
pub use readers::{
    JsonlReadReport, JsonlRecord, MalformedPolicy, read_bytes, read_json, read_jsonl,
    read_jsonl_with_report, read_text,
};
pub use removal::{remove_contained_tree, remove_dir_all};
pub use snapshot::{
    JournalSnapshot, SnapshotDirectory, SnapshotFile, capture_snapshot, restore_snapshot,
};
pub use staged::{StagedDirOptions, StagedWriteError, publish_staged_dir};
pub use strict_segment::{
    ExactLookupError, StrictCreateError, create_segment_strict, preflight_segment_admission,
    resolve_segment_exact, resolve_stream_exact,
};
