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
pub mod paths;
pub mod readers;
pub mod removal;
pub mod snapshot;
pub mod staged;

#[cfg(test)]
pub(crate) mod test_support;

pub use append::{append_jsonl, append_text};
pub use atomic::{
    AtomicWriteOptions, JsonWriteOptions, atomic_replace, install_file, write_bytes_exclusive,
    write_json, write_jsonl, write_reader_exclusive, write_text,
};
pub use deconflict::{
    SegmentDeconflictError, find_available_segment, find_available_segment_with_occupied,
};
pub use entry::{Removed, remove_file, rename_within, sync_dir};
pub use errors::{
    AppendError, AtomicWriteError, LeaseError, LockError, LockTimeout, MalformedDataError,
    PathError, PathEscapeError, ReadError, SnapshotError,
};
pub use lease::{
    DEFAULT_LEASE_ATTEMPTS, DEFAULT_LEASE_MODE, DEFAULT_LEASE_RETRY_MAX, FileLease, LeaseOptions,
    acquire_file_lease,
};
pub use locking::lock_is_held;
pub use locking::{
    DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT, FileLock, LockOptions, hold_lock,
};
pub use paths::{
    DEFAULT_STREAM, DirEntry, DirEntryKind, PathOrDay, Segment, contained_path,
    create_directory_with_mode, day_dirs, day_path, ensure_directory, iter_segments,
    list_dir_entries, list_dir_entries_bounded, path_lexists, realpath_non_strict,
    resolve_configured_journal, resolve_journal_path, segment_path,
};
pub use readers::{
    JsonlReadReport, JsonlRecord, MalformedPolicy, read_bytes, read_json, read_jsonl,
    read_jsonl_with_report, read_text,
};
pub use removal::remove_dir_all;
pub use snapshot::{
    JournalSnapshot, SnapshotDirectory, SnapshotFile, capture_snapshot, restore_snapshot,
};
pub use staged::{StagedDirOptions, StagedWriteError, publish_staged_dir};
