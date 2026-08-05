// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use solstone_core_entity::EntityAmbiguityRescopeError;
use solstone_core_journal_io::{AtomicWriteError, LockError, PathError, ReadError};

use crate::FacetTrustLockError;

/// Failure while reading or inspecting durable facet state.
#[derive(Debug)]
pub enum FacetStoreError {
    Read(ReadError),
    Path(PathError),
    DeclarationNotObject { path: PathBuf },
    EntityLinkNotObject { path: PathBuf },
    CorruptCompletionMarker { path: PathBuf },
}

impl fmt::Display for FacetStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::DeclarationNotObject { path } => {
                write!(
                    formatter,
                    "facet declaration is not an object: {}",
                    path.display()
                )
            }
            Self::EntityLinkNotObject { path } => {
                write!(
                    formatter,
                    "facet entity link is not an object: {}",
                    path.display()
                )
            }
            Self::CorruptCompletionMarker { path } => write!(
                formatter,
                "facet entity-link repair completion marker is empty or malformed: {}",
                path.display()
            ),
        }
    }
}

impl Error for FacetStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::DeclarationNotObject { .. }
            | Self::EntityLinkNotObject { .. }
            | Self::CorruptCompletionMarker { .. } => None,
        }
    }
}

impl From<ReadError> for FacetStoreError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<PathError> for FacetStoreError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

/// Failure while changing durable facet state.
#[derive(Debug)]
pub enum FacetWriteError {
    TrustLock(FacetTrustLockError),
    Read(FacetStoreError),
    DeclarationMissing { path: PathBuf },
    DeclarationWrite(AtomicWriteError),
    EntityLinkWrite(AtomicWriteError),
    EntityLinkRemoval(PathError),
    ContentWrite(AtomicWriteError),
}

impl fmt::Display for FacetWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::DeclarationMissing { path } => {
                write!(
                    formatter,
                    "facet declaration is missing: {}",
                    path.display()
                )
            }
            Self::DeclarationWrite(error)
            | Self::EntityLinkWrite(error)
            | Self::ContentWrite(error) => error.fmt(formatter),
            Self::EntityLinkRemoval(error) => error.fmt(formatter),
        }
    }
}

impl Error for FacetWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::DeclarationWrite(error)
            | Self::EntityLinkWrite(error)
            | Self::ContentWrite(error) => Some(error),
            Self::EntityLinkRemoval(error) => Some(error),
            Self::DeclarationMissing { .. } => None,
        }
    }
}

impl From<FacetStoreError> for FacetWriteError {
    fn from(error: FacetStoreError) -> Self {
        Self::Read(error)
    }
}

impl From<FacetTrustLockError> for FacetWriteError {
    fn from(error: FacetTrustLockError) -> Self {
        Self::TrustLock(error)
    }
}

/// Failure while renaming one facet directory and its dependent references.
#[derive(Debug)]
pub enum FacetRenameError {
    InvalidName {
        name: String,
    },
    Path(FacetStoreError),
    FacetMissing {
        path: PathBuf,
    },
    DestinationExists {
        path: PathBuf,
    },
    TrustLock(FacetTrustLockError),
    DirectoryRename {
        old_path: PathBuf,
        new_path: PathBuf,
        source: io::Error,
    },
    AmbiguityRescope {
        source: EntityAmbiguityRescopeError,
        rollback: Option<io::Error>,
    },
    ConveyConfigLock(LockError),
    ConveyConfigRead(FacetStoreError),
    ConveyConfigWrite(AtomicWriteError),
}

impl fmt::Display for FacetRenameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { name } => write!(formatter, "invalid facet name: {name:?}"),
            Self::Path(error) => error.fmt(formatter),
            Self::FacetMissing { path } => {
                write!(
                    formatter,
                    "facet declaration is missing: {}",
                    path.display()
                )
            }
            Self::DestinationExists { path } => {
                write!(
                    formatter,
                    "facet rename destination already exists: {}",
                    path.display()
                )
            }
            Self::TrustLock(error) => error.fmt(formatter),
            Self::DirectoryRename {
                old_path,
                new_path,
                source,
            } => write!(
                formatter,
                "cannot rename facet directory {} to {}: {source}",
                old_path.display(),
                new_path.display()
            ),
            Self::AmbiguityRescope { source, rollback } => match rollback {
                Some(rollback) => write!(
                    formatter,
                    "facet ambiguity rescope failed: {source}; directory rollback also failed: {rollback}"
                ),
                None => write!(
                    formatter,
                    "facet ambiguity rescope failed and directory was rolled back: {source}"
                ),
            },
            Self::ConveyConfigLock(error) => error.fmt(formatter),
            Self::ConveyConfigRead(error) => error.fmt(formatter),
            Self::ConveyConfigWrite(error) => error.fmt(formatter),
        }
    }
}

impl Error for FacetRenameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) | Self::ConveyConfigRead(error) => Some(error),
            Self::TrustLock(error) => Some(error),
            Self::DirectoryRename { source, .. } => Some(source),
            Self::AmbiguityRescope { source, .. } => Some(source),
            Self::ConveyConfigLock(error) => Some(error),
            Self::ConveyConfigWrite(error) => Some(error),
            Self::InvalidName { .. }
            | Self::FacetMissing { .. }
            | Self::DestinationExists { .. } => None,
        }
    }
}
