// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native write authority for `journal/identity/*` and its audit log.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use solstone_core_journal_io::{AppendError, AtomicWriteError, LockError};

mod fixture;
mod section;
mod store;

pub use store::{ensure_identity_directory, update_identity_section, write_identity};

/// Identity-store failure.
#[derive(Debug)]
pub enum IdentityError {
    /// Advisory-lock acquisition failed.
    Lock(LockError),
    /// Atomic target publication or restoration failed.
    Atomic(AtomicWriteError),
    /// Audit-history append failed.
    Append(AppendError),
    /// Reading an existing identity target failed.
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Atomic(error) => error.fmt(formatter),
            Self::Append(error) => error.fmt(formatter),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for IdentityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Atomic(error) => Some(error),
            Self::Append(error) => Some(error),
            Self::Io { source, .. } => Some(source),
        }
    }
}

impl From<LockError> for IdentityError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}

impl From<AtomicWriteError> for IdentityError {
    fn from(error: AtomicWriteError) -> Self {
        Self::Atomic(error)
    }
}

impl From<AppendError> for IdentityError {
    fn from(error: AppendError) -> Self {
        Self::Append(error)
    }
}
