// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use solstone_core_journal_io::{
    AppendError, AtomicWriteError, LockError, PathError, ReadError, SegmentIdentityError,
};

use crate::ContentNameError;

/// A segment write-door or content-identity failure.
#[derive(Debug)]
pub enum SegmentError {
    Name(ContentNameError),
    Atomic(AtomicWriteError),
    Append(AppendError),
    Lock(LockError),
    Read(ReadError),
    Path(PathError),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    MalformedManifest {
        path: PathBuf,
        message: &'static str,
    },
    UnsupportedManifestSchema {
        path: PathBuf,
        version: Option<u64>,
    },
    IdentityRefusal {
        name: String,
        reason: &'static str,
    },
    Tombstoned {
        path: PathBuf,
    },
    MalformedStreamRecord {
        path: PathBuf,
        source: ReadError,
    },
    StreamInput(&'static str),
    RecordIdentity(SegmentIdentityError),
    InvalidDeviceCid(&'static str),
    InvalidDeviceJid(&'static str),
    StreamBindingConflict {
        name: String,
    },
    StreamAllocationExhausted {
        base: String,
    },
    Serialization {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for SegmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(error) => error.fmt(formatter),
            Self::Atomic(error) => error.fmt(formatter),
            Self::Append(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::MalformedManifest { path, message } => {
                write!(
                    formatter,
                    "malformed ingest manifest {}: {message}",
                    path.display()
                )
            }
            Self::UnsupportedManifestSchema { path, version } => {
                write!(
                    formatter,
                    "unsupported ingest manifest schema at {}: {version:?}",
                    path.display()
                )
            }
            Self::IdentityRefusal { name, reason } => {
                write!(formatter, "content identity refused for {name}: {reason}")
            }
            Self::Tombstoned { path } => {
                write!(formatter, "segment is tombstoned: {}", path.display())
            }
            Self::MalformedStreamRecord { path, source } => {
                write!(
                    formatter,
                    "malformed stream record {}: {source}",
                    path.display()
                )
            }
            Self::StreamInput(message) => formatter.write_str(message),
            Self::RecordIdentity(error) => error.fmt(formatter),
            Self::InvalidDeviceCid(reason) => write!(formatter, "invalid device cid: {reason}"),
            Self::InvalidDeviceJid(reason) => write!(formatter, "invalid device jid: {reason}"),
            Self::StreamBindingConflict { name } => {
                write!(formatter, "stream record binding changed for {name}")
            }
            Self::StreamAllocationExhausted { base } => {
                write!(formatter, "stream name allocation exhausted for {base}")
            }
            Self::Serialization { path, source } => {
                write!(formatter, "{}: {source}", path.display())
            }
        }
    }
}

impl Error for SegmentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Name(error) => Some(error),
            Self::Atomic(error) => Some(error),
            Self::Append(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::MalformedStreamRecord { source, .. } => Some(source),
            Self::Serialization { source, .. } => Some(source),
            Self::RecordIdentity(error) => Some(error),
            Self::MalformedManifest { .. }
            | Self::UnsupportedManifestSchema { .. }
            | Self::IdentityRefusal { .. }
            | Self::Tombstoned { .. }
            | Self::StreamInput(_)
            | Self::InvalidDeviceCid(_)
            | Self::InvalidDeviceJid(_)
            | Self::StreamBindingConflict { .. }
            | Self::StreamAllocationExhausted { .. } => None,
        }
    }
}

impl From<ContentNameError> for SegmentError {
    fn from(error: ContentNameError) -> Self {
        Self::Name(error)
    }
}

impl From<AtomicWriteError> for SegmentError {
    fn from(error: AtomicWriteError) -> Self {
        Self::Atomic(error)
    }
}

impl From<AppendError> for SegmentError {
    fn from(error: AppendError) -> Self {
        Self::Append(error)
    }
}

impl From<LockError> for SegmentError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}

impl From<ReadError> for SegmentError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<PathError> for SegmentError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}
