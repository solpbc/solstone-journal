// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable file-I/O primitives for caller-owned journal paths.

pub mod append;
pub mod atomic;
pub mod errors;
pub mod locking;
pub mod paths;
pub mod readers;

#[cfg(test)]
pub(crate) mod test_support;

pub use append::{append_jsonl, append_text};
pub use atomic::{
    AtomicWriteOptions, JsonWriteOptions, atomic_replace, install_file, write_bytes_exclusive,
    write_json, write_jsonl, write_text,
};
pub use errors::{
    AppendError, AtomicWriteError, LockError, LockTimeout, MalformedDataError, PathError,
    PathEscapeError, ReadError,
};
pub use locking::{
    DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT, FileLock, LockOptions, hold_lock,
};
pub use paths::{
    DEFAULT_STREAM, PathOrDay, Segment, contained_path, day_dirs, day_path, iter_segments,
