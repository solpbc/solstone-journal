// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Crash-safe whole-file writers.

use std::ffi::{OsStr, OsString};
#[cfg(any(unix, windows))]
use std::fs;
use std::fs::File;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(any(unix, windows))]
use std::io::Read;
#[cfg(unix)]
use std::io::Write;
use std::io::{self};
#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
#[cfg(unix)]
use std::sync::atomic::Ordering;
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(any(unix, windows))]
use serde::Serialize;

#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::fcntl::{AT_FDCWD, AtFlags, OFlag, openat, renameat};
#[cfg(unix)]
use nix::sys::stat::{Mode, SFlag, fstat, fstatat};
#[cfg(unix)]
use nix::unistd::{UnlinkatFlags, fsync, linkat, unlinkat};

#[cfg(any(unix, windows))]
use crate::errors::AtomicWriteError;
#[cfg(unix)]
use crate::flat_directory::entry_from_stat;
#[cfg(any(unix, windows))]
use crate::observation::FileObservation;
#[cfg(unix)]
use crate::observation::{same_entry_metadata, same_observation};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Ordered checkpoints used by bound-publication fault and pause tests.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundPublicationPrimitive {
    /// The exclusive stage-file creation.
    TempCreate,
    /// Writing the replacement bytes into the stage file.
    Write,
    /// Syncing the completed stage file.
    FileSync,
    /// Renaming the stage into the destination name.
    Rename,
    /// Syncing the bound parent directory after publication.
    ParentSync,
    /// Removing a stage name after it has been linked into its final name.
    StageUnlink,
    /// Create-only `linkat` of the filled stage into the destination name.
    ExclusivePublish,
    /// First no-follow identity and byte observation after exclusive publication.
    PostPublishObserve,
    /// Completion-boundary no-follow identity and byte observation.
    CompletionObserve,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
struct BoundPublicationTraceState {
    attempted: Vec<BoundPublicationPrimitive>,
    faults: Vec<BoundPublicationFault>,
    faults_consumed: usize,
    barriers: Vec<BoundPublicationBarrier>,
    barriers_fired: usize,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
struct BoundPublicationFault {
    primitive: BoundPublicationPrimitive,
    ordinal: usize,
    error: Errno,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
struct BoundPublicationBarrier {
    primitive: BoundPublicationPrimitive,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
thread_local! {
    static BOUND_PUBLICATION_TRACE: std::cell::RefCell<Option<BoundPublicationTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Run `op` with one injected errno at an ordinal bound-publication checkpoint.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_bound_publication_fault<T>(
    primitive: BoundPublicationPrimitive,
    ordinal: usize,
    raw_errno: i32,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    BOUND_PUBLICATION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "bound publication trace is already active"
        );
        *trace.borrow_mut() = Some(BoundPublicationTraceState {
            attempted: Vec::new(),
            faults: vec![BoundPublicationFault {
                primitive,
                ordinal,
                error: Errno::from_raw(raw_errno),
            }],
            faults_consumed: 0,
            barriers: Vec::new(),
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = BOUND_PUBLICATION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("bound publication trace remains active")
    });
    (result, state.faults_consumed == 1)
}

/// Run `op` with one deterministic bound-publication barrier callback.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_bound_publication_barrier<T>(
    primitive: BoundPublicationPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    BOUND_PUBLICATION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "bound publication trace is already active"
        );
        *trace.borrow_mut() = Some(BoundPublicationTraceState {
            attempted: Vec::new(),
            faults: Vec::new(),
            faults_consumed: 0,
            barriers: vec![BoundPublicationBarrier {
                primitive,
                ordinal,
                callback: Box::new(callback),
            }],
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = BOUND_PUBLICATION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("bound publication trace remains active")
    });
    (result, state.barriers_fired == 1)
}

/// Run `op` with two deterministic bound-publication barrier callbacks.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_two_bound_publication_barriers<T>(
    first_primitive: BoundPublicationPrimitive,
    first_ordinal: usize,
    first_callback: impl FnOnce() + 'static,
    second_primitive: BoundPublicationPrimitive,
    second_ordinal: usize,
    second_callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, usize) {
    BOUND_PUBLICATION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "bound publication trace is already active"
        );
        *trace.borrow_mut() = Some(BoundPublicationTraceState {
            attempted: Vec::new(),
            faults: Vec::new(),
            faults_consumed: 0,
            barriers: vec![
                BoundPublicationBarrier {
                    primitive: first_primitive,
                    ordinal: first_ordinal,
                    callback: Box::new(first_callback),
                },
                BoundPublicationBarrier {
                    primitive: second_primitive,
                    ordinal: second_ordinal,
                    callback: Box::new(second_callback),
                },
            ],
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = BOUND_PUBLICATION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("bound publication trace remains active")
    });
    (result, state.barriers_fired)
}

/// Run `op` with one or more injected errnos at bound-publication checkpoints.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_bound_publication_faults<T>(
    faults: impl IntoIterator<Item = (BoundPublicationPrimitive, usize, i32)>,
    op: impl FnOnce() -> T,
) -> (T, usize) {
    BOUND_PUBLICATION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "bound publication trace is already active"
        );
        *trace.borrow_mut() = Some(BoundPublicationTraceState {
            attempted: Vec::new(),
            faults: faults
                .into_iter()
                .map(|(primitive, ordinal, raw_errno)| BoundPublicationFault {
                    primitive,
                    ordinal,
                    error: Errno::from_raw(raw_errno),
                })
                .collect(),
            faults_consumed: 0,
            barriers: Vec::new(),
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = BOUND_PUBLICATION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("bound publication trace remains active")
    });
    (result, state.faults_consumed)
}

/// Run `op` with injected bound-publication faults and one barrier callback.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_bound_publication_faults_and_barrier<T>(
    faults: impl IntoIterator<Item = (BoundPublicationPrimitive, usize, i32)>,
    barrier_primitive: BoundPublicationPrimitive,
    barrier_ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, usize, bool) {
    BOUND_PUBLICATION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "bound publication trace is already active"
        );
        *trace.borrow_mut() = Some(BoundPublicationTraceState {
            attempted: Vec::new(),
            faults: faults
                .into_iter()
                .map(|(primitive, ordinal, raw_errno)| BoundPublicationFault {
                    primitive,
                    ordinal,
                    error: Errno::from_raw(raw_errno),
                })
                .collect(),
            faults_consumed: 0,
            barriers: vec![BoundPublicationBarrier {
                primitive: barrier_primitive,
                ordinal: barrier_ordinal,
                callback: Box::new(callback),
            }],
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = BOUND_PUBLICATION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("bound publication trace remains active")
    });
    (result, state.faults_consumed, state.barriers_fired == 1)
}

/// Successful publication states returned by [`atomic_replace_detailed`].
#[derive(Debug)]
pub enum DetailedAtomicOutcome {
    /// The caller-visible path was reverified and its directory was synced.
    Published,
    /// Bytes were published, but syncing the bound directory failed.
    PublishedDurabilityUncertain { source: io::Error },
    /// Bytes were published in the inspected directory, but its pathname changed.
    PublishedParentPathRaced { sync_error: Option<io::Error> },
    /// Bytes were published, but the final pathname observation itself failed.
    PublishedParentPathUnverified {
        observation: io::Error,
        sync_error: Option<io::Error>,
    },
}

/// Successful publication states returned by [`atomic_replace_bound`].
///
/// Pathname-identity outcomes stay on [`DetailedAtomicOutcome`]. A bound
/// caller cannot observe a parent pathname.
#[cfg(unix)]
#[derive(Debug)]
pub enum BoundAtomicOutcome {
    /// Rename landed in the bound directory and the directory was synced.
    Published { observation: FileObservation },
    /// Rename landed; syncing the bound directory failed.
    PublishedDurabilityUncertain {
        observation: FileObservation,
        source: io::Error,
    },
    /// Rename landed, but the destination no longer named the published
    /// observation when publication completed.
    PublishedObservationUncertain {
        observation: FileObservation,
        source: io::Error,
        durability_source: Option<io::Error>,
    },
}

#[cfg(unix)]
struct BoundPublicationResult {
    observation: FileObservation,
    durability_source: Option<io::Error>,
    observation_source: Option<io::Error>,
}

/// A failure before publication. The prior destination is preserved.
#[derive(Debug)]
pub struct DetailedAtomicError {
    pub path: PathBuf,
    pub operation: &'static str,
    pub source: io::Error,
    /// A stage is named only when cleanup also failed.
    pub orphan_stage: Option<OsString>,
    pub cleanup_error: Option<io::Error>,
}

impl std::fmt::Display for DetailedAtomicError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: {}: {}",
            self.path.display(),
            self.operation,
            self.source
        )?;
        if let (Some(stage), Some(cleanup)) = (&self.orphan_stage, &self.cleanup_error) {
            write!(formatter, "; could not remove stage {stage:?}: {cleanup}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DetailedAtomicError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Three independently inspectable facts after a create-only publish.
///
/// On Windows, [`MetadataDurability::Confirmed`] is not reachable: there is no
/// documented directory-entry metadata flush on the retained parent handle
/// (`FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE`, plus
/// `FILE_TRAVERSE` on the publication-path capability) — neither includes
/// `GENERIC_WRITE`/`FILE_WRITE_DATA`, which `FlushFileBuffers` requires. This
/// field is therefore always [`MetadataDurability::Unproven`] with
/// `source: None` on Windows.
#[cfg(any(unix, windows))]
#[derive(Debug)]
pub struct ExclusivePublication {
    pub bytes_written: u64,
    pub final_name: FinalNameConfirmation,
    pub durability: MetadataDurability,
    pub cleanup: StageCleanup,
}

#[cfg(any(unix, windows))]
impl ExclusivePublication {
    /// True only when all three facts are the success state. Derived; never stored.
    /// Unreachable on Windows (durability is always Unproven there).
    pub fn is_fully_confirmed(&self) -> bool {
        matches!(self.final_name, FinalNameConfirmation::Confirmed { .. })
            && matches!(self.durability, MetadataDurability::Confirmed)
            && matches!(self.cleanup, StageCleanup::Removed)
    }
}

#[cfg(any(unix, windows))]
#[derive(Debug)]
pub enum FinalNameConfirmation {
    Confirmed {
        observation: FileObservation,
    },
    Unverified {
        destination: PathBuf,
        reason: io::Error,
    },
}

#[cfg(any(unix, windows))]
#[derive(Debug)]
pub enum MetadataDurability {
    Confirmed,
    /// `Some` when a durability syscall ran and failed (Unix `fsync` on the bound parent).
    /// `None` when the platform has no documented primitive (Windows — always this case).
    Unproven {
        source: Option<io::Error>,
    },
}

#[cfg(any(unix, windows))]
#[derive(Debug)]
pub enum StageCleanup {
    Removed,
    Retained { stage: OsString, source: io::Error },
}

#[cfg(any(unix, windows))]
fn select_exclusive_final_name(
    destination: PathBuf,
    first: Result<FileObservation, io::Error>,
    second: Result<FileObservation, io::Error>,
) -> FinalNameConfirmation {
    match (first, second) {
        (Ok(_), Ok(observation)) => FinalNameConfirmation::Confirmed { observation },
        (_, Err(reason)) => FinalNameConfirmation::Unverified {
            destination,
            reason,
        },
        (Err(reason), Ok(_)) => FinalNameConfirmation::Unverified {
            destination,
            reason,
        },
    }
}

/// Options shared by the byte-oriented whole-file writers.
#[cfg(any(unix, windows))]
#[derive(Debug, Clone, Copy, Default)]
pub struct AtomicWriteOptions {
    /// Final file mode, applied before the rename or hard-link publication
    /// on Unix. On Windows this is validated (must be `<= 0o777`) but
    /// otherwise inert — there is no file mode to apply.
    pub mode: Option<u32>,
}

/// JSON formatting and publication options.
#[cfg(any(unix, windows))]
#[derive(Debug, Clone, Copy)]
pub struct JsonWriteOptions {
    /// Final file mode, applied before publication on Unix. On Windows this
    /// is validated (must be `<= 0o777`) but otherwise inert.
    pub mode: Option<u32>,
    /// Pretty-print indentation width. `None` emits compact JSON.
    pub indent: Option<usize>,
    /// Whether object keys are recursively sorted before serialization.
    pub sort_keys: bool,
}

#[cfg(any(unix, windows))]
impl Default for JsonWriteOptions {
    fn default() -> Self {
        Self {
            mode: None,
            indent: Some(2),
            sort_keys: false,
        }
    }
}

/// Atomically replace a regular destination beneath an existing real parent.
///
/// The entire operation is bound to the inspected directory descriptor. This
/// function never creates a parent and never follows a parent or destination
/// symlink. Callers must serialize all writers for `path` with the stable lock.
#[cfg(unix)]
pub fn atomic_replace_detailed(
    path: &Path,
    contents: &[u8],
    mode: u32,
) -> Result<DetailedAtomicOutcome, DetailedAtomicError> {
    if mode > 0o777 {
        return Err(detailed_error(
            path,
            "validate mode",
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    let parent = path
        .parent()
        .filter(|item| !item.as_os_str().is_empty())
        .ok_or_else(|| {
            detailed_error(
                path,
                "validate destination",
                io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"),
            )
        })?;
    let name = normal_name(path.file_name()).ok_or_else(|| {
        detailed_error(
            path,
            "validate destination",
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        )
    })?;
    let inspected =
        stat_parent(parent).map_err(|source| detailed_error(path, "inspect parent", source))?;
    let directory = openat(
        AT_FDCWD,
        parent,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|source| detailed_errno(path, "open parent", source))?;
    let opened = fstat(&directory)
        .map_err(|source| detailed_errno(path, "inspect opened parent", source))?;
    if !same_identity(inspected, opened) {
        return Err(detailed_error(
            path,
            "bind parent",
            io::Error::other("parent pathname changed"),
        ));
    }
    let before_rename = match stat_parent(parent) {
        Ok(status) if same_identity(inspected, status) => Ok(()),
        Ok(_) => Err(io::Error::other(
            "parent pathname changed before publication",
        )),
        Err(source) => Err(source),
    };
    if let Err(source) = before_rename {
        return Err(detailed_error(
            path,
            "reverify parent before publication",
            source,
        ));
    }

    let BoundPublicationResult {
        durability_source: sync_error,
        observation_source,
        ..
    } = publish_into_bound_directory(&directory, name, contents, mode).map_err(|error| {
        DetailedAtomicError {
            path: path.to_path_buf(),
            operation: error.operation,
            source: error.source,
            orphan_stage: error.orphan_stage,
            cleanup_error: error.cleanup_error,
        }
    })?;

    if let Some(observation) = observation_source {
        return Ok(DetailedAtomicOutcome::PublishedParentPathUnverified {
            observation,
            sync_error,
        });
    }

    let final_observation = stat_parent(parent);
    match final_observation {
        Ok(status) if same_identity(inspected, status) => match sync_error {
            None => Ok(DetailedAtomicOutcome::Published),
            Some(source) => Ok(DetailedAtomicOutcome::PublishedDurabilityUncertain { source }),
        },
        Ok(_) => Ok(DetailedAtomicOutcome::PublishedParentPathRaced { sync_error }),
        Err(observation) => Ok(DetailedAtomicOutcome::PublishedParentPathUnverified {
            observation,
            sync_error,
        }),
    }
}

#[cfg(windows)]
pub fn atomic_replace_detailed(
    path: &Path,
    contents: &[u8],
    mode: u32,
) -> Result<DetailedAtomicOutcome, DetailedAtomicError> {
    windows_atomic::atomic_replace_detailed(path, contents, mode)
}

/// Failure before a capability-bound Windows publication can begin.
#[cfg(windows)]
#[allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]
#[derive(Debug)]
pub(crate) enum BoundAtomicPublishError {
    /// The retained alias parent or its fresh pathname binding was no longer authorized.
    NamespaceChanged,
    /// Ordinary staging/publication failure before an outcome exists.
    Atomic(DetailedAtomicError),
}

#[cfg(windows)]
impl std::fmt::Display for BoundAtomicPublishError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NamespaceChanged => {
                formatter.write_str("bound publication parent namespace changed")
            }
            Self::Atomic(error) => error.fmt(formatter),
        }
    }
}

#[cfg(windows)]
impl std::error::Error for BoundAtomicPublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NamespaceChanged => None,
            Self::Atomic(error) => Some(error),
        }
    }
}

/// Publish through the held lock's retained parent authority.
#[cfg(windows)]
#[allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]
pub(crate) fn atomic_replace_detailed_bound(
    parent: &crate::windows_sync_dir::WindowsFlatDirectory,
    lock: &crate::locking::BoundParentLock,
    destination_name: &OsStr,
    contents: &[u8],
    mode: u32,
) -> Result<DetailedAtomicOutcome, BoundAtomicPublishError> {
    windows_atomic::atomic_replace_detailed_bound(
        parent,
        lock.parent_identity(),
        destination_name,
        contents,
        mode,
    )
}

/// An initial construction must distinguish every landed-but-unverified result.
#[cfg(windows)]
#[allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]
#[derive(Debug)]
pub(crate) enum StrictManagedLogPublication {
    Published,
    Outcome(DetailedAtomicOutcome),
}

/// Apply the strict initial-publication policy without deleting or repairing outcomes.
#[cfg(windows)]
#[allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]
pub(crate) fn require_strict_managed_log_publication(
    result: Result<DetailedAtomicOutcome, BoundAtomicPublishError>,
) -> Result<StrictManagedLogPublication, BoundAtomicPublishError> {
    match result? {
        DetailedAtomicOutcome::Published => Ok(StrictManagedLogPublication::Published),
        outcome => Ok(StrictManagedLogPublication::Outcome(outcome)),
    }
}

#[cfg(all(test, windows))]
mod managed_log_windows_tests {
    use super::*;

    #[test]
    fn strict_initial_publication_preserves_every_nonexact_outcome() {
        assert!(matches!(
            require_strict_managed_log_publication(Ok(DetailedAtomicOutcome::Published)),
            Ok(StrictManagedLogPublication::Published)
        ));
        assert!(matches!(
            require_strict_managed_log_publication(Ok(
                DetailedAtomicOutcome::PublishedParentPathRaced { sync_error: None }
            )),
            Ok(StrictManagedLogPublication::Outcome(
                DetailedAtomicOutcome::PublishedParentPathRaced { .. }
            ))
        ));
        assert!(matches!(
            require_strict_managed_log_publication(Ok(
                DetailedAtomicOutcome::PublishedDurabilityUncertain {
                    source: io::Error::other("durability")
                }
            )),
            Ok(StrictManagedLogPublication::Outcome(
                DetailedAtomicOutcome::PublishedDurabilityUncertain { .. }
            ))
        ));
        assert!(matches!(
            require_strict_managed_log_publication(Ok(
                DetailedAtomicOutcome::PublishedParentPathUnverified {
                    observation: io::Error::other("observation"),
                    sync_error: None,
                }
            )),
            Ok(StrictManagedLogPublication::Outcome(
                DetailedAtomicOutcome::PublishedParentPathUnverified { .. }
            ))
        ));
    }
}

#[cfg(windows)]
#[path = "windows_atomic.rs"]
mod windows_atomic;
#[cfg(windows)]
#[path = "windows_create_only.rs"]
mod windows_create_only;
#[cfg(all(windows, feature = "test-hooks"))]
pub use windows_create_only::{
    WindowsCreateOnlyPrimitive, WindowsCreateOnlyTrace, run_with_windows_create_only_barrier,
    run_with_windows_create_only_faults, run_with_windows_create_only_faults_and_barrier,
};

#[cfg(all(windows, feature = "test-hooks"))]
pub use windows_atomic::run_with_windows_detailed_atomic_faults_and_two_barriers;
#[cfg(all(windows, feature = "test-hooks"))]
pub use windows_atomic::{
    run_with_windows_detailed_atomic_backoffs, run_with_windows_detailed_atomic_barrier,
    run_with_windows_detailed_atomic_faults, run_with_windows_detailed_atomic_faults_and_barrier,
};

/// Atomically replace a regular destination beneath an already-bound parent.
///
/// The parent is the caller-supplied directory descriptor. This never opens a
/// parent via `AT_FDCWD` and never treats a stored pathname as source authority.
#[cfg(unix)]
pub fn atomic_replace_bound(
    directory: &impl AsFd,
    name: &OsStr,
    contents: &[u8],
    mode: u32,
) -> Result<BoundAtomicOutcome, DetailedAtomicError> {
    let path = Path::new(name);
    if mode > 0o777 {
        return Err(detailed_error(
            path,
            "validate mode",
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    let name = normal_name(Some(name)).ok_or_else(|| {
        detailed_error(
            path,
            "validate destination",
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        )
    })?;
    let result = publish_into_bound_directory(directory, name, contents, mode)?;
    if let Some(source) = result.observation_source {
        return Ok(BoundAtomicOutcome::PublishedObservationUncertain {
            observation: result.observation,
            source,
            durability_source: result.durability_source,
        });
    }
    match result.durability_source {
        None => Ok(BoundAtomicOutcome::Published {
            observation: result.observation,
        }),
        Some(source) => Ok(BoundAtomicOutcome::PublishedDurabilityUncertain {
            observation: result.observation,
            source,
        }),
    }
}

/// Publish `contents` only when `name` does not yet exist in `directory`.
///
/// Contents are written and synced to an exclusive stage inode first, then
/// published with `linkat(2)`. The destination name is never visible with
/// partial content.
#[cfg(unix)]
pub fn write_bytes_exclusive_bound(
    directory: &impl AsFd,
    name: &OsStr,
    contents: &[u8],
    mode: u32,
) -> Result<(), AtomicWriteError> {
    let path = Path::new(name);
    if mode > 0o777 {
        return Err(io_error(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    let name = normal_name(Some(name)).ok_or_else(|| {
        io_error(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        )
    })?;
    match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(_) => {
            return Err(io_error(
                path,
                io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
            ));
        }
        Err(Errno::ENOENT) => {}
        Err(source) => return Err(io_error(path, errno_io(source))),
    }
    let (stage_name, mut stage_file) = allocate_bound_stage(directory, name, path)
        .map_err(|error| io_error(path, error.source))?;
    let prepared = (|| -> io::Result<()> {
        stage_file.write_all(contents)?;
        stage_file.set_permissions(fs::Permissions::from_mode(mode))?;
        sync_file(&stage_file)?;
        Ok(())
    })();
    drop(stage_file);
    if let Err(source) = prepared {
        let _ = unlinkat(
            directory,
            stage_name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        );
        return Err(io_error(path, source));
    }
    if let Err(source) = linkat(
        directory,
        stage_name.as_os_str(),
        directory,
        name,
        AtFlags::empty(),
    ) {
        let _ = unlinkat(
            directory,
            stage_name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        );
        return Err(io_error(path, errno_io(source)));
    }
    let stage_path = Path::new(stage_name.as_os_str());
    checkpoint(BoundPublicationPrimitive::StageUnlink)
        .map_err(|source| io_error(stage_path, source))?;
    unlinkat(
        directory,
        stage_name.as_os_str(),
        UnlinkatFlags::NoRemoveDir,
    )
    .map_err(|source| io_error(stage_path, errno_io(source)))?;
    fsync(directory).map_err(|source| io_error(path, errno_io(source)))?;
    Ok(())
}

/// Publish `contents` only when `path` does not yet exist, reporting each
/// post-publication fact independently.
///
/// Unlike [`write_bytes_exclusive`], this never creates a missing parent. The
/// parent is bound with `openat(2)` and `O_NOFOLLOW`; a missing parent is an
/// error.
#[cfg(unix)]
pub fn write_bytes_exclusive_detailed(
    path: impl AsRef<Path>,
    contents: &[u8],
    options: AtomicWriteOptions,
) -> Result<ExclusivePublication, DetailedAtomicError> {
    let path = path.as_ref();
    if let Some(mode) = options.mode
        && mode > 0o777
    {
        return Err(detailed_error(
            path,
            "validate mode",
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    let (directory, name) = bind_existing_parent(path)?;
    publish_exclusive_into_bound_directory_detailed(
        &directory,
        name,
        &mut io::Cursor::new(contents),
        options.mode,
        Some(contents),
        path,
    )
}

/// Publish a reader into a new dest name, reporting each post-publication fact
/// independently.
///
/// Unlike [`write_reader_exclusive`], this never creates a missing parent. The
/// parent is bound with `openat(2)` and `O_NOFOLLOW`; a missing parent is an
/// error.
#[cfg(unix)]
pub fn write_reader_exclusive_detailed(
    path: &Path,
    reader: &mut impl Read,
    options: AtomicWriteOptions,
) -> Result<ExclusivePublication, DetailedAtomicError> {
    if let Some(mode) = options.mode
        && mode > 0o777
    {
        return Err(detailed_error(
            path,
            "validate mode",
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    let (directory, name) = bind_existing_parent(path)?;
    publish_exclusive_into_bound_directory_detailed(
        &directory,
        name,
        reader,
        options.mode,
        None,
        path,
    )
}

/// Publish `contents` only when `name` does not yet exist in `directory`,
/// reporting each post-publication fact independently.
#[cfg(unix)]
pub fn write_bytes_exclusive_bound_detailed(
    directory: &impl AsFd,
    name: &OsStr,
    contents: &[u8],
    mode: u32,
) -> Result<ExclusivePublication, DetailedAtomicError> {
    let path = Path::new(name);
    if mode > 0o777 {
        return Err(detailed_error(
            path,
            "validate mode",
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    let name = normal_name(Some(name)).ok_or_else(|| {
        detailed_error(
            path,
            "validate destination",
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        )
    })?;
    publish_exclusive_into_bound_directory_detailed(
        directory,
        name,
        &mut io::Cursor::new(contents),
        Some(mode),
        Some(contents),
        path,
    )
}

#[cfg(unix)]
fn bind_existing_parent(path: &Path) -> Result<(File, &OsStr), DetailedAtomicError> {
    let parent = path
        .parent()
        .filter(|item| !item.as_os_str().is_empty())
        .ok_or_else(|| {
            detailed_error(
                path,
                "validate destination",
                io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"),
            )
        })?;
    let name = normal_name(path.file_name()).ok_or_else(|| {
        detailed_error(
            path,
            "validate destination",
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        )
    })?;
    let inspected =
        stat_parent(parent).map_err(|source| detailed_error(path, "inspect parent", source))?;
    let directory = openat(
        AT_FDCWD,
        parent,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|source| detailed_errno(path, "open parent", source))?;
    let opened = fstat(&directory)
        .map_err(|source| detailed_errno(path, "inspect opened parent", source))?;
    if !same_identity(inspected, opened) {
        return Err(detailed_error(
            path,
            "bind parent",
            io::Error::other("parent pathname changed"),
        ));
    }
    let before_publish = match stat_parent(parent) {
        Ok(status) if same_identity(inspected, status) => Ok(()),
        Ok(_) => Err(io::Error::other(
            "parent pathname changed before publication",
        )),
        Err(source) => Err(source),
    };
    if let Err(source) = before_publish {
        return Err(detailed_error(
            path,
            "reverify parent before publication",
            source,
        ));
    }
    Ok((directory, name))
}

#[cfg(unix)]
fn publish_exclusive_into_bound_directory_detailed(
    directory: &impl AsFd,
    name: &OsStr,
    reader: &mut impl Read,
    mode: Option<u32>,
    expected_bytes: Option<&[u8]>,
    error_path: &Path,
) -> Result<ExclusivePublication, DetailedAtomicError> {
    match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(_) => {
            return Err(detailed_error(
                error_path,
                "inspect destination",
                io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
            ));
        }
        Err(Errno::ENOENT) => {}
        Err(source) => {
            return Err(detailed_errno(error_path, "inspect destination", source));
        }
    }
    checkpoint(BoundPublicationPrimitive::TempCreate)
        .map_err(|source| detailed_error(error_path, "create stage", source))?;
    let (stage_name, mut stage_file) = allocate_bound_stage(directory, name, error_path)?;
    let prepared = (|| -> io::Result<(u64, FileObservation)> {
        checkpoint(BoundPublicationPrimitive::Write)?;
        let bytes_written = io::copy(reader, &mut stage_file)?;
        if let Some(mode) = mode {
            apply_mode(&stage_file, mode)?;
        }
        checkpoint(BoundPublicationPrimitive::FileSync)?;
        sync_file(&stage_file)?;
        let expected = match expected_bytes {
            Some(bytes) => bytes.to_vec(),
            None => snapshot_bound_stage(directory, stage_name.as_os_str(), &stage_file)?,
        };
        let status = fstat(&stage_file).map_err(errno_io)?;
        let entry = entry_from_stat(name.to_os_string(), &status, error_path)
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok((
            bytes_written,
            FileObservation {
                entry,
                bytes: expected,
            },
        ))
    })();
    let (bytes_written, expected) = match prepared {
        Ok(prepared) => prepared,
        Err(source) => {
            drop(stage_file);
            return Err(cleanup_stage_error(
                directory,
                error_path,
                stage_name,
                "prepare stage",
                source,
            ));
        }
    };
    let named = fstatat(
        directory,
        stage_name.as_os_str(),
        AtFlags::AT_SYMLINK_NOFOLLOW,
    )
    .map_err(|source| {
        cleanup_stage_error(
            directory,
            error_path,
            stage_name.clone(),
            "verify stage",
            errno_io(source),
        )
    })?;
    let opened = fstat(&stage_file).map_err(|source| {
        cleanup_stage_error(
            directory,
            error_path,
            stage_name.clone(),
            "verify stage",
            errno_io(source),
        )
    })?;
    if !same_identity(named, opened) {
        drop(stage_file);
        return Err(cleanup_stage_error(
            directory,
            error_path,
            stage_name,
            "verify stage",
            io::Error::other("stage identity changed before publication"),
        ));
    }
    match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(_) => {
            drop(stage_file);
            return Err(cleanup_stage_error(
                directory,
                error_path,
                stage_name,
                "inspect destination",
                io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
            ));
        }
        Err(Errno::ENOENT) => {}
        Err(source) => {
            drop(stage_file);
            return Err(cleanup_stage_error(
                directory,
                error_path,
                stage_name,
                "inspect destination",
                errno_io(source),
            ));
        }
    }
    drop(stage_file);
    checkpoint(BoundPublicationPrimitive::ExclusivePublish).map_err(|source| {
        cleanup_stage_error(
            directory,
            error_path,
            stage_name.clone(),
            "publish stage",
            source,
        )
    })?;
    if let Err(source) = linkat(
        directory,
        stage_name.as_os_str(),
        directory,
        name,
        AtFlags::empty(),
    ) {
        return Err(cleanup_stage_error(
            directory,
            error_path,
            stage_name,
            "publish stage",
            errno_io(source),
        ));
    }

    let first = match checkpoint(BoundPublicationPrimitive::PostPublishObserve) {
        Err(source) => Err(source),
        Ok(()) => observe_exclusive_dest(directory, name, &expected),
    };
    let cleanup = match checkpoint(BoundPublicationPrimitive::StageUnlink) {
        Err(source) => StageCleanup::Retained {
            stage: stage_name.clone(),
            source,
        },
        Ok(()) => match unlinkat(
            directory,
            stage_name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        ) {
            Ok(()) => StageCleanup::Removed,
            Err(source) => StageCleanup::Retained {
                stage: stage_name.clone(),
                source: errno_io(source),
            },
        },
    };
    let durability = match checkpoint(BoundPublicationPrimitive::ParentSync) {
        Err(source) => MetadataDurability::Unproven {
            source: Some(source),
        },
        Ok(()) => match fsync(directory) {
            Ok(()) => MetadataDurability::Confirmed,
            Err(source) => MetadataDurability::Unproven {
                source: Some(errno_io(source)),
            },
        },
    };
    let second = match checkpoint(BoundPublicationPrimitive::CompletionObserve) {
        Err(source) => Err(source),
        Ok(()) => observe_exclusive_dest(directory, name, &expected),
    };
    Ok(ExclusivePublication {
        bytes_written,
        final_name: select_exclusive_final_name(error_path.to_path_buf(), first, second),
        durability,
        cleanup,
    })
}

#[cfg(unix)]
fn snapshot_bound_stage(
    directory: &impl AsFd,
    stage_name: &OsStr,
    stage_file: &File,
) -> io::Result<Vec<u8>> {
    let reader = openat(
        directory,
        stage_name,
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(errno_io)?;
    let named = fstat(&reader).map_err(errno_io)?;
    let opened = fstat(stage_file).map_err(errno_io)?;
    if !same_identity(named, opened) {
        return Err(io::Error::other(
            "stage identity changed while snapshotting",
        ));
    }
    let mut bytes = Vec::new();
    let mut reader = reader;
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn observe_exclusive_dest(
    directory: &impl AsFd,
    name: &OsStr,
    expected: &FileObservation,
) -> io::Result<FileObservation> {
    let mut file = openat(
        directory,
        name,
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(errno_io)?;
    let status = fstat(&file).map_err(errno_io)?;
    let entry = entry_from_stat(name.to_os_string(), &status, Path::new(name))
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let observed = FileObservation { entry, bytes };
    if same_observation(&observed, expected) {
        Ok(observed)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "published destination identity or bytes changed before verification",
        ))
    }
}

/// Publish an already-open exclusive stage with `linkat(2)` without closing it.
///
/// The destination name is never visible with partial content. The returned
/// `File` still refers to the published inode after the stage name is unlinked.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn publish_open_stage_no_replace(
    directory: &impl AsFd,
    stage_name: &OsStr,
    dest_name: &OsStr,
    stage_file: File,
) -> Result<File, AtomicWriteError> {
    let path = Path::new(dest_name);
    if let Err(source) = linkat(
        directory,
        stage_name,
        directory,
        dest_name,
        AtFlags::empty(),
    ) {
        let _ = unlinkat(directory, stage_name, UnlinkatFlags::NoRemoveDir);
        drop(stage_file);
        return Err(io_error(path, errno_io(source)));
    }
    let _ = checkpoint(BoundPublicationPrimitive::StageUnlink);
    let _ = unlinkat(directory, stage_name, UnlinkatFlags::NoRemoveDir);
    let _ = fsync(directory);
    Ok(stage_file)
}

/// Stage → fsync → rename → parent-sync on an already-bound directory.
///
/// Pathname identity observations stay in [`atomic_replace_detailed`].
#[cfg(unix)]
fn publish_into_bound_directory(
    directory: &impl AsFd,
    name: &OsStr,
    contents: &[u8],
    mode: u32,
) -> Result<BoundPublicationResult, DetailedAtomicError> {
    let path = Path::new(name);
    inspect_destination(directory, name, path)?;
    checkpoint(BoundPublicationPrimitive::TempCreate)
        .map_err(|source| detailed_error(path, "create stage", source))?;
    let (stage_name, mut stage_file) = allocate_bound_stage(directory, name, path)?;
    pause_at("temp-create");
    let operation = (|| -> io::Result<FileObservation> {
        checkpoint(BoundPublicationPrimitive::Write)?;
        stage_file.write_all(contents)?;
        pause_at("write");
        stage_file.set_permissions(fs::Permissions::from_mode(mode))?;
        checkpoint(BoundPublicationPrimitive::FileSync)?;
        sync_file(&stage_file)?;
        pause_at("fsync-file");
        let status = fstat(&stage_file).map_err(errno_io)?;
        let entry = entry_from_stat(name.to_os_string(), &status, path)
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(FileObservation {
            entry,
            bytes: contents.to_vec(),
        })
    })();
    let observation = match operation {
        Ok(observation) => observation,
        Err(source) => {
            drop(stage_file);
            return Err(cleanup_stage_error(
                directory,
                path,
                stage_name,
                "prepare stage",
                source,
            ));
        }
    };
    drop(stage_file);
    pause_at("close");
    checkpoint(BoundPublicationPrimitive::Rename).map_err(|source| {
        cleanup_stage_error(directory, path, stage_name.clone(), "publish stage", source)
    })?;
    renameat(directory, stage_name.as_os_str(), directory, name).map_err(|source| {
        cleanup_stage_error(
            directory,
            path,
            stage_name.clone(),
            "publish stage",
            errno_io(source),
        )
    })?;
    pause_at("rename");
    let durability_source = match checkpoint(BoundPublicationPrimitive::ParentSync) {
        Err(source) => Some(source),
        Ok(()) => fsync(directory)
            .err()
            .map(|error| io::Error::from_raw_os_error(error as i32)),
    };
    pause_at("fsync-bound-parent-dir");
    let observation_source = verify_bound_publication(directory, name, &observation).err();
    Ok(BoundPublicationResult {
        observation,
        durability_source,
        observation_source,
    })
}

#[cfg(unix)]
fn verify_bound_publication(
    directory: &impl AsFd,
    name: &OsStr,
    observation: &FileObservation,
) -> io::Result<()> {
    let status = fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(errno_io)?;
    let current = entry_from_stat(name.to_os_string(), &status, Path::new(name))
        .map_err(|error| io::Error::other(error.to_string()))?;
    if same_entry_metadata(&observation.entry, &current) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "published destination identity changed before verification",
        ))
    }
}

/// Atomically replace `path` with durably prepared `contents`.
///
/// The replacement bytes are synced before rename. On Apple targets, that sync
/// also performs `F_FULLFSYNC`; a full-flush failure aborts publication and the
/// temporary file is removed. The containing directory is synced afterwards on
/// a best-effort basis; a directory-sync failure is logged and does not turn an
/// otherwise published replacement into an error.
#[cfg(unix)]
pub fn atomic_replace(
    path: impl AsRef<Path>,
    contents: &[u8],
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let path = path.as_ref();
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;

    let (temporary_path, mut temporary_file) = create_temporary(parent, path)?;
    pause_at("temp-create");
    let operation = (|| {
        temporary_file
            .write_all(contents)
            .map_err(|source| io_error(path, source))?;
        pause_at("write");
        sync_file(&temporary_file).map_err(|source| io_error(path, source))?;
        pause_at("fsync-file");
        if let Some(mode) = options.mode {
            apply_mode(&temporary_file, mode).map_err(|source| io_error(path, source))?;
        }
        pause_at("chmod");
        drop(temporary_file);
        pause_at("close");
        fs::rename(&temporary_path, path).map_err(|source| io_error(path, source))?;
        pause_at("rename");
        fsync_dir(parent);
        pause_at("fsync-parent-dir");
        Ok(())
    })();

    if operation.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    operation
}

/// Create a missing parent directory, then publish through the detailed Windows backend.
///
/// `options.mode` is inert on Windows beyond the backend's input validation.
#[cfg(windows)]
pub fn atomic_replace(
    path: impl AsRef<Path>,
    contents: &[u8],
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let path = path.as_ref();
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    let mode = options.mode.unwrap_or(0o644);
    map_detailed_outcome(path, atomic_replace_detailed(path, contents, mode))
}

#[cfg(windows)]
fn map_detailed_outcome(
    path: &Path,
    result: Result<DetailedAtomicOutcome, DetailedAtomicError>,
) -> Result<(), AtomicWriteError> {
    match result {
        Ok(_) => Ok(()),
        Err(error) => Err(io_error(path, error.source)),
    }
}

/// Publish `contents` only when `path` does not yet exist.
///
/// Contents are written and synced to an unlinked temporary inode first, then
/// published with `link(2)`. Consequently the destination name is never visible
/// with partial content.
#[cfg(unix)]
pub fn write_bytes_exclusive(
    path: impl AsRef<Path>,
    contents: &[u8],
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let path = path.as_ref();
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;

    let (temporary_path, mut temporary_file) = create_temporary(parent, path)?;
    let operation = (|| {
        temporary_file
            .write_all(contents)
            .map_err(|source| io_error(path, source))?;
        sync_file(&temporary_file).map_err(|source| io_error(path, source))?;
        if let Some(mode) = options.mode {
            apply_mode(&temporary_file, mode).map_err(|source| io_error(path, source))?;
        }
        drop(temporary_file);
        fs::hard_link(&temporary_path, path).map_err(|source| io_error(path, source))?;
        fs::remove_file(&temporary_path).map_err(|source| io_error(path, source))?;
        fsync_dir(parent);
        Ok(())
    })();

    if operation.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    operation
}

/// Create and durably fill a new file from a bounded-memory reader.
///
/// The destination is create-only, receives the requested final mode before
/// publication is reported, and is synced before this function returns. A
/// failed copy removes the incomplete destination.
#[cfg(unix)]
pub fn write_reader_exclusive(
    path: &Path,
    reader: &mut impl Read,
    options: AtomicWriteOptions,
) -> Result<u64, AtomicWriteError> {
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    let (temporary_path, mut temporary_file) = create_temporary(parent, path)?;
    let operation = (|| {
        let bytes =
            io::copy(reader, &mut temporary_file).map_err(|source| io_error(path, source))?;
        sync_file(&temporary_file).map_err(|source| io_error(path, source))?;
        if let Some(mode) = options.mode {
            apply_mode(&temporary_file, mode).map_err(|source| io_error(path, source))?;
        }
        drop(temporary_file);
        fs::hard_link(&temporary_path, path).map_err(|source| io_error(path, source))?;
        fs::remove_file(&temporary_path).map_err(|source| io_error(path, source))?;
        fsync_dir(parent);
        Ok(bytes)
    })();
    if operation.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    operation
}

/// Publish `contents` only when `path` does not yet exist.
#[cfg(windows)]
pub fn write_bytes_exclusive(
    path: impl AsRef<Path>,
    contents: &[u8],
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    windows_create_only::write_bytes_exclusive(path.as_ref(), contents, options)
}

/// Create and durably fill a new file from a bounded-memory reader.
#[cfg(windows)]
pub fn write_reader_exclusive(
    path: &Path,
    reader: &mut impl Read,
    options: AtomicWriteOptions,
) -> Result<u64, AtomicWriteError> {
    windows_create_only::write_reader_exclusive(path, reader, options)
}

/// Publish `contents` only when `path` does not yet exist, reporting each
/// post-publication fact independently.
#[cfg(windows)]
pub fn write_bytes_exclusive_detailed(
    path: impl AsRef<Path>,
    contents: &[u8],
    options: AtomicWriteOptions,
) -> Result<ExclusivePublication, DetailedAtomicError> {
    windows_create_only::write_bytes_exclusive_detailed(path.as_ref(), contents, options)
}

/// Publish a reader into a new dest name, reporting each post-publication fact
/// independently.
#[cfg(windows)]
pub fn write_reader_exclusive_detailed(
    path: &Path,
    reader: &mut impl Read,
    options: AtomicWriteOptions,
) -> Result<ExclusivePublication, DetailedAtomicError> {
    windows_create_only::write_reader_exclusive_detailed(path, reader, options)
}

/// Publish an already-created temporary file by syncing and atomically renaming it.
#[cfg(unix)]
pub fn install_file(
    temporary_path: impl AsRef<Path>,
    path: impl AsRef<Path>,
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let temporary_path = temporary_path.as_ref();
    let path = path.as_ref();
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    let temporary_file = File::open(temporary_path).map_err(|source| io_error(path, source))?;
    let operation = (|| {
        sync_file(&temporary_file).map_err(|source| io_error(path, source))?;
        if let Some(mode) = options.mode {
            apply_mode(&temporary_file, mode).map_err(|source| io_error(path, source))?;
        }
        drop(temporary_file);
        fs::rename(temporary_path, path).map_err(|source| io_error(path, source))?;
        fsync_dir(parent);
        Ok(())
    })();
    if operation.is_err() {
        let _ = fs::remove_file(temporary_path);
    }
    operation
}

/// Serialize and atomically replace a JSON file.
#[cfg(any(unix, windows))]
pub fn write_json<T: Serialize>(
    path: impl AsRef<Path>,
    value: &T,
    options: JsonWriteOptions,
) -> Result<(), AtomicWriteError> {
    let mut value =
        serde_json::to_value(value).map_err(|source| serialization_error(path.as_ref(), source))?;
    if options.sort_keys {
        sort_json_keys(&mut value);
    }
    let mut contents = serialize_json(&value, options.indent)
        .map_err(|source| serialization_error(path.as_ref(), source))?;
    contents.push(b'\n');
    atomic_replace(path, &contents, AtomicWriteOptions { mode: options.mode })
}

/// Atomically replace a UTF-8 text file.
#[cfg(any(unix, windows))]
pub fn write_text(
    path: impl AsRef<Path>,
    text: &str,
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    atomic_replace(path, text.as_bytes(), options)
}

/// Atomically replace a JSONL file with one record per line.
#[cfg(any(unix, windows))]
pub fn write_jsonl<T: Serialize>(
    path: impl AsRef<Path>,
    records: impl IntoIterator<Item = T>,
    options: AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let path = path.as_ref();
    let mut contents = Vec::new();
    for record in records {
        serde_json::to_writer(&mut contents, &record)
            .map_err(|source| serialization_error(path, source))?;
        contents.push(b'\n');
    }
    atomic_replace(path, &contents, options)
}

/// Sync a file before its name is published.
///
/// All sync failures are hard errors. On Apple targets this performs both
/// `sync_all()` and `F_FULLFSYNC`; an `F_FULLFSYNC` failure propagates to the
/// caller before rename or hard-link publication. This is distinct from the
/// best-effort parent-directory sync performed after publication.
#[cfg(unix)]
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()?;
    #[cfg(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos"
    ))]
    {
        nix::fcntl::fcntl(file, nix::fcntl::FcntlArg::F_FULLFSYNC)
            .map_err(|error| io::Error::from_raw_os_error(error as i32))?;
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn fsync_dir(path: &Path) {
    if let Err(error) = File::open(path).and_then(|directory| directory.sync_all()) {
        log::warn!(
            "parent-directory fsync degraded for {}: {error}",
            path.display()
        );
    }
}

#[cfg(any(unix, windows))]
fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn normal_name(value: Option<&OsStr>) -> Option<&OsStr> {
    let value = value?;
    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(component)), None) if component == value => Some(value),
        _ => None,
    }
}

#[cfg(unix)]
fn stat_parent(parent: &Path) -> io::Result<nix::sys::stat::FileStat> {
    let status = fstatat(AT_FDCWD, parent, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(errno_io)?;
    if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR {
        Ok(status)
    } else {
        Err(io::Error::other("parent is not a real directory"))
    }
}

#[cfg(unix)]
fn same_identity(left: nix::sys::stat::FileStat, right: nix::sys::stat::FileStat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino
}

#[cfg(unix)]
fn inspect_destination(
    directory: &impl AsFd,
    name: &OsStr,
    path: &Path,
) -> Result<(), DetailedAtomicError> {
    match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(status)
            if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG =>
        {
            Ok(())
        }
        Ok(_) => Err(detailed_error(
            path,
            "inspect destination",
            io::Error::other("destination is not a regular file"),
        )),
        Err(Errno::ENOENT) => Ok(()),
        Err(source) => Err(detailed_errno(path, "inspect destination", source)),
    }
}

pub(crate) const ATOMIC_CANDIDATE_MARKER: &str = "tmp";
#[cfg(unix)]
pub(crate) const STAGED_CANDIDATE_MARKER: &str = "stage";
pub(crate) const CANDIDATE_SUFFIX: &str = ".tmp";

pub(crate) fn publication_candidate_name(
    destination_name: &OsStr,
    marker: &str,
    entropy: &[u128],
) -> OsString {
    let mut name = OsString::new();
    if destination_name.as_encoded_bytes().first() == Some(&b'.') {
        name.push("_");
    } else {
        name.push(".");
    }
    name.push(marker);
    for value in entropy {
        name.push("_");
        name.push(value.to_string());
    }
    name.push(CANDIDATE_SUFFIX);
    name
}

#[cfg(unix)]
pub(crate) fn allocate_bound_stage(
    directory: &impl AsFd,
    destination: &OsStr,
    path: &Path,
) -> Result<(OsString, File), DetailedAtomicError> {
    for _ in 0..100 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = publication_candidate_name(
            destination,
            ATOMIC_CANDIDATE_MARKER,
            &[u128::from(std::process::id()), u128::from(sequence)],
        );
        match openat(
            directory,
            candidate.as_os_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        ) {
            Ok(fd) => return Ok((candidate, File::from(fd))),
            Err(Errno::EEXIST) => continue,
            Err(source) => return Err(detailed_errno(path, "create stage", source)),
        }
    }
    Err(detailed_error(
        path,
        "create stage",
        io::Error::new(io::ErrorKind::AlreadyExists, "could not allocate stage"),
    ))
}

#[cfg(unix)]
fn cleanup_stage_error(
    directory: &impl AsFd,
    path: &Path,
    stage: OsString,
    operation: &'static str,
    source: io::Error,
) -> DetailedAtomicError {
    let cleanup = unlinkat(directory, stage.as_os_str(), UnlinkatFlags::NoRemoveDir)
        .err()
        .map(errno_io);
    DetailedAtomicError {
        path: path.to_path_buf(),
        operation,
        source,
        orphan_stage: cleanup.as_ref().map(|_| stage),
        cleanup_error: cleanup,
    }
}

fn detailed_error(path: &Path, operation: &'static str, source: io::Error) -> DetailedAtomicError {
    DetailedAtomicError {
        path: path.to_path_buf(),
        operation,
        source,
        orphan_stage: None,
        cleanup_error: None,
    }
}

#[cfg(unix)]
fn detailed_errno(path: &Path, operation: &'static str, source: Errno) -> DetailedAtomicError {
    detailed_error(path, operation, errno_io(source))
}

#[cfg(unix)]
fn errno_io(source: Errno) -> io::Error {
    io::Error::from_raw_os_error(source as i32)
}

#[cfg(unix)]
fn create_temporary(
    parent: &Path,
    destination: &Path,
) -> Result<(PathBuf, File), AtomicWriteError> {
    let destination_name = destination.file_name().unwrap_or(OsStr::new(""));
    for _ in 0..100 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate = parent.join(publication_candidate_name(
            destination_name,
            ATOMIC_CANDIDATE_MARKER,
            &[u128::from(std::process::id()), nanos, u128::from(sequence)],
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(io_error(destination, source)),
        }
    }
    Err(io_error(
        destination,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate unique temporary file",
        ),
    ))
}

#[cfg(unix)]
fn apply_mode(file: &File, mode: u32) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn apply_mode(_file: &File, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(any(unix, windows))]
fn io_error(path: &Path, source: io::Error) -> AtomicWriteError {
    AtomicWriteError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(any(unix, windows))]
fn serialization_error(path: &Path, source: serde_json::Error) -> AtomicWriteError {
    io_error(path, io::Error::new(io::ErrorKind::InvalidData, source))
}

#[cfg(any(unix, windows))]
fn serialize_json(value: &serde_json::Value, indent: Option<usize>) -> serde_json::Result<Vec<u8>> {
    match indent {
        Some(width) => {
            let mut contents = Vec::new();
            let indent = vec![b' '; width];
            let formatter = serde_json::ser::PrettyFormatter::with_indent(&indent);
            let mut serializer = serde_json::Serializer::with_formatter(&mut contents, formatter);
            value.serialize(&mut serializer)?;
            Ok(contents)
        }
        None => serde_json::to_vec(value),
    }
}

#[cfg(any(unix, windows))]
fn sort_json_keys(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            let mut sorted = std::collections::BTreeMap::new();
            for (key, mut child) in std::mem::take(object) {
                sort_json_keys(&mut child);
                sorted.insert(key, child);
            }
            *object = sorted.into_iter().collect();
        }
        serde_json::Value::Array(values) => {
            for child in values {
                sort_json_keys(child);
            }
        }
        _ => {}
    }
}

// Gated on `test-hooks` as well as `test` so a dependent that enables the feature
// can inject a crash on this path. `staged.rs` already honours both; this half was
// `cfg(test)`-only, which left the hook unreachable outside this crate even for a
// dependent that asked for it by name (`solstone-core-sol-link`).
#[cfg(any(test, feature = "test-hooks"))]
fn pause_at(step: &str) {
    if std::env::var("JOURNAL_IO_TEST_PAUSE_AT").ok().as_deref() != Some(step) {
        return;
    }
    if let Ok(marker) = std::env::var("JOURNAL_IO_TEST_MARKER") {
        let _ = fs::write(marker, step);
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn pause_at(_step: &str) {}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
fn checkpoint(primitive: BoundPublicationPrimitive) -> Result<(), io::Error> {
    let (fault, barrier) = BOUND_PUBLICATION_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return (None, None);
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == primitive)
            .count();
        let inject = state
            .faults
            .iter()
            .position(|fault| fault.primitive == primitive && fault.ordinal == ordinal);
        if let Some(index) = inject {
            let fault = state.faults.remove(index);
            state.faults_consumed += 1;
            (Some(fault.error), None)
        } else {
            let barrier = state
                .barriers
                .iter()
                .position(|barrier| barrier.primitive == primitive && barrier.ordinal == ordinal);
            if let Some(index) = barrier {
                let barrier = state.barriers.remove(index);
                state.barriers_fired += 1;
                (None, Some(barrier.callback))
            } else {
                (None, None)
            }
        }
    });
    if let Some(error) = fault {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    if let Some(callback) = barrier {
        callback();
    }
    Ok(())
}

#[cfg(all(unix, not(any(test, feature = "test-hooks"))))]
fn checkpoint(_primitive: BoundPublicationPrimitive) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn detailed_replace_publishes_with_exact_mode_without_creating_parent() {
        let temporary = TempDir::new();
        let missing_parent = temporary.path().join("missing");
        let missing = missing_parent.join("unit.service");
        assert!(atomic_replace_detailed(&missing, b"new", 0o644).is_err());
        assert!(!missing_parent.exists());

        let target = temporary.path().join("unit.service");
        fs::write(&target, b"old").unwrap();
        let result = atomic_replace_detailed(&target, b"new", 0o640).unwrap();
        assert!(matches!(result, DetailedAtomicOutcome::Published));
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(fs::read_dir(temporary.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp_")
        }));
    }

    #[test]
    fn detailed_replace_refuses_unsafe_destination_and_mode() {
        let temporary = TempDir::new();
        let target = temporary.path().join("unit.service");
        let outside = temporary.path().join("outside");
        fs::write(&outside, b"outside").unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        assert!(atomic_replace_detailed(&target, b"new", 0o644).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
        assert!(atomic_replace_detailed(&target, b"new", 0o1000).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
    }

    #[test]
    fn reader_exclusive_is_create_only_and_copies_the_full_stream() {
        let temporary = TempDir::new();
        let path = temporary.path().join("reader.bin");
        let mut reader = io::Cursor::new(vec![b'x'; 131_073]);
        let copied =
            write_reader_exclusive(&path, &mut reader, AtomicWriteOptions { mode: Some(0o600) })
                .unwrap();
        assert_eq!(copied, 131_073);
        assert_eq!(fs::read(&path).unwrap(), vec![b'x'; 131_073]);
        assert!(
            write_reader_exclusive(
                &path,
                &mut io::Cursor::new(b"replacement"),
                AtomicWriteOptions::default(),
            )
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), vec![b'x'; 131_073]);
    }
    use crate::test_support::TempDir;

    #[test]
    fn write_bytes_exclusive_publishes_only_a_complete_temp_inode() {
        let temporary = TempDir::new();
        let target = temporary.path().join("record.bin");
        let payload = vec![b'x'; 1024 * 1024];

        write_bytes_exclusive(&target, &payload, AtomicWriteOptions::default()).unwrap();

        assert_eq!(fs::read(&target).unwrap(), payload);
        assert!(fs::read_dir(temporary.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp_")
        }));
        assert!(write_bytes_exclusive(&target, b"other", AtomicWriteOptions::default()).is_err());
        assert_eq!(fs::read(&target).unwrap().len(), 1024 * 1024);
    }

    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStringExt;

    fn name_255() -> OsString {
        OsString::from("a".repeat(255))
    }

    fn os_from_bytes(bytes: &[u8]) -> OsString {
        OsString::from_vec(bytes.to_vec())
    }

    fn assert_candidate_bounded(destination_name: &OsStr, marker: &str) {
        let candidate = publication_candidate_name(
            destination_name,
            marker,
            &[u128::from(u32::MAX), u128::MAX, u128::from(u64::MAX)],
        );
        assert!(
            candidate.as_encoded_bytes().len() < 100,
            "candidate {} bytes",
            candidate.as_encoded_bytes().len()
        );
    }

    #[test]
    fn filesystem_accepts_255_byte_file_names() {
        let temporary = TempDir::new();
        let path = temporary.path().join(name_255());
        fs::write(&path, b"ok").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"ok");
    }

    #[test]
    fn write_bytes_exclusive_publishes_255_byte_basename() {
        let temporary = TempDir::new();
        let path = temporary.path().join(name_255());
        assert_candidate_bounded(path.file_name().unwrap(), ATOMIC_CANDIDATE_MARKER);
        write_bytes_exclusive(&path, b"payload", AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"payload");
    }

    #[test]
    fn detailed_replace_publishes_255_byte_basename() {
        let temporary = TempDir::new();
        let path = temporary.path().join(name_255());
        assert_candidate_bounded(path.file_name().unwrap(), ATOMIC_CANDIDATE_MARKER);
        let result = atomic_replace_detailed(&path, b"payload", 0o644).unwrap();
        assert!(matches!(result, DetailedAtomicOutcome::Published));
        assert_eq!(fs::read(&path).unwrap(), b"payload");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn write_bytes_exclusive_preserves_distinct_invalid_utf8_basenames() {
        let temporary = TempDir::new();
        let left = temporary.path().join(os_from_bytes(b"file-\xff-a"));
        let right = temporary.path().join(os_from_bytes(b"file-\xfe-a"));
        write_bytes_exclusive(&left, b"alpha", AtomicWriteOptions::default()).unwrap();
        write_bytes_exclusive(&right, b"beta", AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&left).unwrap(), b"alpha");
        assert_eq!(fs::read(&right).unwrap(), b"beta");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn write_reader_exclusive_preserves_distinct_invalid_utf8_basenames() {
        let temporary = TempDir::new();
        let left = temporary.path().join(os_from_bytes(b"reader-\xff-a"));
        let right = temporary.path().join(os_from_bytes(b"reader-\xfe-a"));
        let copied_left = write_reader_exclusive(
            &left,
            &mut io::Cursor::new(b"alpha"),
            AtomicWriteOptions::default(),
        )
        .unwrap();
        let copied_right = write_reader_exclusive(
            &right,
            &mut io::Cursor::new(b"beta"),
            AtomicWriteOptions::default(),
        )
        .unwrap();
        assert_eq!(copied_left, 5);
        assert_eq!(copied_right, 4);
        assert_eq!(fs::read(&left).unwrap(), b"alpha");
        assert_eq!(fs::read(&right).unwrap(), b"beta");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn atomic_replace_preserves_distinct_invalid_utf8_basenames() {
        let temporary = TempDir::new();
        let left = temporary.path().join(os_from_bytes(b"replace-\xff-a"));
        let right = temporary.path().join(os_from_bytes(b"replace-\xfe-a"));
        atomic_replace(&left, b"alpha", AtomicWriteOptions::default()).unwrap();
        atomic_replace(&right, b"beta", AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&left).unwrap(), b"alpha");
        assert_eq!(fs::read(&right).unwrap(), b"beta");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detailed_replace_preserves_distinct_invalid_utf8_basenames() {
        let temporary = TempDir::new();
        let left = temporary.path().join(os_from_bytes(b"detailed-\xff-a"));
        let right = temporary.path().join(os_from_bytes(b"detailed-\xfe-a"));
        assert!(matches!(
            atomic_replace_detailed(&left, b"alpha", 0o644).unwrap(),
            DetailedAtomicOutcome::Published
        ));
        assert!(matches!(
            atomic_replace_detailed(&right, b"beta", 0o644).unwrap(),
            DetailedAtomicOutcome::Published
        ));
        assert_eq!(fs::read(&left).unwrap(), b"alpha");
        assert_eq!(fs::read(&right).unwrap(), b"beta");
    }

    #[test]
    fn publication_candidate_dot_destination_uses_underscore_sentinel() {
        let name = publication_candidate_name(OsStr::new(".env"), ATOMIC_CANDIDATE_MARKER, &[1, 2]);
        assert_eq!(name.as_encoded_bytes().first(), Some(&b'_'));
        assert!(name.as_encoded_bytes().starts_with(b"_tmp_"));
    }

    #[test]
    fn publication_candidate_underscore_destination_uses_dot_sentinel() {
        let name = publication_candidate_name(OsStr::new("_keep"), ATOMIC_CANDIDATE_MARKER, &[1]);
        assert_eq!(name.as_encoded_bytes().first(), Some(&b'.'));
        assert!(name.as_encoded_bytes().starts_with(b".tmp_"));
    }

    #[test]
    fn publication_candidate_ordinary_ascii_uses_dot_sentinel() {
        let name =
            publication_candidate_name(OsStr::new("report.json"), ATOMIC_CANDIDATE_MARKER, &[1, 2]);
        assert_eq!(name.as_encoded_bytes().first(), Some(&b'.'));
        assert!(name.as_encoded_bytes().starts_with(b".tmp_"));
        assert!(name.as_encoded_bytes().ends_with(b".tmp"));
        let staged =
            publication_candidate_name(OsStr::new("bundle"), STAGED_CANDIDATE_MARKER, &[1, 2]);
        assert!(staged.as_encoded_bytes().starts_with(b".stage_"));
        assert!(staged.as_encoded_bytes().ends_with(b".tmp"));
        assert!(!staged.as_encoded_bytes().starts_with(b".tmp_"));
    }

    #[test]
    fn publication_candidate_interior_dot_does_not_flip_sentinel() {
        let name = publication_candidate_name(OsStr::new("foo.bar"), ATOMIC_CANDIDATE_MARKER, &[1]);
        assert_eq!(name.as_encoded_bytes().first(), Some(&b'.'));
    }

    #[test]
    fn publication_candidate_invalid_utf8_leading_byte_uses_dot_unless_ascii_dot() {
        let not_dot = publication_candidate_name(
            &os_from_bytes(b"\xffhidden"),
            ATOMIC_CANDIDATE_MARKER,
            &[1],
        );
        assert_eq!(not_dot.as_encoded_bytes().first(), Some(&b'.'));
        let leading_dot = publication_candidate_name(
            &os_from_bytes(b".\xffhidden"),
            ATOMIC_CANDIDATE_MARKER,
            &[1],
        );
        assert_eq!(leading_dot.as_encoded_bytes().first(), Some(&b'_'));
        assert!(
            !not_dot.as_encoded_bytes().contains(&0xff)
                && !leading_dot.as_encoded_bytes().contains(&0xff)
        );
        let replacement = [0xef, 0xbf, 0xbd];
        assert!(
            !not_dot
                .as_encoded_bytes()
                .windows(3)
                .any(|window| window == replacement)
        );
        assert!(
            !leading_dot
                .as_encoded_bytes()
                .windows(3)
                .any(|window| window == replacement)
        );
    }

    #[test]
    fn publication_candidate_leading_bytes_remain_distinct_after_ascii_case_fold() {
        // '.' (U+002E) and '_' (U+005F) have no canonical decomposition and no case mapping.
        for dest in [OsStr::new(".env"), OsStr::new("report.json")] {
            let candidate = publication_candidate_name(dest, ATOMIC_CANDIDATE_MARKER, &[1, 2]);
            let dest_lead = dest
                .as_encoded_bytes()
                .first()
                .copied()
                .unwrap_or(b'x')
                .to_ascii_lowercase();
            let cand_lead = candidate.as_encoded_bytes()[0].to_ascii_lowercase();
            assert_ne!(cand_lead, dest_lead);
        }
    }

    fn open_directory(path: &Path) -> std::os::fd::OwnedFd {
        nix::fcntl::openat(
            nix::fcntl::AT_FDCWD,
            path,
            nix::fcntl::OFlag::O_RDONLY
                | nix::fcntl::OFlag::O_DIRECTORY
                | nix::fcntl::OFlag::O_CLOEXEC
                | nix::fcntl::OFlag::O_NOFOLLOW,
            nix::sys::stat::Mode::empty(),
        )
        .expect("open bound test directory")
    }

    #[test]
    fn atomic_replace_bound_publishes_with_exact_mode() {
        let temporary = TempDir::new();
        let target = temporary.path().join("unit.service");
        fs::write(&target, b"old").unwrap();
        let directory = open_directory(temporary.path());
        let result =
            atomic_replace_bound(&directory, OsStr::new("unit.service"), b"new", 0o640).unwrap();
        let BoundAtomicOutcome::Published { observation } = result else {
            panic!("bound publication must be durable");
        };
        assert_eq!(observation.entry.name, OsStr::new("unit.service"));
        assert_eq!(observation.bytes, b"new");
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[test]
    fn atomic_replace_bound_survives_parent_pathname_rename() {
        let temporary = TempDir::new();
        let parent = temporary.path().join("parent");
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("unit.service"), b"old").unwrap();
        let directory = open_directory(&parent);
        let moved = temporary.path().join("parent-moved");
        fs::rename(&parent, &moved).unwrap();
        fs::create_dir(&parent).unwrap();
        fs::write(parent.join("unit.service"), b"replacement").unwrap();

        let result =
            atomic_replace_bound(&directory, OsStr::new("unit.service"), b"new", 0o644).unwrap();
        assert!(matches!(
            result,
            BoundAtomicOutcome::Published { observation }
                if observation.bytes == b"new"
        ));
        assert_eq!(fs::read(moved.join("unit.service")).unwrap(), b"new");
        assert_eq!(
            fs::read(parent.join("unit.service")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn write_bytes_exclusive_bound_publishes_only_a_complete_inode() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        write_bytes_exclusive_bound(&directory, OsStr::new("record.bin"), b"payload", 0o600)
            .unwrap();
        assert_eq!(
            fs::read(temporary.path().join("record.bin")).unwrap(),
            b"payload"
        );
        assert!(
            write_bytes_exclusive_bound(&directory, OsStr::new("record.bin"), b"other", 0o600)
                .is_err()
        );
        assert_eq!(
            fs::read(temporary.path().join("record.bin")).unwrap(),
            b"payload"
        );
    }

    #[test]
    fn write_bytes_exclusive_bound_reports_post_link_stage_unlink_failure() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());

        let (result, fault_consumed) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::StageUnlink,
            1,
            Errno::EIO as i32,
            || write_bytes_exclusive_bound(&directory, OsStr::new("record.bin"), b"payload", 0o600),
        );

        assert!(fault_consumed, "stage-unlink fault was not consumed");
        assert!(matches!(result, Err(AtomicWriteError::Io { .. })));
        assert_eq!(
            fs::read(temporary.path().join("record.bin")).unwrap(),
            b"payload"
        );
        let stale_stages: Vec<_> = fs::read_dir(temporary.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.as_encoded_bytes().starts_with(b".tmp_"))
            .collect();
        assert_eq!(stale_stages.len(), 1);
    }

    #[test]
    fn bound_publication_faults_preserve_or_report_publication_state() {
        for primitive in [
            BoundPublicationPrimitive::TempCreate,
            BoundPublicationPrimitive::Write,
            BoundPublicationPrimitive::FileSync,
            BoundPublicationPrimitive::Rename,
            BoundPublicationPrimitive::ParentSync,
        ] {
            let temporary = TempDir::new();
            let target = temporary.path().join("unit.service");
            fs::write(&target, b"old").unwrap();
            let directory = open_directory(temporary.path());

            let (result, fault_consumed) =
                run_with_bound_publication_fault(primitive, 1, Errno::EIO as i32, || {
                    atomic_replace_bound(&directory, OsStr::new("unit.service"), b"new", 0o600)
                });

            assert!(fault_consumed, "{primitive:?} fault was not consumed");
            match primitive {
                BoundPublicationPrimitive::ParentSync => {
                    assert!(matches!(
                        result,
                        Ok(BoundAtomicOutcome::PublishedDurabilityUncertain {
                            ref observation,
                            ref source,
                        }) if source.raw_os_error() == Some(Errno::EIO as i32)
                            && observation.bytes == b"new"
                    ));
                    assert_eq!(fs::read(&target).unwrap(), b"new");
                }
                _ => {
                    assert!(result.is_err(), "{primitive:?} unexpectedly published");
                    assert_eq!(fs::read(&target).unwrap(), b"old");
                }
            }
            assert!(fs::read_dir(temporary.path()).unwrap().all(|entry| {
                let name = entry.unwrap().file_name();
                name == OsStr::new("unit.service")
            }));
        }
    }

    #[test]
    fn bound_publication_barriers_fire_in_checkpoint_order() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let observed = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::clone(&observed);
        let second = Arc::clone(&observed);

        let (result, barriers_fired) = run_with_two_bound_publication_barriers(
            BoundPublicationPrimitive::TempCreate,
            1,
            move || {
                first
                    .lock()
                    .unwrap()
                    .push(BoundPublicationPrimitive::TempCreate)
            },
            BoundPublicationPrimitive::ParentSync,
            1,
            move || {
                second
                    .lock()
                    .unwrap()
                    .push(BoundPublicationPrimitive::ParentSync)
            },
            || atomic_replace_bound(&directory, OsStr::new("unit.service"), b"new", 0o600),
        );

        assert!(matches!(
            result,
            Ok(BoundAtomicOutcome::Published { observation })
                if observation.bytes == b"new"
        ));
        assert_eq!(barriers_fired, 2);
        assert_eq!(
            observed.lock().unwrap().as_slice(),
            [
                BoundPublicationPrimitive::TempCreate,
                BoundPublicationPrimitive::ParentSync,
            ]
        );
    }

    #[test]
    fn bound_publication_does_not_report_success_after_destination_disappears() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let target = temporary.path().join("unit.service");
        let callback_target = target.clone();

        let (result, barrier_fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::ParentSync,
            1,
            move || fs::remove_file(&callback_target).expect("remove published destination"),
            || atomic_replace_bound(&directory, OsStr::new("unit.service"), b"new", 0o600),
        );

        assert!(barrier_fired);
        assert!(matches!(
            result,
            Ok(BoundAtomicOutcome::PublishedObservationUncertain {
                observation,
                source,
                durability_source: None,
            }) if observation.bytes == b"new" && source.kind() == io::ErrorKind::NotFound
        ));
        assert!(!target.exists());
    }

    const EXCLUSIVE_PAYLOAD: &[u8] = b"exclusive-detailed-payload";
    const EXCLUSIVE_COMPETITOR: &[u8] = b"competitor-bytes";

    fn exclusive_stage_names(parent: &Path) -> Vec<OsString> {
        fs::read_dir(parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| {
                let lossy = name.to_string_lossy();
                lossy.starts_with(".tmp_") || lossy.starts_with("_.tmp_")
            })
            .collect()
    }

    fn assert_no_exclusive_stage(parent: &Path) {
        let leftover = exclusive_stage_names(parent);
        assert!(leftover.is_empty(), "stage residue remained: {leftover:?}");
    }

    fn replace_exclusive_stage_name(parent: &Path) {
        let names = exclusive_stage_names(parent);
        assert_eq!(names.len(), 1, "expected one stage, found {names:?}");
        let path = parent.join(&names[0]);
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"intruder").unwrap();
    }

    #[test]
    fn detailed_exclusive_publication_confirmed_success() {
        let temporary = TempDir::new();
        let bytes_path = temporary.path().join("bytes.bin");
        let published = write_bytes_exclusive_detailed(
            &bytes_path,
            EXCLUSIVE_PAYLOAD,
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .unwrap();
        assert!(
            published.is_fully_confirmed(),
            "unix exclusive detailed success must be fully confirmed"
        );
        assert_eq!(published.bytes_written, EXCLUSIVE_PAYLOAD.len() as u64);
        assert!(matches!(
            published.final_name,
            FinalNameConfirmation::Confirmed { ref observation }
                if observation.bytes == EXCLUSIVE_PAYLOAD
        ));
        assert!(matches!(
            published.durability,
            MetadataDurability::Confirmed
        ));
        assert!(matches!(published.cleanup, StageCleanup::Removed));
        assert_eq!(fs::read(&bytes_path).unwrap(), EXCLUSIVE_PAYLOAD);
        assert_no_exclusive_stage(temporary.path());

        let reader_path = temporary.path().join("reader.bin");
        let mut reader = io::Cursor::new(EXCLUSIVE_PAYLOAD);
        let published = write_reader_exclusive_detailed(
            &reader_path,
            &mut reader,
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .unwrap();
        assert!(published.is_fully_confirmed());
        assert_eq!(published.bytes_written, EXCLUSIVE_PAYLOAD.len() as u64);
        assert_eq!(fs::read(&reader_path).unwrap(), EXCLUSIVE_PAYLOAD);

        let directory = open_directory(temporary.path());
        let published = write_bytes_exclusive_bound_detailed(
            &directory,
            OsStr::new("bound.bin"),
            EXCLUSIVE_PAYLOAD,
            0o600,
        )
        .unwrap();
        assert!(published.is_fully_confirmed());
        assert_eq!(
            fs::read(temporary.path().join("bound.bin")).unwrap(),
            EXCLUSIVE_PAYLOAD
        );
        assert_no_exclusive_stage(temporary.path());
    }

    #[test]
    fn detailed_exclusive_publication_missing_parent_is_not_created() {
        let temporary = TempDir::new();
        let missing_parent = temporary.path().join("missing");
        let dest = missing_parent.join("leaf.bin");
        assert!(
            write_bytes_exclusive_detailed(&dest, EXCLUSIVE_PAYLOAD, AtomicWriteOptions::default())
                .is_err()
        );
        assert!(!missing_parent.exists());
    }

    #[test]
    fn detailed_exclusive_publication_preexisting_dest_is_untouched() {
        let temporary = TempDir::new();
        let path = temporary.path().join("held.bin");
        fs::write(&path, EXCLUSIVE_COMPETITOR).unwrap();
        let before = fs::metadata(&path).unwrap();
        let error =
            write_bytes_exclusive_detailed(&path, EXCLUSIVE_PAYLOAD, AtomicWriteOptions::default())
                .expect_err("occupied destination must refuse");
        assert_eq!(error.source.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), EXCLUSIVE_COMPETITOR);
        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(after.modified().ok(), before.modified().ok());
        assert_no_exclusive_stage(temporary.path());
    }

    #[test]
    fn detailed_exclusive_publication_exclusive_stage_collision_preserved() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let dest = temporary.path().join("record.bin");
        let dest_for_barrier = dest.clone();
        let (result, fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::ExclusivePublish,
            1,
            move || fs::write(&dest_for_barrier, EXCLUSIVE_COMPETITOR).unwrap(),
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert!(fired);
        assert!(result.is_err());
        assert_eq!(fs::read(&dest).unwrap(), EXCLUSIVE_COMPETITOR);
        assert_no_exclusive_stage(temporary.path());
    }

    #[test]
    fn detailed_exclusive_publication_stage_identity_mismatch_refused() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let parent = temporary.path().to_path_buf();
        let dest = temporary.path().join("record.bin");
        let (result, fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::FileSync,
            1,
            move || replace_exclusive_stage_name(&parent),
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert!(fired);
        assert!(result.is_err());
        assert!(!dest.exists());
        assert_no_exclusive_stage(temporary.path());
    }

    #[test]
    fn detailed_exclusive_publication_race_before_observation_one_is_unverified() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let dest = temporary.path().join("record.bin");
        let dest_for_barrier = dest.clone();
        let (result, fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::PostPublishObserve,
            1,
            move || fs::remove_file(&dest_for_barrier).unwrap(),
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert!(fired);
        let published = result.unwrap();
        assert!(matches!(
            published.final_name,
            FinalNameConfirmation::Unverified { ref destination, ref reason }
                if destination == Path::new("record.bin")
                    && reason.kind() == io::ErrorKind::NotFound
        ));
        assert!(matches!(published.cleanup, StageCleanup::Removed));
        assert!(!dest.exists());
        assert_no_exclusive_stage(temporary.path());
    }

    #[test]
    fn detailed_exclusive_publication_forced_durability_failure_is_unproven() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let dest = temporary.path().join("record.bin");
        let (result, consumed) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::ParentSync,
            1,
            Errno::EIO as i32,
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert!(consumed);
        let published = result.unwrap();
        assert!(matches!(
            published.final_name,
            FinalNameConfirmation::Confirmed { ref observation }
                if observation.bytes == EXCLUSIVE_PAYLOAD
        ));
        assert!(matches!(
            published.durability,
            MetadataDurability::Unproven { source: Some(ref source) }
                if source.raw_os_error() == Some(Errno::EIO as i32)
        ));
        assert!(matches!(published.cleanup, StageCleanup::Removed));
        assert!(!published.is_fully_confirmed());
        assert_eq!(fs::read(&dest).unwrap(), EXCLUSIVE_PAYLOAD);
        assert_no_exclusive_stage(temporary.path());
    }

    #[test]
    fn detailed_exclusive_publication_forced_cleanup_failure_is_retained() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let dest = temporary.path().join("record.bin");
        let (result, consumed) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::StageUnlink,
            1,
            Errno::EIO as i32,
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert!(consumed);
        let published = result.unwrap();
        assert!(matches!(
            published.final_name,
            FinalNameConfirmation::Confirmed { ref observation }
                if observation.bytes == EXCLUSIVE_PAYLOAD
        ));
        assert!(matches!(
            published.durability,
            MetadataDurability::Confirmed
        ));
        assert!(matches!(
            published.cleanup,
            StageCleanup::Retained { ref source, .. }
                if source.raw_os_error() == Some(Errno::EIO as i32)
        ));
        assert_eq!(fs::read(&dest).unwrap(), EXCLUSIVE_PAYLOAD);
        assert_eq!(exclusive_stage_names(temporary.path()).len(), 1);
    }

    #[test]
    fn detailed_exclusive_publication_final_observation_and_durability_combined_failure() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let dest = temporary.path().join("record.bin");
        let dest_for_barrier = dest.clone();
        let (result, consumed, fired) = run_with_bound_publication_faults_and_barrier(
            [(BoundPublicationPrimitive::ParentSync, 1, Errno::EIO as i32)],
            BoundPublicationPrimitive::CompletionObserve,
            1,
            move || fs::remove_file(&dest_for_barrier).unwrap(),
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert_eq!(consumed, 1);
        assert!(fired);
        let published = result.unwrap();
        assert!(matches!(
            published.final_name,
            FinalNameConfirmation::Unverified { ref reason, .. }
                if reason.kind() == io::ErrorKind::NotFound
        ));
        assert!(matches!(
            published.durability,
            MetadataDurability::Unproven { source: Some(_) }
        ));
        assert!(matches!(published.cleanup, StageCleanup::Removed));
        assert!(!dest.exists());
        assert_no_exclusive_stage(temporary.path());
    }

    #[test]
    fn detailed_exclusive_publication_observation_one_race_and_cleanup_failure() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let dest = temporary.path().join("record.bin");
        let dest_for_barrier = dest.clone();
        let (result, consumed, fired) = run_with_bound_publication_faults_and_barrier(
            [(BoundPublicationPrimitive::StageUnlink, 1, Errno::EIO as i32)],
            BoundPublicationPrimitive::PostPublishObserve,
            1,
            move || fs::remove_file(&dest_for_barrier).unwrap(),
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert_eq!(consumed, 1);
        assert!(fired);
        let published = result.unwrap();
        assert!(matches!(
            published.final_name,
            FinalNameConfirmation::Unverified { .. }
        ));
        assert!(matches!(published.cleanup, StageCleanup::Retained { .. }));
        assert_eq!(exclusive_stage_names(temporary.path()).len(), 1);
    }

    #[test]
    fn detailed_exclusive_publication_durability_and_cleanup_failure() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let dest = temporary.path().join("record.bin");
        let (result, consumed) = run_with_bound_publication_faults(
            [
                (BoundPublicationPrimitive::StageUnlink, 1, Errno::EIO as i32),
                (BoundPublicationPrimitive::ParentSync, 1, Errno::EIO as i32),
            ],
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert_eq!(consumed, 2);
        let published = result.unwrap();
        assert!(matches!(
            published.final_name,
            FinalNameConfirmation::Confirmed { ref observation }
                if observation.bytes == EXCLUSIVE_PAYLOAD
        ));
        assert!(matches!(
            published.durability,
            MetadataDurability::Unproven { source: Some(_) }
        ));
        assert!(matches!(published.cleanup, StageCleanup::Retained { .. }));
        assert_eq!(fs::read(&dest).unwrap(), EXCLUSIVE_PAYLOAD);
        assert_eq!(exclusive_stage_names(temporary.path()).len(), 1);
    }

    #[test]
    fn detailed_exclusive_publication_all_three_facts_fail() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let dest = temporary.path().join("record.bin");
        let dest_for_barrier = dest.clone();
        let (result, consumed, fired) = run_with_bound_publication_faults_and_barrier(
            [
                (BoundPublicationPrimitive::StageUnlink, 1, Errno::EIO as i32),
                (BoundPublicationPrimitive::ParentSync, 1, Errno::EIO as i32),
            ],
            BoundPublicationPrimitive::PostPublishObserve,
            1,
            move || fs::remove_file(&dest_for_barrier).unwrap(),
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert_eq!(consumed, 2);
        assert!(fired);
        let published = result.unwrap();
        assert!(matches!(
            published.final_name,
            FinalNameConfirmation::Unverified { .. }
        ));
        assert!(matches!(
            published.durability,
            MetadataDurability::Unproven { source: Some(_) }
        ));
        assert!(matches!(published.cleanup, StageCleanup::Retained { .. }));
        assert_eq!(exclusive_stage_names(temporary.path()).len(), 1);
    }

    #[test]
    fn detailed_exclusive_publication_race_before_observation_two_is_unverified() {
        let temporary = TempDir::new();
        let directory = open_directory(temporary.path());
        let dest = temporary.path().join("record.bin");
        let dest_for_barrier = dest.clone();
        let (result, fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::CompletionObserve,
            1,
            move || fs::remove_file(&dest_for_barrier).unwrap(),
            || {
                write_bytes_exclusive_bound_detailed(
                    &directory,
                    OsStr::new("record.bin"),
                    EXCLUSIVE_PAYLOAD,
                    0o600,
                )
            },
        );
        assert!(fired);
        let published = result.unwrap();
        assert!(matches!(
            published.final_name,
            FinalNameConfirmation::Unverified { ref reason, .. }
                if reason.kind() == io::ErrorKind::NotFound
        ));
        assert!(matches!(
            published.durability,
            MetadataDurability::Confirmed
        ));
        assert!(matches!(published.cleanup, StageCleanup::Removed));
        assert!(!dest.exists());
        assert_no_exclusive_stage(temporary.path());
    }
}

#[cfg(test)]
mod generic_writers {
    use std::cell::Cell;
    use std::fs;
    use std::io;
    use std::rc::Rc;

    use serde::Serialize;
    use serde_json::json;

    use super::*;
    use crate::test_support::TempDir;

    fn assert_only_destination(parent: &Path, destination_name: &OsStr) {
        assert!(
            fs::read_dir(parent)
                .unwrap()
                .all(|entry| { entry.unwrap().file_name() == destination_name })
        );
    }

    struct CountedRecord {
        value: u8,
        serializations: Rc<Cell<usize>>,
    }

    impl Serialize for CountedRecord {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.serializations.set(self.serializations.get() + 1);
            self.value.serialize(serializer)
        }
    }

    struct FailOnSecond {
        index: usize,
    }

    impl Serialize for FailOnSecond {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            if self.index == 1 {
                return Err(serde::ser::Error::custom("intentional serialize failure"));
            }
            self.index.serialize(serializer)
        }
    }

    #[test]
    fn write_jsonl_publishes_an_empty_file_for_zero_records() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        write_jsonl(
            &path,
            std::iter::empty::<serde_json::Value>(),
            AtomicWriteOptions::default(),
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"");
        assert_only_destination(temporary.path(), path.file_name().unwrap());
    }

    #[test]
    fn write_jsonl_publishes_one_newline_terminated_record() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        write_jsonl(&path, [json!({"a": 1})], AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}\n");
    }

    #[test]
    fn write_jsonl_publishes_multiple_records_in_iterator_order() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        write_jsonl(
            &path,
            [json!({"a": 1}), json!({"b": 2})],
            AtomicWriteOptions::default(),
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}\n{\"b\":2}\n");
    }

    #[test]
    fn write_jsonl_creates_a_missing_nested_parent_directory() {
        let temporary = TempDir::new();
        let path = temporary.path().join("nested/a/b/records.jsonl");
        write_jsonl(&path, [json!({"a": 1})], AtomicWriteOptions::default()).unwrap();
        assert!(path.parent().unwrap().is_dir());
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}\n");
    }

    #[test]
    fn write_jsonl_serializes_each_record_exactly_once() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        let serializations = Rc::new(Cell::new(0));
        let records = [
            CountedRecord {
                value: 1,
                serializations: Rc::clone(&serializations),
            },
            CountedRecord {
                value: 2,
                serializations: Rc::clone(&serializations),
            },
            CountedRecord {
                value: 3,
                serializations: Rc::clone(&serializations),
            },
        ];
        write_jsonl(&path, records, AtomicWriteOptions::default()).unwrap();
        assert_eq!(serializations.get(), 3);
        assert_eq!(fs::read(&path).unwrap(), b"1\n2\n3\n");
    }

    #[test]
    fn write_jsonl_serialization_failure_does_not_publish_or_replace() {
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        fs::write(&path, b"keep-me-exactly").unwrap();
        let error = write_jsonl(
            &path,
            [
                FailOnSecond { index: 0 },
                FailOnSecond { index: 1 },
                FailOnSecond { index: 2 },
            ],
            AtomicWriteOptions::default(),
        )
        .expect_err("second record must fail to serialize");
        match error {
            AtomicWriteError::Io {
                path: error_path,
                source,
            } => {
                assert_eq!(error_path, path);
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
            }
            #[cfg(windows)]
            AtomicWriteError::PublicationUncertain { .. } => {
                panic!("serialization failure is pre-publication")
            }
        }
        assert_eq!(fs::read(&path).unwrap(), b"keep-me-exactly");
        assert_only_destination(temporary.path(), path.file_name().unwrap());
    }

    #[test]
    fn write_text_publishes_the_given_bytes_exactly() {
        let temporary = TempDir::new();
        let path = temporary.path().join("notes.txt");
        let text = "alpha\nbeta";
        write_text(&path, text, AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::io;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn detailed_publication_outcomes_map_to_success() {
        let path = PathBuf::from("unit.service");
        for outcome in [
            DetailedAtomicOutcome::Published,
            DetailedAtomicOutcome::PublishedDurabilityUncertain {
                source: io::Error::other("x"),
            },
            DetailedAtomicOutcome::PublishedParentPathRaced { sync_error: None },
            DetailedAtomicOutcome::PublishedParentPathUnverified {
                observation: io::Error::other("x"),
                sync_error: None,
            },
        ] {
            assert!(map_detailed_outcome(&path, Ok(outcome)).is_ok());
        }
    }

    #[test]
    fn detailed_prepublication_error_maps_to_atomic_write_error() {
        let path = PathBuf::from("unit.service");
        let error = map_detailed_outcome(
            &path,
            Err(DetailedAtomicError {
                path: path.clone(),
                operation: "test",
                source: io::Error::other("boom"),
                orphan_stage: None,
                cleanup_error: None,
            }),
        )
        .expect_err("pre-publication error maps to atomic write error");

        match error {
            AtomicWriteError::Io {
                path: error_path,
                source,
            } => {
                assert_eq!(error_path, path);
                assert_eq!(source.to_string(), "boom");
            }
            AtomicWriteError::PublicationUncertain { .. } => {
                panic!("detailed pre-publication errors map to Io")
            }
        }
    }
}
