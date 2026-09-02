// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Error types for journal file-I/O primitives.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crate::journal_root::JournalEntryKind;
use crate::name_admission::{ClaimName, NameAdmissionReason};

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
    /// Publication landed, but post-move observation could not be proven.
    #[cfg(windows)]
    PublicationUncertain {
        path: PathBuf,
        operation: &'static str,
        source: io::Error,
    },
}

impl fmt::Display for AtomicWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            #[cfg(windows)]
            Self::PublicationUncertain {
                path,
                operation,
                source,
            } => write!(formatter, "{}: {operation}: {source}", path.display()),
        }
    }
}

impl Error for AtomicWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            #[cfg(windows)]
            Self::PublicationUncertain { source, .. } => Some(source),
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug)]
struct ExclusiveCleanupChain {
    primary: io::Error,
    stage: OsString,
    cleanup: io::Error,
}

impl fmt::Display for ExclusiveCleanupChain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}; could not remove stage {:?}: {}",
            self.primary, self.stage, self.cleanup
        )
    }
}

impl Error for ExclusiveCleanupChain {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.primary)
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn compose_exclusive_cleanup(
    primary: io::Error,
    stage: impl AsRef<OsStr>,
    cleanup: io::Error,
) -> io::Error {
    io::Error::new(
        primary.kind(),
        ExclusiveCleanupChain {
            primary,
            stage: stage.as_ref().to_os_string(),
            cleanup,
        },
    )
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

/// Failure while acquiring, listing, or reading a descriptor-bound flat directory.
#[derive(Debug)]
pub enum FlatDirectoryError {
    /// A descendant path was not a nonempty sequence of normal components.
    InvalidRelativePath { path: PathBuf, reason: &'static str },
    /// A direct entry name did not satisfy portable-component admission.
    InvalidName {
        name: OsString,
        reason: NameAdmissionReason,
    },
    /// A requested descendant was not a directory.
    NotDirectory { path: PathBuf },
    /// A symlink was refused while descending to a directory.
    SymlinkRefused { path: PathBuf },
    /// An entry that must be read as a regular file was another kind.
    NotRegular { path: PathBuf },
    /// An observed regular file exceeded the caller-supplied byte limit.
    SizeLimitExceeded {
        path: PathBuf,
        kind: JournalEntryKind,
        size: u64,
        limit: usize,
    },
    /// A retained directory or observed entry changed while it was being checked.
    IdentityChanged { path: PathBuf },
    /// A directory entry disappeared while an all-or-nothing listing was built.
    EnumerationChanged { path: PathBuf },
    /// A filesystem operation failed.
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for FlatDirectoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRelativePath { path, reason } => {
                write!(
                    formatter,
                    "invalid flat-directory path {}: {reason}",
                    path.display()
                )
            }
            Self::InvalidName { name, reason } => {
                write!(formatter, "invalid flat-directory entry {name:?}: {reason}")
            }
            Self::NotDirectory { path } => {
                write!(
                    formatter,
                    "flat-directory descendant is not a directory: {}",
                    path.display()
                )
            }
            Self::SymlinkRefused { path } => {
                write!(
                    formatter,
                    "flat-directory descendant is a symlink: {}",
                    path.display()
                )
            }
            Self::NotRegular { path } => {
                write!(
                    formatter,
                    "flat-directory entry is not a regular file: {}",
                    path.display()
                )
            }
            Self::SizeLimitExceeded {
                path,
                kind,
                size,
                limit,
            } => write!(
                formatter,
                "flat-directory entry exceeds observed-read limit: {} is {kind:?}, {size} bytes exceeds {limit}",
                path.display()
            ),
            Self::IdentityChanged { path } => {
                write!(
                    formatter,
                    "flat-directory identity changed: {}",
                    path.display()
                )
            }
            Self::EnumerationChanged { path } => {
                write!(
                    formatter,
                    "flat-directory entry vanished while listing: {}",
                    path.display()
                )
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} failed for {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for FlatDirectoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidRelativePath { .. }
            | Self::InvalidName { .. }
            | Self::NotDirectory { .. }
            | Self::SymlinkRefused { .. }
            | Self::NotRegular { .. }
            | Self::SizeLimitExceeded { .. }
            | Self::IdentityChanged { .. }
            | Self::EnumerationChanged { .. } => None,
        }
    }
}

/// The platform primitive required to claim a name without replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoReplacePrimitive {
    /// Linux `renameat2(2)` with `RENAME_NOREPLACE`.
    LinuxRenameAt2,
    /// macOS `renameatx_np(2)` with `RENAME_EXCL`.
    MacosRenameAtxNp,
    /// Another Unix platform with no supported no-replace rename primitive.
    UnsupportedUnix,
}

/// Why a claim operation proved it did not mutate either supplied name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimUnchangedReason {
    /// The caller-supplied claim entry already existed.
    ClaimNameOccupied,
    /// The filesystem cannot perform the required no-replace rename primitive.
    UnsupportedNoReplace { primitive: NoReplacePrimitive },
    /// Reconciliation proved an ambiguous rename error did not apply the rename.
    RenameNotAppliedAfterReconciliation,
}

/// The known disposition after the observed object changed during removal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityChangeDisposition {
    /// The changed claimed entry was restored to its original name.
    Restored,
    /// Restoration refused to overwrite a newly occupied original name.
    RetainedClaim { claim: ClaimName },
    /// The observed object no longer has a safely known location.
    UnknownLocation,
}

/// Durability evidence associated with an identity-change outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimDurability {
    /// No known local namespace transition was finalized and synced.
    NotEstablished,
    /// The known namespace transition was directory-synced.
    Synced,
    /// A directory sync was attempted but failed.
    Uncertain,
}

/// Successful or explicitly non-destructive result of claimed removal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimRemovalOutcome {
    /// The exact observed object was unlinked and the directory was synced.
    Removed,
    /// The exact observed object was unlinked, but directory sync failed.
    RemovedDurabilityUncertain,
    /// This invocation proved it did not mutate either supplied name.
    Unchanged { reason: ClaimUnchangedReason },
    /// The observed object changed, was restored, or has an unknown location.
    IdentityChanged {
        /// The proven disposition of the observed object.
        disposition: IdentityChangeDisposition,
        /// Durability evidence for that disposition.
        durability: ClaimDurability,
    },
}

/// Failure while claiming or removing one observed file entry.
#[derive(Debug)]
pub enum ClaimRemovalError {
    /// The original name was not a portable component.
    InvalidOriginalName {
        name: OsString,
        reason: NameAdmissionReason,
    },
    /// The supplied original name differs from the observation's name.
    ObservationNameMismatch {
        original: OsString,
        observed: OsString,
    },
    /// The caller-supplied claim name is already an alias of the original entry.
    AliasedClaimName {
        original: OsString,
        claim: ClaimName,
        device: u64,
        inode: u64,
    },
    /// Inspection of the claimed entry failed after a successful claim.
    PostClaimInspection {
        claim: ClaimName,
        source: FlatDirectoryError,
    },
    /// Inspection before claim or while restoring failed.
    Preflight { source: FlatDirectoryError },
    /// Inspection required to reconcile an ambiguous rename failed.
    Reconciliation {
        original: OsString,
        claim: ClaimName,
        source: FlatDirectoryError,
    },
    /// Both names matched during reconciliation, so no safe conclusion was possible.
    ReconciliationInconclusive {
        original: OsString,
        claim: ClaimName,
    },
    /// A restore operation failed while retaining the claim name for the caller.
    RestoreFailure { claim: ClaimName, source: io::Error },
    /// Unlinking the successfully claimed name failed.
    UnlinkFailure { claim: ClaimName, source: io::Error },
    /// A filesystem operation failed.
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for ClaimRemovalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOriginalName { name, reason } => {
                write!(formatter, "invalid original entry {name:?}: {reason}")
            }
            Self::ObservationNameMismatch { original, observed } => write!(
                formatter,
                "original entry {original:?} does not match observation name {observed:?}"
            ),
            Self::AliasedClaimName {
                original,
                claim,
                device,
                inode,
            } => write!(
                formatter,
                "claim {claim:?} aliases original {original:?} at ({device}, {inode})"
            ),
            Self::PostClaimInspection { claim, source } => {
                write!(
                    formatter,
                    "could not inspect retained claim {claim:?}: {source}"
                )
            }
            Self::Preflight { source } => {
                write!(formatter, "could not preflight claimed removal: {source}")
            }
            Self::Reconciliation {
                original,
                claim,
                source,
            } => write!(
                formatter,
                "could not reconcile original {original:?} and claim {claim:?}: {source}"
            ),
            Self::ReconciliationInconclusive { original, claim } => write!(
                formatter,
                "cannot reconcile original {original:?} and claim {claim:?} safely"
            ),
            Self::RestoreFailure { claim, source } => {
                write!(
                    formatter,
                    "could not restore retained claim {claim:?}: {source}"
                )
            }
            Self::UnlinkFailure { claim, source } => {
                write!(
                    formatter,
                    "could not unlink claimed entry {claim:?}: {source}"
                )
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} failed for {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ClaimRemovalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::PostClaimInspection { source, .. }
            | Self::Preflight { source }
            | Self::Reconciliation { source, .. } => Some(source),
            Self::RestoreFailure { source, .. }
            | Self::UnlinkFailure { source, .. }
            | Self::Io { source, .. } => Some(source),
            Self::InvalidOriginalName { .. }
            | Self::ObservationNameMismatch { .. }
            | Self::AliasedClaimName { .. }
            | Self::ReconciliationInconclusive { .. } => None,
        }
    }
}

#[cfg(test)]
mod exclusive_cleanup_tests {
    use super::{AtomicWriteError, compose_exclusive_cleanup};
    use std::error::Error;
    use std::ffi::OsString;
    use std::io;
    use std::path::PathBuf;

    #[test]
    fn compose_preserves_already_exists_and_appends_cleanup() {
        let primary = io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists");
        let cleanup = io::Error::other("cleanup boom");
        let stage = OsString::from(".tmp_1_2.tmp");
        let composed = compose_exclusive_cleanup(primary, &stage, cleanup);
        assert_eq!(composed.kind(), io::ErrorKind::AlreadyExists);
        let display = composed.to_string();
        assert!(display.contains("destination already exists"), "{display}");
        assert!(
            display.contains("could not remove stage \".tmp_1_2.tmp\": cleanup boom"),
            "{display}"
        );
        let chain = composed
            .get_ref()
            .and_then(|error| error.downcast_ref::<super::ExclusiveCleanupChain>())
            .expect("ExclusiveCleanupChain");
        assert_eq!(chain.primary.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(chain.primary.to_string(), "destination already exists");
        assert!(
            Error::source(chain)
                .is_some_and(|source| source.to_string() == "destination already exists"),
            "Error::source must be the original primary"
        );

        let wrapped = AtomicWriteError::Io {
            path: PathBuf::from("dest.bin"),
            source: compose_exclusive_cleanup(
                io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
                &stage,
                io::Error::other("cleanup boom"),
            ),
        };
        match wrapped {
            AtomicWriteError::Io { source, .. } => {
                assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
            }
            #[cfg(windows)]
            AtomicWriteError::PublicationUncertain { .. } => {
                panic!("cleanup composition stays on Io")
            }
        }
    }
}
