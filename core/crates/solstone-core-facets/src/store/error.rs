// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use solstone_core_journal_io::{AtomicWriteError, IdentifierMintError, PathError, ReadError};

use crate::FacetTrustLockError;

/// Failure while reading or inspecting durable facet state.
#[derive(Debug)]
pub enum FacetStoreError {
    Read(ReadError),
    Path(PathError),
    DeclarationNotObject {
        path: PathBuf,
    },
    EntityLinkNotObject {
        path: PathBuf,
    },
    CorruptCompletionMarker {
        path: PathBuf,
    },
    MalformedObservation {
        source: ObservationErrorSource,
        line: usize,
        reason: &'static str,
    },
    MalformedActivityDefinition {
        path: PathBuf,
        line: usize,
        reason: &'static str,
    },
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
            Self::MalformedObservation {
                source,
                line,
                reason,
            } => write!(
                formatter,
                "malformed observation {source} line {line}: {reason}",
            ),
            Self::MalformedActivityDefinition { path, line, reason } => write!(
                formatter,
                "cannot read activity definitions at {} line {line}: {reason}",
                path.display(),
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
            | Self::CorruptCompletionMarker { .. }
            | Self::MalformedObservation { .. }
            | Self::MalformedActivityDefinition { .. } => None,
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
    AlreadyExists {
        path: PathBuf,
    },
    FacetId(FacetIdError),
    DeclarationMissing {
        path: PathBuf,
    },
    /// The declaration exists but is malformed, not an object, or carries a malformed id.
    DeclarationDamaged {
        detail: String,
    },
    /// The declaration exists but could not be read.
    DeclarationUnreadable {
        detail: String,
    },
    DeclarationWrite(AtomicWriteError),
    EntityLinkWrite(AtomicWriteError),
    EntityLinkRemoval(PathError),
    ContentWrite(AtomicWriteError),
    /// Muting or deleting this facet would leave the journal nowhere to route activity.
    LastEnabledFacet {
        facet: String,
    },
    /// The name belonged to a facet that was deleted or merged away; facet
    /// names are never reused.
    NameRetired {
        name: String,
    },
    /// `facets/retired.json` exists but could not be read or parsed; it is
    /// never overwritten.
    RetiredFileDamaged {
        detail: String,
    },
    /// A permanent retired entry already records a different outcome.
    RetiredEntryConflict {
        name: String,
    },
}

pub use solstone_core_entity::{
    ObservationErrorSource, ObservationStoreError, ObservationWriteError,
};

impl fmt::Display for FacetWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::AlreadyExists { path } => {
                write!(
                    formatter,
                    "facet declaration already exists: {}",
                    path.display()
                )
            }
            Self::FacetId(error) => error.fmt(formatter),
            Self::DeclarationMissing { path } => {
                write!(
                    formatter,
                    "facet declaration is missing: {}",
                    path.display()
                )
            }
            Self::DeclarationDamaged { detail } => {
                write!(formatter, "facet declaration is damaged: {detail}")
            }
            Self::DeclarationUnreadable { detail } => {
                write!(formatter, "facet declaration could not be read: {detail}")
            }
            Self::DeclarationWrite(error)
            | Self::EntityLinkWrite(error)
            | Self::ContentWrite(error) => error.fmt(formatter),
            Self::EntityLinkRemoval(error) => error.fmt(formatter),
            Self::LastEnabledFacet { facet } => write!(
                formatter,
                "facet '{facet}' is the only enabled facet; a journal keeps at least one"
            ),
            Self::NameRetired { name } => write!(
                formatter,
                "the facet name '{name}' belonged to a facet that was deleted or merged; facet names are never reused"
            ),
            Self::RetiredFileDamaged { detail } => write!(
                formatter,
                "facets/retired.json could not be read ({detail}); run 'journal facet doctor --fix'"
            ),
            Self::RetiredEntryConflict { name } => write!(
                formatter,
                "the facet name '{name}' is already recorded as retired with a different outcome"
            ),
        }
    }
}

impl Error for FacetWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::FacetId(error) => Some(error),
            Self::DeclarationWrite(error)
            | Self::EntityLinkWrite(error)
            | Self::ContentWrite(error) => Some(error),
            Self::EntityLinkRemoval(error) => Some(error),
            Self::AlreadyExists { .. }
            | Self::DeclarationMissing { .. }
            | Self::DeclarationDamaged { .. }
            | Self::DeclarationUnreadable { .. }
            | Self::LastEnabledFacet { .. }
            | Self::NameRetired { .. }
            | Self::RetiredFileDamaged { .. }
            | Self::RetiredEntryConflict { .. } => None,
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

impl From<FacetIdError> for FacetWriteError {
    fn from(error: FacetIdError) -> Self {
        Self::FacetId(error)
    }
}

/// Failure while allocating, assigning, or backfilling facet identifiers.
#[derive(Debug)]
pub enum FacetIdError {
    Random(IdentifierMintError),
    TrustLock(FacetTrustLockError),
    Store(FacetStoreError),
    Write(Box<FacetWriteError>),
    CollisionExhausted,
    DuplicateIdOnDisk { id: String },
}

impl fmt::Display for FacetIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Random(error) => error.fmt(formatter),
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::CollisionExhausted => {
                formatter.write_str("exhausted retries generating a unique facet identifier")
            }
            Self::DuplicateIdOnDisk { id } => write!(
                formatter,
                "duplicate well-formed facet identifier already on disk: {id}"
            ),
        }
    }
}

impl Error for FacetIdError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Random(error) => Some(error),
            Self::TrustLock(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::CollisionExhausted | Self::DuplicateIdOnDisk { .. } => None,
        }
    }
}

impl From<IdentifierMintError> for FacetIdError {
    fn from(error: IdentifierMintError) -> Self {
        Self::Random(error)
    }
}

impl From<FacetTrustLockError> for FacetIdError {
    fn from(error: FacetTrustLockError) -> Self {
        Self::TrustLock(error)
    }
}

impl From<FacetStoreError> for FacetIdError {
    fn from(error: FacetStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<FacetWriteError> for FacetIdError {
    fn from(error: FacetWriteError) -> Self {
        Self::Write(Box::new(error))
    }
}

/// Failure while resolving a facet identifier to its directory name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetIdResolveError {
    Missing,
    Malformed,
    Duplicate,
    Store,
}

impl fmt::Display for FacetIdResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => {
                formatter.write_str("no facet declaration found with the requested identifier")
            }
            Self::Malformed => formatter.write_str("facet identifier is malformed"),
            Self::Duplicate => {
                formatter.write_str("multiple facet declarations share the requested identifier")
            }
            Self::Store => {
                formatter.write_str("could not read facet declarations to resolve identifier")
            }
        }
    }
}

impl Error for FacetIdResolveError {}

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
    /// The facet already holds a different relationship under this folder.
    RelationshipOccupied {
        relationship_dir: String,
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
            Self::RelationshipOccupied { relationship_dir } => write!(
                formatter,
                "this facet already holds a different entity under '{relationship_dir}'"
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
            | Self::MoveConflict { .. }
            | Self::RelationshipOccupied { .. } => None,
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
impl From<ObservationStoreError> for FacetEntityWriteError {
    fn from(error: ObservationStoreError) -> Self {
        Self::Io(io::Error::other(error.to_string()))
    }
}

impl From<ObservationWriteError> for FacetEntityWriteError {
    fn from(error: ObservationWriteError) -> Self {
        match error {
            ObservationWriteError::TrustLock(e) => Self::TrustLock(e),
            ObservationWriteError::Read(e) => Self::from(e),
            ObservationWriteError::Write(e) => Self::FacetWrite(FacetWriteError::ContentWrite(e)),
            ObservationWriteError::Resolve(_)
            | ObservationWriteError::EmptyContent
            | ObservationWriteError::Conflict { .. } => {
                Self::Io(io::Error::other(error.to_string()))
            }
        }
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
