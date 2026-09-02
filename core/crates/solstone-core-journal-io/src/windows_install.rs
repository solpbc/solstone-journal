// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows install_file: publish an already-created caller-owned source by replace-move.

use std::fs::File;
use std::io::{self};
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use std::thread;
use std::time::Duration;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND, ERROR_LOCK_VIOLATION,
    ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION, GENERIC_WRITE,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_DISPOSITION_INFO, FILE_READ_ATTRIBUTES, FileAttributeTagInfo, FileDispositionInfo,
    FlushFileBuffers, GetFileInformationByHandleEx, MOVEFILE_REPLACE_EXISTING, MoveFileExW,
    SYNCHRONIZE, SetFileInformationByHandle,
};

use super::io_error;
use crate::errors::{AtomicWriteError, compose_exclusive_cleanup};
use crate::install_retry::{
    INSTALL_MAX_ATTEMPTS, InstallDestinationClass, InstallMoveFailure, InstallRetryDecision,
    InstallSourceClass, classify_install_names, decide_install_retry,
};
use crate::windows_identity::{WindowsFileIdentity, file_identity};
use crate::windows_ntcreate::{nt_create_relative, nt_create_relative_share_read_delete};
use crate::windows_publication_path::{
    AdmittedPublicationPath, AncestorAdmission, PublicationPathError, WindowsPublicationPath,
    admit_publication_path, leaf_move_spelling, parse_publication_path,
};

const INSTALL_RETRY_DELAY: Duration = Duration::from_millis(250);
const SOURCE_ACCESS: u32 = FILE_READ_ATTRIBUTES | GENERIC_WRITE | DELETE | SYNCHRONIZE;
const SOURCE_OPTIONS: u32 =
    FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
const PROBE_ACCESS: u32 = FILE_READ_ATTRIBUTES | SYNCHRONIZE;

/// Checkpoints in the Windows install_file publication protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsInstallPrimitive {
    /// The source leaf has been opened and its identity frozen.
    SourceReady,
    /// About to flush the retained source handle.
    Flush,
    /// Full admission is about to run immediately before a move attempt.
    BeforeMove,
    /// An install move is about to call `MoveFileExW`.
    Move,
    /// Failed-move reclassification is about to revalidate both path capabilities.
    ReclassifyCapability,
    /// Failed-move reclassification is about to inspect the source name.
    ReclassifySource,
    /// Failed-move reclassification is about to inspect the destination name.
    ReclassifyDestination,
    /// Failure cleanup is about to mark the held source handle for deletion.
    Cleanup,
    /// Successful publication is about to revalidate both path capabilities.
    PostMoveCapability,
    /// Successful publication is about to observe the destination identity.
    PostMoveDestination,
}

/// Trace returned by the feature-gated Windows install_file test seam.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug)]
pub struct WindowsInstallTrace {
    /// Every checkpoint reached, in order.
    pub attempted: Vec<WindowsInstallPrimitive>,
    /// Calls that crossed the injection seam and invoked the real move API.
    pub real_moves: usize,
    /// Requested retry delays, recorded without sleeping.
    pub backoffs: Vec<Duration>,
    /// True only when every requested injected fault was consumed.
    pub faults_consumed: bool,
    /// True only when every requested barrier fired.
    pub barriers_fired: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
struct WindowsInstallTraceState {
    attempted: Vec<WindowsInstallPrimitive>,
    real_moves: usize,
    backoffs: Vec<Duration>,
    faults: Vec<WindowsInstallFault>,
    barriers: Vec<WindowsInstallBarrier>,
}

#[cfg(any(test, feature = "test-hooks"))]
struct WindowsInstallFault {
    primitive: WindowsInstallPrimitive,
    ordinal: usize,
    raw_error: i32,
}

#[cfg(any(test, feature = "test-hooks"))]
struct WindowsInstallBarrier {
    primitive: WindowsInstallPrimitive,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static WINDOWS_INSTALL_TRACE: std::cell::RefCell<Option<WindowsInstallTraceState>> =
        const { std::cell::RefCell::new(None) };
}

/// Run one operation with injected install faults and a complete trace.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_install_faults<T>(
    faults: impl IntoIterator<Item = (WindowsInstallPrimitive, usize, i32)>,
    op: impl FnOnce() -> T,
) -> (T, WindowsInstallTrace) {
    start_trace(faults, std::iter::empty());
    let result = op();
    (result, finish_trace())
}

/// Run one operation with a deterministic install barrier and a complete trace.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_install_barrier<T>(
    primitive: WindowsInstallPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, WindowsInstallTrace) {
    start_trace(
        std::iter::empty(),
        [(primitive, ordinal, Box::new(callback) as Box<dyn FnOnce()>)],
    );
    let result = op();
    (result, finish_trace())
}

/// Run one operation with injected faults, one barrier, and a complete trace.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_install_faults_and_barrier<T>(
    faults: impl IntoIterator<Item = (WindowsInstallPrimitive, usize, i32)>,
    primitive: WindowsInstallPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, WindowsInstallTrace) {
    start_trace(
        faults,
        [(primitive, ordinal, Box::new(callback) as Box<dyn FnOnce()>)],
    );
    let result = op();
    (result, finish_trace())
}

#[cfg(any(test, feature = "test-hooks"))]
fn start_trace(
    faults: impl IntoIterator<Item = (WindowsInstallPrimitive, usize, i32)>,
    barriers: impl IntoIterator<Item = (WindowsInstallPrimitive, usize, Box<dyn FnOnce()>)>,
) {
    WINDOWS_INSTALL_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Windows install trace is already active"
        );
        *trace.borrow_mut() = Some(WindowsInstallTraceState {
            attempted: Vec::new(),
            real_moves: 0,
            backoffs: Vec::new(),
            faults: faults
                .into_iter()
                .map(|(primitive, ordinal, raw_error)| WindowsInstallFault {
                    primitive,
                    ordinal,
                    raw_error,
                })
                .collect(),
            barriers: barriers
                .into_iter()
                .map(|(primitive, ordinal, callback)| WindowsInstallBarrier {
                    primitive,
                    ordinal,
                    callback,
                })
                .collect(),
        });
    });
}

#[cfg(any(test, feature = "test-hooks"))]
fn finish_trace() -> WindowsInstallTrace {
    let state = WINDOWS_INSTALL_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("Windows install trace remains active")
    });
    WindowsInstallTrace {
        attempted: state.attempted,
        real_moves: state.real_moves,
        backoffs: state.backoffs,
        faults_consumed: state.faults.is_empty(),
        barriers_fired: state.barriers.is_empty(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DestinationState {
    Absent,
    Regular(WindowsFileIdentity),
}

enum DestObservation {
    Absent,
    Regular(WindowsFileIdentity),
    Directory,
    Reparse,
}

enum PreMoveFailure {
    Cleanup(io::Error),
    AliasNoCleanup(io::Error),
}

pub(super) fn install_file(
    temporary_path: &Path,
    path: &Path,
    options: crate::atomic::AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    if let Some(mode) = options.mode
        && mode > 0o777
    {
        return Err(io_error(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    let dest_str = path.to_str().ok_or_else(|| {
        io_error(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "publication path is not valid UTF-8",
            ),
        )
    })?;
    let src_str = temporary_path.to_str().ok_or_else(|| {
        io_error(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "publication path is not valid UTF-8",
            ),
        )
    })?;
    if path.file_name().is_none() {
        return Err(io_error(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        ));
    }
    if temporary_path.file_name().is_none() {
        return Err(io_error(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "source has no normal name"),
        ));
    }
    parse_publication_path(src_str).map_err(|error| io_error(path, io::Error::other(error)))?;
    parse_publication_path(dest_str).map_err(|error| io_error(path, io::Error::other(error)))?;

    let dest_admitted: AdmittedPublicationPath =
        admit_publication_path(dest_str, &[]).map_err(|error| map_prepare_error(path, error))?;
    let src_admitted: AdmittedPublicationPath =
        admit_publication_path(src_str, &[]).map_err(|error| map_prepare_error(path, error))?;
    let same_volume = src_admitted.has_same_volume_root(&dest_admitted);
    let src_cap = src_admitted
        .retain_ancestors(AncestorAdmission::ExistingOnly)
        .map_err(|error| map_prepare_error(path, error))?;

    let source_file = match open_source_leaf(&src_cap) {
        Ok(file) => file,
        Err(error) => return Err(map_prepare_error(path, error)),
    };
    if let Err(source) = checkpoint(WindowsInstallPrimitive::SourceReady) {
        return Err(fail_with_cleanup(
            path,
            src_cap.leaf_name(),
            &source_file,
            source,
        ));
    }
    let source_identity = match file_identity(source_file.as_raw_handle()) {
        Ok(identity) => identity,
        Err(source) => {
            return Err(fail_with_cleanup(
                path,
                src_cap.leaf_name(),
                &source_file,
                source,
            ));
        }
    };

    if !same_volume {
        return Err(fail_with_cleanup(
            path,
            src_cap.leaf_name(),
            &source_file,
            io::Error::other("source and destination are on different volumes"),
        ));
    }

    let dest_cap = match dest_admitted.retain_ancestors(AncestorAdmission::CreateMissing) {
        Ok(capability) => capability,
        Err(error) => {
            return Err(fail_with_cleanup(
                path,
                src_cap.leaf_name(),
                &source_file,
                prepare_io_error(error),
            ));
        }
    };

    if src_cap.terminal_parent_identity() == dest_cap.terminal_parent_identity()
        && src_cap.leaf_name() == dest_cap.leaf_name()
    {
        return Err(io_error(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "source and destination name the same file",
            ),
        ));
    }

    let dest_state = match inspect_leaf(dest_cap.terminal_parent(), dest_cap.leaf_name()) {
        Ok(DestObservation::Absent) => DestinationState::Absent,
        Ok(DestObservation::Regular(identity)) if identity == source_identity => {
            return Err(io_error(
                path,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination already identifies the source file",
                ),
            ));
        }
        Ok(DestObservation::Regular(identity)) => DestinationState::Regular(identity),
        Ok(DestObservation::Directory) => {
            return Err(fail_with_cleanup(
                path,
                src_cap.leaf_name(),
                &source_file,
                io::Error::other("destination is not a regular file"),
            ));
        }
        Ok(DestObservation::Reparse) => {
            return Err(fail_with_cleanup(
                path,
                src_cap.leaf_name(),
                &source_file,
                io::Error::other("destination is a reparse point"),
            ));
        }
        Err(source) => {
            return Err(fail_with_cleanup(
                path,
                src_cap.leaf_name(),
                &source_file,
                source,
            ));
        }
    };

    if let Err(source) = checkpoint(WindowsInstallPrimitive::Flush)
        .and_then(|()| flush_handle(source_file.as_raw_handle()))
    {
        return Err(fail_with_cleanup(
            path,
            src_cap.leaf_name(),
            &source_file,
            source,
        ));
    }

    for attempt in 1..=INSTALL_MAX_ATTEMPTS {
        match admit_before_move(
            &src_cap,
            &dest_cap,
            &source_file,
            source_identity,
            dest_state,
        ) {
            Ok(()) => {}
            Err(PreMoveFailure::Cleanup(source)) => {
                return Err(fail_with_cleanup(
                    path,
                    src_cap.leaf_name(),
                    &source_file,
                    source,
                ));
            }
            Err(PreMoveFailure::AliasNoCleanup(source)) => return Err(io_error(path, source)),
        }
        match move_source(&src_cap, &dest_cap) {
            Ok(()) => {
                return finish_published(path, &src_cap, &dest_cap, &source_file);
            }
            Err(os_error) => {
                let failure = classify_move_error(&os_error);
                let reclass = match reclassify(&src_cap, &dest_cap, source_identity, dest_state) {
                    Ok(reclass) => reclass,
                    Err(source) => {
                        return Err(AtomicWriteError::PublicationUncertain {
                            path: path.to_path_buf(),
                            operation: "reconcile install after failed move",
                            source,
                        });
                    }
                };
                match decide_install_retry(failure, reclass, attempt) {
                    InstallRetryDecision::Retry { wait: true } => {
                        sleep_or_record_backoff(INSTALL_RETRY_DELAY);
                    }
                    InstallRetryDecision::Retry { wait: false } => {}
                    InstallRetryDecision::Landed => {
                        return finish_published(path, &src_cap, &dest_cap, &source_file);
                    }
                    InstallRetryDecision::StopCleanup => {
                        return Err(fail_with_cleanup(
                            path,
                            src_cap.leaf_name(),
                            &source_file,
                            os_error,
                        ));
                    }
                    InstallRetryDecision::StopUncertain => {
                        return Err(AtomicWriteError::PublicationUncertain {
                            path: path.to_path_buf(),
                            operation: "reconcile install after failed move",
                            source: os_error,
                        });
                    }
                }
            }
        }
    }
    Err(fail_with_cleanup(
        path,
        src_cap.leaf_name(),
        &source_file,
        io::Error::new(io::ErrorKind::TimedOut, "install publication exhausted"),
    ))
}

fn admit_before_move(
    source_cap: &WindowsPublicationPath,
    dest_cap: &WindowsPublicationPath,
    source_file: &File,
    source_identity: WindowsFileIdentity,
    dest_state: DestinationState,
) -> Result<(), PreMoveFailure> {
    checkpoint(WindowsInstallPrimitive::BeforeMove).map_err(PreMoveFailure::Cleanup)?;
    source_cap
        .revalidate()
        .map_err(|error| PreMoveFailure::Cleanup(io::Error::other(error)))?;
    dest_cap
        .revalidate()
        .map_err(|error| PreMoveFailure::Cleanup(io::Error::other(error)))?;

    let live = file_identity(source_file.as_raw_handle()).map_err(PreMoveFailure::Cleanup)?;
    if live != source_identity {
        return Err(PreMoveFailure::Cleanup(io::Error::other(
            "source identity changed before publication",
        )));
    }

    let source_name = inspect_leaf(source_cap.terminal_parent(), source_cap.leaf_name())
        .map_err(PreMoveFailure::Cleanup)?;
    let dest_name = inspect_leaf(dest_cap.terminal_parent(), dest_cap.leaf_name())
        .map_err(PreMoveFailure::Cleanup)?;

    if let DestObservation::Regular(identity) = dest_name
        && identity == source_identity
    {
        return Err(PreMoveFailure::AliasNoCleanup(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination already identifies the source file",
        )));
    }

    let source_retained =
        matches!(source_name, DestObservation::Regular(identity) if identity == source_identity);
    if !source_retained {
        if dest_matches_admitted(&dest_name, dest_state) {
            return Err(PreMoveFailure::Cleanup(io::Error::new(
                io::ErrorKind::NotFound,
                "source name is absent before publication",
            )));
        }
        return Err(PreMoveFailure::Cleanup(io::Error::other(
            "source name changed before publication",
        )));
    }

    if dest_matches_admitted(&dest_name, dest_state) {
        return Ok(());
    }
    Err(PreMoveFailure::Cleanup(io::Error::other(
        "destination changed during publication",
    )))
}

fn dest_matches_admitted(observed: &DestObservation, admitted: DestinationState) -> bool {
    match (admitted, observed) {
        (DestinationState::Absent, DestObservation::Absent) => true,
        (DestinationState::Regular(expected), DestObservation::Regular(identity))
            if expected == *identity =>
        {
            true
        }
        _ => false,
    }
}

fn classify_move_error(error: &io::Error) -> InstallMoveFailure {
    match error.raw_os_error() {
        Some(code) if code == ERROR_SHARING_VIOLATION as i32 => {
            InstallMoveFailure::SharingViolation
        }
        Some(code) if code == ERROR_LOCK_VIOLATION as i32 => InstallMoveFailure::LockViolation,
        Some(code) if code == ERROR_ACCESS_DENIED as i32 => {
            InstallMoveFailure::MoveOriginAccessDenied
        }
        _ => InstallMoveFailure::Other,
    }
}

fn reclassify(
    source_cap: &WindowsPublicationPath,
    dest_cap: &WindowsPublicationPath,
    source_identity: WindowsFileIdentity,
    dest_state: DestinationState,
) -> io::Result<crate::install_retry::InstallReclass> {
    checkpoint(WindowsInstallPrimitive::ReclassifyCapability)?;
    source_cap.revalidate().map_err(io::Error::other)?;
    dest_cap.revalidate().map_err(io::Error::other)?;
    let source = checkpoint(WindowsInstallPrimitive::ReclassifySource)
        .and_then(|()| inspect_leaf(source_cap.terminal_parent(), source_cap.leaf_name()))?;
    let dest = checkpoint(WindowsInstallPrimitive::ReclassifyDestination)
        .and_then(|()| inspect_leaf(dest_cap.terminal_parent(), dest_cap.leaf_name()))?;
    Ok(classify_install_names(
        Some(source_class(source, source_identity)),
        Some(destination_class(dest, dest_state, source_identity)),
        true,
    ))
}

fn source_class(
    observed: DestObservation,
    source_identity: WindowsFileIdentity,
) -> InstallSourceClass {
    match observed {
        DestObservation::Regular(identity) if identity == source_identity => {
            InstallSourceClass::Retained
        }
        DestObservation::Absent => InstallSourceClass::Absent,
        DestObservation::Regular(_) | DestObservation::Directory | DestObservation::Reparse => {
            InstallSourceClass::Different
        }
    }
}

fn destination_class(
    observed: DestObservation,
    dest_state: DestinationState,
    source_identity: WindowsFileIdentity,
) -> InstallDestinationClass {
    match observed {
        DestObservation::Absent => InstallDestinationClass::Absent,
        DestObservation::Regular(identity) if identity == source_identity => {
            InstallDestinationClass::IsSource
        }
        DestObservation::Regular(identity) => match dest_state {
            DestinationState::Regular(admitted) if admitted == identity => {
                InstallDestinationClass::Admitted
            }
            _ => InstallDestinationClass::Other,
        },
        DestObservation::Directory | DestObservation::Reparse => InstallDestinationClass::Other,
    }
}

fn finish_published(
    path: &Path,
    source_cap: &WindowsPublicationPath,
    dest_cap: &WindowsPublicationPath,
    source_file: &File,
) -> Result<(), AtomicWriteError> {
    if let Err(source) = checkpoint(WindowsInstallPrimitive::PostMoveCapability)
        .and_then(|()| source_cap.revalidate().map_err(io::Error::other))
        .and_then(|()| dest_cap.revalidate().map_err(io::Error::other))
    {
        return Err(AtomicWriteError::PublicationUncertain {
            path: path.to_path_buf(),
            operation: "revalidate install paths after move",
            source,
        });
    }
    let live = match file_identity(source_file.as_raw_handle()) {
        Ok(identity) => identity,
        Err(source) => {
            return Err(AtomicWriteError::PublicationUncertain {
                path: path.to_path_buf(),
                operation: "observe installed destination after move",
                source,
            });
        }
    };
    match checkpoint(WindowsInstallPrimitive::PostMoveDestination)
        .and_then(|()| inspect_leaf(dest_cap.terminal_parent(), dest_cap.leaf_name()))
    {
        Ok(DestObservation::Regular(identity)) if identity == live => Ok(()),
        Ok(DestObservation::Regular(_)) => Err(AtomicWriteError::PublicationUncertain {
            path: path.to_path_buf(),
            operation: "observe installed destination after move",
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "published destination identity does not match the live source",
            ),
        }),
        Ok(DestObservation::Absent) => Err(AtomicWriteError::PublicationUncertain {
            path: path.to_path_buf(),
            operation: "observe installed destination after move",
            source: io::Error::new(
                io::ErrorKind::NotFound,
                "published destination name is absent",
            ),
        }),
        Ok(DestObservation::Directory | DestObservation::Reparse) => {
            Err(AtomicWriteError::PublicationUncertain {
                path: path.to_path_buf(),
                operation: "observe installed destination after move",
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "published destination is not a regular file",
                ),
            })
        }
        Err(source) => Err(AtomicWriteError::PublicationUncertain {
            path: path.to_path_buf(),
            operation: "observe installed destination after move",
            source,
        }),
    }
}

fn open_source_leaf(capability: &WindowsPublicationPath) -> Result<File, PublicationPathError> {
    let name = capability.leaf_name();
    let component = name.to_string_lossy().into_owned();
    match nt_create_relative_share_read_delete(
        capability.terminal_parent().as_raw_handle(),
        name,
        SOURCE_ACCESS,
        FILE_OPEN,
        SOURCE_OPTIONS,
    ) {
        Ok(handle) => {
            validate_regular_leaf(handle.as_raw_handle(), &component)?;
            Ok(File::from(handle))
        }
        Err(source) if is_not_found(&source) => Err(PublicationPathError::Missing { component }),
        Err(source) if is_not_directory(&source) => {
            Err(PublicationPathError::NotRegularFile { component })
        }
        Err(source) => Err(PublicationPathError::Io {
            operation: "open install source",
            source,
        }),
    }
}

fn validate_regular_leaf(
    handle: RawHandle,
    component: &str,
) -> Result<WindowsFileIdentity, PublicationPathError> {
    let attributes = attribute_tag(handle)?;
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(PublicationPathError::ReparsePoint {
            component: component.to_owned(),
        });
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(PublicationPathError::NotRegularFile {
            component: component.to_owned(),
        });
    }
    file_identity(handle).map_err(|source| PublicationPathError::Io {
        operation: "query install source identity",
        source,
    })
}

fn move_source(
    source_cap: &WindowsPublicationPath,
    dest_cap: &WindowsPublicationPath,
) -> io::Result<()> {
    let source = leaf_move_spelling(source_cap.move_spelling(), source_cap.leaf_name())?;
    let destination = leaf_move_spelling(dest_cap.move_spelling(), dest_cap.leaf_name())?;
    checkpoint(WindowsInstallPrimitive::Move)?;
    record_real_move();
    // SAFETY: both buffers are NUL-terminated and remain live for the synchronous call.
    #[allow(unsafe_code)]
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn inspect_leaf(parent: &OwnedHandle, name: &std::ffi::OsStr) -> io::Result<DestObservation> {
    match open_probe(parent, name) {
        Ok(handle) => classify_probe(handle),
        Err(error) if is_not_found(&error) => Ok(DestObservation::Absent),
        Err(error) if is_not_directory(&error) => Ok(DestObservation::Directory),
        Err(error) => Err(error),
    }
}

fn classify_probe(handle: OwnedHandle) -> io::Result<DestObservation> {
    let attributes = attribute_tag(handle.as_raw_handle()).map_err(|error| match error {
        PublicationPathError::Io { source, .. } => source,
        other => io::Error::other(other),
    })?;
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Ok(DestObservation::Reparse);
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Ok(DestObservation::Directory);
    }
    Ok(DestObservation::Regular(file_identity(
        handle.as_raw_handle(),
    )?))
}

fn open_probe(parent: &OwnedHandle, name: &std::ffi::OsStr) -> io::Result<OwnedHandle> {
    nt_create_relative(
        parent.as_raw_handle(),
        name,
        PROBE_ACCESS,
        FILE_OPEN,
        SOURCE_OPTIONS,
    )
}

fn attribute_tag(handle: RawHandle) -> Result<FILE_ATTRIBUTE_TAG_INFO, PublicationPathError> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: `info` is writable for its exact buffer size and `handle` remains valid.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    (result != 0)
        .then_some(info)
        .ok_or_else(|| PublicationPathError::Io {
            operation: "query install path attributes",
            source: io::Error::last_os_error(),
        })
}

fn is_not_found(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code)
            if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
    )
}

fn is_not_directory(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ERROR_DIRECTORY as i32)
}

fn flush_handle(handle: RawHandle) -> io::Result<()> {
    // SAFETY: `handle` remains valid for the synchronous FlushFileBuffers call.
    #[allow(unsafe_code)]
    let result = unsafe { FlushFileBuffers(handle) };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn fail_with_cleanup(
    path: &Path,
    source_name: &std::ffi::OsStr,
    source: &File,
    primary: io::Error,
) -> AtomicWriteError {
    let source = cleanup_source(source, source_name, primary);
    io_error(path, source)
}

fn cleanup_source(source: &File, source_name: &std::ffi::OsStr, primary: io::Error) -> io::Error {
    match delete_by_handle(source) {
        Ok(()) => primary,
        Err(cleanup) => compose_exclusive_cleanup(primary, source_name, cleanup),
    }
}

fn delete_by_handle(handle: &impl AsRawHandle) -> io::Result<()> {
    checkpoint(WindowsInstallPrimitive::Cleanup)?;
    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `handle` remains live and was opened with DELETE; `disposition` is the exact
    // FileDispositionInfo buffer and remains live for the synchronous call.
    #[allow(unsafe_code)]
    let result = unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfo,
            (&mut disposition as *mut FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint(primitive: WindowsInstallPrimitive) -> io::Result<()> {
    let (fault, barrier) = WINDOWS_INSTALL_TRACE.with(|trace| {
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
        let fault = state
            .faults
            .iter()
            .position(|fault| fault.primitive == primitive && fault.ordinal == ordinal)
            .map(|index| state.faults.remove(index).raw_error);
        if fault.is_some() {
            return (fault, None);
        }
        let barrier = state
            .barriers
            .iter()
            .position(|barrier| barrier.primitive == primitive && barrier.ordinal == ordinal)
            .map(|index| state.barriers.remove(index).callback);
        (None, barrier)
    });
    if let Some(raw_error) = fault {
        return Err(io::Error::from_raw_os_error(raw_error));
    }
    if let Some(callback) = barrier {
        callback();
    }
    Ok(())
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn checkpoint(_primitive: WindowsInstallPrimitive) -> io::Result<()> {
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
fn record_real_move() {
    WINDOWS_INSTALL_TRACE.with(|trace| {
        if let Some(state) = trace.borrow_mut().as_mut() {
            state.real_moves += 1;
        }
    });
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn record_real_move() {}

#[cfg(any(test, feature = "test-hooks"))]
fn sleep_or_record_backoff(delay: Duration) {
    let recorded = WINDOWS_INSTALL_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return false;
        };
        state.backoffs.push(delay);
        true
    });
    if !recorded {
        thread::sleep(delay);
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn sleep_or_record_backoff(delay: Duration) {
    thread::sleep(delay);
}

fn map_prepare_error(path: &Path, error: PublicationPathError) -> AtomicWriteError {
    io_error(path, prepare_io_error(error))
}

fn prepare_io_error(error: PublicationPathError) -> io::Error {
    let kind = match &error {
        PublicationPathError::PathTooLong => io::ErrorKind::InvalidInput,
        PublicationPathError::Missing { .. } => io::ErrorKind::NotFound,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}
