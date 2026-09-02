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

/// Failure while applying a facet-scoped observation mutation.
#[derive(Debug)]
pub enum ObservationWriteError {
    EmptyContent,
    TrustLock(FacetTrustLockError),
    Read(FacetStoreError),
    Write(FacetWriteError),
    Resolve(FacetEntityWriteError),
}

impl fmt::Display for ObservationWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyContent => formatter.write_str("observation content cannot be empty"),
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Resolve(error) => error.fmt(formatter),
        }
    }
}

impl Error for ObservationWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::Resolve(error) => Some(error),
            Self::EmptyContent => None,
        }
    }
}

impl From<FacetTrustLockError> for ObservationWriteError {
    fn from(error: FacetTrustLockError) -> Self {
        Self::TrustLock(error)
    }
}

impl From<FacetStoreError> for ObservationWriteError {
    fn from(error: FacetStoreError) -> Self {
        Self::Read(error)
    }
}

impl From<FacetWriteError> for ObservationWriteError {
    fn from(error: FacetWriteError) -> Self {
        Self::Write(error)
    }
}

impl ObservationWriteError {
    pub(crate) fn is_lock_timeout(&self) -> bool {
        match self {
            Self::TrustLock(error) => trust_lock_is_timeout(error),
            Self::Write(FacetWriteError::TrustLock(error)) => trust_lock_is_timeout(error),
            Self::Resolve(FacetEntityWriteError::TrustLock(error)) => trust_lock_is_timeout(error),
            _ => false,
        }
    }

    pub(crate) fn is_retryable_io(&self) -> bool {
        match self {
            Self::TrustLock(error) => trust_lock_is_io(error),
            Self::Read(error) => store_error_is_io(error),
            Self::Write(error) => write_error_is_io(error),
            Self::Resolve(FacetEntityWriteError::TrustLock(error)) => trust_lock_is_io(error),
            Self::Resolve(FacetEntityWriteError::Io(_)) => true,
            Self::EmptyContent | Self::Resolve(_) => false,
        }
    }
}

fn trust_lock_is_timeout(error: &FacetTrustLockError) -> bool {
    matches!(error, FacetTrustLockError::Lock(LockError::Timeout(_)))
}

fn trust_lock_is_io(error: &FacetTrustLockError) -> bool {
    matches!(
        error,
        FacetTrustLockError::Lock(LockError::Io { .. })
            | FacetTrustLockError::Path(PathError::Io { .. })
    )
}

fn store_error_is_io(error: &FacetStoreError) -> bool {
    matches!(
        error,
        FacetStoreError::Read(ReadError::Io { .. }) | FacetStoreError::Path(PathError::Io { .. })
    )
}

fn write_error_is_io(error: &FacetWriteError) -> bool {
    match error {
        FacetWriteError::TrustLock(error) => trust_lock_is_io(error),
        FacetWriteError::Read(error) => store_error_is_io(error),
        FacetWriteError::DeclarationWrite(AtomicWriteError::Io { .. })
        | FacetWriteError::EntityLinkWrite(AtomicWriteError::Io { .. })
        | FacetWriteError::ContentWrite(AtomicWriteError::Io { .. })
        | FacetWriteError::EntityLinkRemoval(PathError::Io { .. }) => true,
        #[cfg(windows)]
        FacetWriteError::DeclarationWrite(AtomicWriteError::PublicationUncertain { .. })
        | FacetWriteError::EntityLinkWrite(AtomicWriteError::PublicationUncertain { .. })
        | FacetWriteError::ContentWrite(AtomicWriteError::PublicationUncertain { .. }) => false,
        FacetWriteError::DeclarationMissing { .. } | FacetWriteError::EntityLinkRemoval(_) => false,
    }
}

/// Failure while resolving a query or reading its facet-scoped observations.
#[derive(Debug)]
pub enum ObservationLookupError {
    Resolve(FacetEntityWriteError),
    Read {
        entity_dir: String,
        source: FacetStoreError,
    },
}

impl fmt::Display for ObservationLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolve(error) => error.fmt(formatter),
            Self::Read { source, .. } => source.fmt(formatter),
        }
    }
}

impl Error for ObservationLookupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Resolve(error) => Some(error),
            Self::Read { source, .. } => Some(source),
        }
    }
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

/// Failure while changing a facet entity relationship or its journal identity.
#[derive(Debug)]
pub enum FacetEntityWriteError {
    TrustLock(FacetTrustLockError),
    EntityTrustLock(solstone_core_entity::EntityTrustLockError),
    FacetStore(FacetStoreError),
    FacetWrite(FacetWriteError),
    EntityStore(solstone_core_entity::EntityStoreError),
    EntityWrite(solstone_core_entity::EntityWriteError),
    EntityExists {
        name: String,
    },
    EntityBlocked {
        entity_id: String,
    },
    EntityNotFound {
        entity_id: String,
    },
    AkaConflict {
        alias: String,
        conflict_name: String,
    },
    IdentityMapLoser {
        entity_id: String,
        entity_dir: String,
    },
    MoveConflict {
        path: PathBuf,
    },
    Io(io::Error),
}

impl fmt::Display for FacetEntityWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::EntityTrustLock(error) => error.fmt(formatter),
            Self::FacetStore(error) => error.fmt(formatter),
            Self::FacetWrite(error) => error.fmt(formatter),
            Self::EntityStore(error) => error.fmt(formatter),
            Self::EntityWrite(error) => error.fmt(formatter),
            Self::EntityExists { name } => write!(formatter, "entity already exists: {name:?}"),
            Self::EntityBlocked { entity_id } => {
                write!(formatter, "entity is blocked: {entity_id}")
            }
            Self::EntityNotFound { entity_id } => {
                write!(formatter, "entity not found: {entity_id}")
            }
            Self::AkaConflict {
                alias,
                conflict_name,
            } => write!(
                formatter,
                "alias {alias:?} conflicts with entity {conflict_name:?}"
            ),
            Self::IdentityMapLoser {
                entity_id,
                entity_dir,
            } => write!(
                formatter,
                "identity-map loser {entity_dir} for effective id {entity_id}"
            ),
            Self::MoveConflict { path } => write!(
                formatter,
                "cannot account for conflicting moved file: {}",
                path.display()
            ),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl Error for FacetEntityWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::EntityTrustLock(error) => Some(error),
            Self::FacetStore(error) => Some(error),
            Self::FacetWrite(error) => Some(error),
            Self::EntityStore(error) => Some(error),
            Self::EntityWrite(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::EntityExists { .. }
            | Self::EntityBlocked { .. }
            | Self::EntityNotFound { .. }
            | Self::AkaConflict { .. }
            | Self::IdentityMapLoser { .. }
            | Self::MoveConflict { .. } => None,
        }
    }
}

impl From<FacetTrustLockError> for FacetEntityWriteError {
    fn from(value: FacetTrustLockError) -> Self {
        Self::TrustLock(value)
    }
}
impl From<solstone_core_entity::EntityTrustLockError> for FacetEntityWriteError {
    fn from(value: solstone_core_entity::EntityTrustLockError) -> Self {
        Self::EntityTrustLock(value)
    }
}
impl From<FacetStoreError> for FacetEntityWriteError {
    fn from(value: FacetStoreError) -> Self {
        Self::FacetStore(value)
    }
}
impl From<FacetWriteError> for FacetEntityWriteError {
    fn from(value: FacetWriteError) -> Self {
        Self::FacetWrite(value)
    }
}
impl From<solstone_core_entity::EntityStoreError> for FacetEntityWriteError {
    fn from(value: solstone_core_entity::EntityStoreError) -> Self {
        Self::EntityStore(value)
    }
}
impl From<solstone_core_entity::EntityWriteError> for FacetEntityWriteError {
    fn from(value: solstone_core_entity::EntityWriteError) -> Self {
        Self::EntityWrite(value)
    }
}
impl From<io::Error> for FacetEntityWriteError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
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
