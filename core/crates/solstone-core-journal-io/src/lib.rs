// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable file-I/O primitives for caller-owned journal paths.

pub mod append;
pub mod atomic;
pub mod config;
pub mod errors;
pub mod locking;
pub mod paths;
pub mod readers;
pub mod removal;
pub mod staged;

#[cfg(test)]
pub(crate) mod test_support;

pub use append::{append_jsonl, append_text};
pub use atomic::{
    AtomicWriteOptions, JsonWriteOptions, atomic_replace, install_file, write_bytes_exclusive,
    write_json, write_jsonl, write_text,
};
pub use config::{
    ConfigLoadError, ConfigMutationError, JournalConfigMutation, JournalConfigTransaction,
    get_journal_config_path, mutate_journal_config,
};
pub use errors::{
    AppendError, AtomicWriteError, LockError, LockTimeout, MalformedDataError, PathError,
    PathEscapeError, ReadError,
};
pub use locking::{
    DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT, FileLock, LockOptions, hold_lock,
};
pub use paths::{
    DEFAULT_STREAM, DirEntry, DirEntryKind, PathOrDay, Segment, contained_path, day_dirs, day_path,
    iter_segments, list_dir_entries, path_lexists, resolve_configured_journal,
    resolve_journal_path, segment_path,
};
pub use readers::{MalformedPolicy, read_json, read_jsonl, read_text};
pub use removal::remove_dir_all;
pub use staged::{StagedDirOptions, StagedWriteError, publish_staged_dir};
