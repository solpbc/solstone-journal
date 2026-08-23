// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Error types for journal file-I/O primitives.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// Raised when a stable sidecar lock is not acquired before the deadline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockTimeout {
    /// The protected path, not its sidecar lock path.
    pub path: PathBuf,
    /// The requested acquisition timeout.
    pub timeout: Duration,
}

impl fmt::Display for LockTimeout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "could not acquire lock for {} within {}s",
            self.path.display(),
            self.timeout.as_secs_f64()
        )
    }
}

impl Error for LockTimeout {}

/// Raised when JSON or JSONL data is malformed under strict policy.
#[derive(Debug)]
pub struct MalformedDataError {
    /// File that contained malformed JSON data.
    pub path: PathBuf,
    /// One-based JSONL line number, when applicable.
    pub line: Option<usize>,
    /// Underlying JSON parse failure.
    pub source: serde_json::Error,
}

impl fmt::Display for MalformedDataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(
                formatter,
                "malformed data in {} at line {line}",
                self.path.display()
            ),
            None => write!(formatter, "malformed data in {}", self.path.display()),
        }
    }
}

impl Error for MalformedDataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

/// Raised when a journal-relative path escapes after symlink resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEscapeError {
    /// Resolved candidate outside the journal root.
    pub path: PathBuf,
    /// Original relative input.
    pub rel: String,
}

impl fmt::Display for PathEscapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?} escapes the journal root (resolved to {})",
            self.rel,
            self.path.display()
        )
    }
}

impl Error for PathEscapeError {}

/// Lock acquisition failure.
#[derive(Debug)]
pub enum LockError {
    /// A filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
    /// Acquisition exceeded the supplied deadline.
    Timeout(LockTimeout),
}

impl fmt::Display for LockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Timeout(error) => error.fmt(formatter),
        }
    }
}

impl Error for LockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Timeout(error) => Some(error),
        }
    }
}

/// Failure while acquiring a persistent lock entry under an existing parent.
#[derive(Debug)]
pub enum ExistingParentLockError {
    /// The caller did not supply exactly one normal lock-entry name.
    InvalidLockPath { name: OsString },
    /// The requested parent directory does not exist.
    MissingParent { parent: PathBuf },
    /// The requested parent is a symlink or is not a directory.
    UnsafeParent { parent: PathBuf, kind: &'static str },
    /// The persistent lock entry is a symlink or is not a regular file.
    UnsafeLockEntry { path: PathBuf, kind: &'static str },
    /// The persistent lock entry does not have the required mode.
    WrongMode { path: PathBuf, observed: u32 },
    /// A filesystem operation failed.
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    /// The requested parent changed after it was inspected.
    ParentChanged { parent: PathBuf },
    /// The persistent lock-entry name changed during acquisition.
    NamespaceChanged { path: PathBuf },
    /// Acquisition exceeded the supplied deadline.
    Timeout(LockTimeout),
}

impl fmt::Display for ExistingParentLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLockPath { name } => {
                write!(formatter, "invalid persistent lock entry {name:?}")
            }
            Self::MissingParent { parent } => {
                write!(
                    formatter,
                    "persistent lock parent is missing: {}",
                    parent.display()
                )
            }
            Self::UnsafeParent { parent, kind } => write!(
                formatter,
                "persistent lock parent is an unsafe {kind}: {}",
                parent.display()
            ),
            Self::UnsafeLockEntry { path, kind } => write!(
                formatter,
                "persistent lock entry is an unsafe {kind}: {}",
                path.display()
            ),
            Self::WrongMode { path, observed } => write!(
                formatter,
                "persistent lock entry has mode {observed:o}, expected 600: {}",
                path.display()
            ),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} failed for {}: {source}",
                path.display()
            ),
            Self::ParentChanged { parent } => write!(
                formatter,
                "persistent lock parent changed during acquisition: {}",
                parent.display()
            ),
            Self::NamespaceChanged { path } => write!(
                formatter,
                "persistent lock entry changed during acquisition: {}",
                path.display()
            ),
            Self::Timeout(error) => error.fmt(formatter),
        }
    }
}

impl Error for ExistingParentLockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// File-lease acquisition failure.
#[derive(Debug)]
pub enum LeaseError {
    /// A filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for LeaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
        }
    }
}

/// Reader failure. It deliberately carries no decoded value.
#[derive(Debug)]
pub enum ReadError {
    /// A filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
    /// JSON content was malformed under strict policy.
    Malformed(MalformedDataError),
}

impl fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Malformed(error) => error.fmt(formatter),
        }
    }
}

impl Error for ReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Malformed(error) => Some(error),
        }
    }
}

/// A discovered segment cannot be named as a `(stream, key)` record.
///
/// Covers genuine non-UTF-8 names and a UTF-8 name that collides with Direct's
/// reserved `_default` spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentIdentityError {
    /// Stream directory or segment basename is not UTF-8.
    ///
    /// Shared by `record_identity()` and `locator_identity()`.
    NotUtf8 { path: PathBuf },
    /// A directory literally named `_default` cannot share Direct's record spelling.
    AmbiguousNamedDefault { path: PathBuf },
    /// Two selected segments share the same UTF-8 stream spelling and parsed key.
    DuplicateKey { stream: String, key: String },
}

impl fmt::Display for SegmentIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 { path } => write!(
                formatter,
                "segment path is not UTF-8 representable: {}",
                path.display()
            ),
            Self::AmbiguousNamedDefault { path } => write!(
                formatter,
                "named stream directory \"_default\" cannot be spelled as a record identity: {}",
                path.display()
            ),
            Self::DuplicateKey { stream, key } => write!(
                formatter,
                "multiple segments share stream {stream:?} key {key:?}"
            ),
        }
    }
}

impl Error for SegmentIdentityError {}

/// Journal path validation or filesystem failure.
#[derive(Debug)]
pub enum PathError {
    /// The caller supplied an invalid journal-relative path.
    InvalidRelativePath { rel: String, message: &'static str },
    /// A symlink-aware containment check failed.
    Escape(PathEscapeError),
    /// A filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRelativePath { message, .. } => formatter.write_str(message),
            Self::Escape(error) => error.fmt(formatter),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for PathError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Escape(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::InvalidRelativePath { .. } => None,
        }
    }
}

/// Atomic writer failure.
#[derive(Debug)]
pub enum AtomicWriteError {
    /// A filesystem or durability operation failed.
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for AtomicWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for AtomicWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
        }
    }
}

/// Append writer failure.
#[derive(Debug)]
pub enum AppendError {
    /// A filesystem or durability operation failed.
    Io { path: PathBuf, source: io::Error },
}

/// Recursive snapshot capture or restore failure.
#[derive(Debug)]
pub enum SnapshotError {
    /// A journal-relative path was invalid or escaped the journal root.
    Path(PathError),
    /// A filesystem operation failed.
    Io { path: PathBuf, source: io::Error },
    /// Atomic file publication failed during restore.
    Atomic(AtomicWriteError),
    /// A symlink or other unsupported filesystem object was encountered.
    UnsupportedFileType { path: PathBuf },
    /// The supplied snapshot has an invalid structure.
    InvalidSnapshot { path: String, message: &'static str },
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Atomic(error) => error.fmt(formatter),
            Self::UnsupportedFileType { path } => {
                write!(
                    formatter,
                    "unsupported filesystem object at {}",
                    path.display()
                )
            }
            Self::InvalidSnapshot { message, .. } => formatter.write_str(message),
        }
    }
}

impl Error for SnapshotError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::Atomic(error) => Some(error),
            Self::UnsupportedFileType { .. } | Self::InvalidSnapshot { .. } => None,
        }
    }
}

impl fmt::Display for AppendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for AppendError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
        }
    }
}
