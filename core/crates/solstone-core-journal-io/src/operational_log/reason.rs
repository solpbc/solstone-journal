// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed causal reasons and identity evidence for operational-log create.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;

use crate::journal_root::ObjectIdentity;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogCreateReason {
    InvalidField,
    EntropySource,
    EntropyExhausted,
    Clock,
    Stage,
    Admission,
    LeaseFailed,
    LeaseIo,
    Namespace {
        stage: OplogCreateNamespaceStage,
        class: OplogCreateNamespaceClass,
    },
    Lock(OplogCreateLockClass),
    Publish(OplogPublishReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogCreateNamespaceStage {
    Chronicle,
    Day,
    Health,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogCreateNamespaceClass {
    Unsafe,
    IdentityChanged,
    Io,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogCreateLockClass {
    Unsafe,
    IdentityChanged,
    Busy,
    Io,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogPublishReason {
    Rename,
    Reconciliation,
    DestinationInspection,
    Alias,
    DestinationExhaustion,
    DirectorySync,
    FinalBinding,
    AncestorRevalidation,
}

impl OplogCreateReason {
    fn token(self) -> String {
        match self {
            Self::InvalidField => "oplog_create_invalid_field".to_owned(),
            Self::EntropySource => "oplog_create_entropy_source".to_owned(),
            Self::EntropyExhausted => "oplog_create_entropy_exhausted".to_owned(),
            Self::Clock => "oplog_create_clock".to_owned(),
            Self::Stage => "oplog_create_stage".to_owned(),
            Self::Admission => "oplog_create_admission".to_owned(),
            Self::LeaseFailed => "oplog_create_lease_failed".to_owned(),
            Self::LeaseIo => "oplog_create_lease_io".to_owned(),
            Self::Namespace { stage, class } => {
                format!("oplog_create_namespace_{}_{}", stage.token(), class.token())
            }
            Self::Lock(class) => format!("oplog_create_lock_{}", class.token()),
            Self::Publish(reason) => format!("oplog_create_{}", reason.token()),
        }
    }
}

impl OplogCreateNamespaceStage {
    const fn token(self) -> &'static str {
        match self {
            Self::Chronicle => "chronicle",
            Self::Day => "day",
            Self::Health => "health",
        }
    }
}

impl OplogCreateNamespaceClass {
    const fn token(self) -> &'static str {
        match self {
            Self::Unsafe => "unsafe",
            Self::IdentityChanged => "identity_changed",
            Self::Io => "io",
        }
    }
}

impl OplogCreateLockClass {
    const fn token(self) -> &'static str {
        match self {
            Self::Unsafe => "unsafe",
            Self::IdentityChanged => "identity_changed",
            Self::Busy => "busy",
            Self::Io => "io",
        }
    }
}

impl OplogPublishReason {
    const fn token(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Reconciliation => "reconciliation",
            Self::DestinationInspection => "destination_inspection",
            Self::Alias => "alias",
            Self::DestinationExhaustion => "destination_exhaustion",
            Self::DirectorySync => "directory_sync",
            Self::FinalBinding => "final_binding",
            Self::AncestorRevalidation => "ancestor_revalidation",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OplogIdentityObservation {
    OwnNoncanonical(OplogVerifiedAt),
    OwnLanded(OplogVerifiedAt),
    ForeignLanded(OplogVerifiedAt),
    OwnMultipleLinks {
        nlink: u64,
        verified_at: OplogVerifiedAt,
    },
    NoVerifiedLeaf {
        nlink: u64,
        verified_at: OplogVerifiedAt,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OplogVerifiedAt {
    native_leaf: Option<OsString>,
    checkpoint: OplogEvidenceCheckpoint,
}

impl OplogVerifiedAt {
    pub(super) fn new(native_leaf: Option<OsString>, checkpoint: OplogEvidenceCheckpoint) -> Self {
        Self {
            native_leaf,
            checkpoint,
        }
    }

    pub fn native_leaf(&self) -> Option<&OsStr> {
        self.native_leaf.as_deref()
    }

    pub fn checkpoint(&self) -> OplogEvidenceCheckpoint {
        self.checkpoint
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogEvidenceCheckpoint {
    Stage,
    Admission,
    Lease,
    AncestorRevalidation,
    Rename { ordinal: u8 },
    AfterForeignCollision { ordinal: u8 },
    AfterRename,
    DirectorySync,
    RetainedHandle,
    FinalBinding,
    FinalFailureClassification,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OplogCollisionRecord {
    ordinal: u8,
    dest: OsString,
    occupant: OplogCollisionOccupant,
}

impl OplogCollisionRecord {
    pub(super) fn new(ordinal: u8, dest: OsString, occupant: OplogCollisionOccupant) -> Self {
        Self {
            ordinal,
            dest,
            occupant,
        }
    }

    pub fn ordinal(&self) -> u8 {
        self.ordinal
    }

    pub fn dest(&self) -> &OsStr {
        &self.dest
    }

    pub fn occupant(&self) -> &OplogCollisionOccupant {
        &self.occupant
    }

    pub(super) fn set_occupant(&mut self, occupant: OplogCollisionOccupant) {
        self.occupant = occupant;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OplogCollisionOccupant {
    Foreign {
        identity: OplogFileIdentity,
        verified_at: OplogVerifiedAt,
    },
    Replaced,
    Absent,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OplogFileIdentity {
    #[cfg(unix)]
    pub(super) dev: u64,
    #[cfg(unix)]
    pub(super) ino: u64,
    #[cfg(windows)]
    pub(super) volume_serial: u64,
    #[cfg(windows)]
    pub(super) file_id: [u8; 16],
}

impl OplogFileIdentity {
    #[cfg(unix)]
    pub(super) const fn from_unix(dev: u64, ino: u64) -> Self {
        Self { dev, ino }
    }

    #[cfg(windows)]
    pub(super) const fn from_windows(volume_serial: u64, file_id: [u8; 16]) -> Self {
        Self {
            volume_serial,
            file_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OplogObservationGap {
    location: OplogEvidenceCheckpoint,
    cause: OplogGapCause,
}

impl OplogObservationGap {
    pub(super) const fn new(location: OplogEvidenceCheckpoint, cause: OplogGapCause) -> Self {
        Self { location, cause }
    }

    pub fn location(&self) -> OplogEvidenceCheckpoint {
        self.location
    }

    pub fn cause(&self) -> OplogGapCause {
        self.cause
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogGapCause {
    Io,
    UnobservableHandle,
    Changed,
    Inconsistent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainedNamespaceState {
    NotEstablished,
    Established(OplogNamespaceIdentity),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OplogNamespaceIdentity(ObjectIdentity);

impl OplogNamespaceIdentity {
    pub(super) const fn new(inner: ObjectIdentity) -> Self {
        Self(inner)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct OplogCreateError {
    reason: OplogCreateReason,
    namespace: RetainedNamespaceState,
    observations: Vec<OplogIdentityObservation>,
    collisions: Vec<OplogCollisionRecord>,
    gaps: Vec<OplogObservationGap>,
}

impl OplogCreateError {
    pub fn reason(&self) -> OplogCreateReason {
        self.reason
    }

    pub fn namespace(&self) -> RetainedNamespaceState {
        self.namespace
    }

    pub fn observations(&self) -> &[OplogIdentityObservation] {
        &self.observations
    }

    pub fn collisions(&self) -> &[OplogCollisionRecord] {
        &self.collisions
    }

    pub fn gaps(&self) -> &[OplogObservationGap] {
        &self.gaps
    }

    fn token(&self) -> String {
        self.reason.token()
    }
}

impl fmt::Display for OplogCreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.token())
    }
}

impl fmt::Debug for OplogCreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for OplogCreateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(super) struct OplogCreateEvidence {
    namespace: RetainedNamespaceState,
    observations: Vec<OplogIdentityObservation>,
    collisions: Vec<OplogCollisionRecord>,
    gaps: Vec<OplogObservationGap>,
}

impl OplogCreateEvidence {
    pub(super) fn not_established() -> Self {
        Self {
            namespace: RetainedNamespaceState::NotEstablished,
            observations: Vec::new(),
            collisions: Vec::new(),
            gaps: Vec::new(),
        }
    }

    pub(super) fn established(identity: OplogNamespaceIdentity) -> Self {
        Self {
            namespace: RetainedNamespaceState::Established(identity),
            observations: Vec::new(),
            collisions: Vec::new(),
            gaps: Vec::new(),
        }
    }

    pub(super) fn observe(&mut self, observation: OplogIdentityObservation) {
        self.observations.push(observation);
    }

    pub(super) fn collision(&mut self, record: OplogCollisionRecord) {
        self.collisions.push(record);
    }

    pub(super) fn collisions_mut(&mut self) -> &mut [OplogCollisionRecord] {
        &mut self.collisions
    }

    pub(super) fn observations(&self) -> &[OplogIdentityObservation] {
        &self.observations
    }

    pub(super) fn gap(&mut self, location: OplogEvidenceCheckpoint, cause: OplogGapCause) {
        self.gaps.push(OplogObservationGap::new(location, cause));
    }

    pub(super) fn fail(self, reason: OplogCreateReason) -> OplogCreateError {
        OplogCreateError {
            reason,
            namespace: self.namespace,
            observations: self.observations,
            collisions: self.collisions,
            gaps: self.gaps,
        }
    }
}

pub(super) enum NamedOccupant {
    Absent,
    Regular {
        identity: OplogFileIdentity,
        nlink: u64,
    },
    Other,
}

pub(super) enum StageError {
    Allocate,
    Leftover(OsString),
}
