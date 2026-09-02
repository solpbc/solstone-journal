// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows create-only exclusive publication. Separate from the replace backend.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_DIRECTORY, ERROR_FILE_EXISTS,
    ERROR_FILE_NOT_FOUND, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, GENERIC_READ,
    GENERIC_WRITE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_INFO, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
    FileAttributeTagInfo, FileDispositionInfo, FlushFileBuffers, GetFileInformationByHandle,
    GetFileInformationByHandleEx, MoveFileExW, SYNCHRONIZE, SetFileInformationByHandle,
};

use super::{
    ATOMIC_CANDIDATE_MARKER, DetailedAtomicError, ExclusivePublication, MetadataDurability,
    StageCleanup, TEMP_SEQUENCE, detailed_error, io_error, publication_candidate_name,
    select_exclusive_final_name,
};
use crate::create_only_retry::{
    CREATE_ONLY_MAX_ATTEMPTS, CreateOnlyMoveFailure, CreateOnlyReclass, CreateOnlyRetry,
    decide_create_only_retry,
};
use crate::errors::{AtomicWriteError, compose_exclusive_cleanup};
use crate::exclusive_copy::copy_exclusive;
use crate::journal_root::JournalEntryKind;
use crate::observation::{FileObservation, FlatDirectoryEntry, NativeMtime};
use crate::windows_identity::{WindowsFileIdentity, file_identity};
use crate::windows_ntcreate::{nt_create_relative, nt_create_relative_share_read_delete};
use crate::windows_publication_path::{
    PublicationPathError, prepare_publication_path_with_terminals,
};

const PUBLICATION_RETRY_DELAY: Duration = Duration::from_millis(250);
const STAGE_CREATE_ATTEMPTS: usize = 100;
const STAGE_OPTIONS: u32 =
    FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
// Destination observation must admit every occupied terminal kind. In particular,
// asking NtCreateFile for a non-directory handle to an existing directory maps
// STATUS_FILE_IS_A_DIRECTORY to ERROR_ACCESS_DENIED, which hides a create-only
// collision behind a permission error.
const ENTRY_OPTIONS: u32 = FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
const STAGE_ACCESS: u32 = GENERIC_READ | GENERIC_WRITE | DELETE | SYNCHRONIZE;

/// Checkpoints in the Windows create-only publication protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsCreateOnlyPrimitive {
    /// A continuously held stage has been created.
    StageReady,
    /// Full admission is about to run immediately before a move attempt.
    BeforeMove,
    /// A create-only move is about to call `MoveFileExW`.
    Move,
    /// Failed-move reclassification is about to revalidate the path capability.
    ReclassifyCapability,
    /// Failed-move reclassification is about to inspect the destination.
    ReclassifyDestination,
    /// Failed-move reclassification is about to inspect the stage name.
    ReclassifyStage,
    /// Failure cleanup is about to mark the held stage handle for deletion.
    Cleanup,
    /// Successful publication is about to revalidate the path capability.
    PostMoveCapability,
    /// Successful publication is about to observe the destination identity.
    PostMoveDestination,
}

/// Trace returned by the feature-gated Windows create-only test seam.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug)]
pub struct WindowsCreateOnlyTrace {
    /// Every checkpoint reached, in order.
    pub attempted: Vec<WindowsCreateOnlyPrimitive>,
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
struct WindowsCreateOnlyTraceState {
    attempted: Vec<WindowsCreateOnlyPrimitive>,
    real_moves: usize,
    backoffs: Vec<Duration>,
    faults: Vec<WindowsCreateOnlyFault>,
    barriers: Vec<WindowsCreateOnlyBarrier>,
}

#[cfg(any(test, feature = "test-hooks"))]
struct WindowsCreateOnlyFault {
    primitive: WindowsCreateOnlyPrimitive,
    ordinal: usize,
    raw_error: i32,
}

#[cfg(any(test, feature = "test-hooks"))]
struct WindowsCreateOnlyBarrier {
    primitive: WindowsCreateOnlyPrimitive,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static WINDOWS_CREATE_ONLY_TRACE: std::cell::RefCell<Option<WindowsCreateOnlyTraceState>> =
        const { std::cell::RefCell::new(None) };
}

/// Run one operation with injected create-only faults and a complete trace.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_create_only_faults<T>(
    faults: impl IntoIterator<Item = (WindowsCreateOnlyPrimitive, usize, i32)>,
    op: impl FnOnce() -> T,
) -> (T, WindowsCreateOnlyTrace) {
    start_trace(faults, std::iter::empty());
    let result = op();
    (result, finish_trace())
}

/// Run one operation with a deterministic create-only barrier and a complete trace.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_create_only_barrier<T>(
    primitive: WindowsCreateOnlyPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, WindowsCreateOnlyTrace) {
    start_trace(
        std::iter::empty(),
        [(primitive, ordinal, Box::new(callback) as Box<dyn FnOnce()>)],
    );
    let result = op();
    (result, finish_trace())
}

/// Run one operation with injected faults, one barrier, and a complete trace.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_create_only_faults_and_barrier<T>(
    faults: impl IntoIterator<Item = (WindowsCreateOnlyPrimitive, usize, i32)>,
    primitive: WindowsCreateOnlyPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, WindowsCreateOnlyTrace) {
    start_trace(
        faults,
        [(primitive, ordinal, Box::new(callback) as Box<dyn FnOnce()>)],
    );
    let result = op();
    (result, finish_trace())
}

#[cfg(any(test, feature = "test-hooks"))]
fn start_trace(
    faults: impl IntoIterator<Item = (WindowsCreateOnlyPrimitive, usize, i32)>,
    barriers: impl IntoIterator<Item = (WindowsCreateOnlyPrimitive, usize, Box<dyn FnOnce()>)>,
) {
    WINDOWS_CREATE_ONLY_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Windows create-only trace is already active"
        );
        *trace.borrow_mut() = Some(WindowsCreateOnlyTraceState {
            attempted: Vec::new(),
            real_moves: 0,
            backoffs: Vec::new(),
            faults: faults
                .into_iter()
                .map(|(primitive, ordinal, raw_error)| WindowsCreateOnlyFault {
                    primitive,
                    ordinal,
                    raw_error,
                })
                .collect(),
            barriers: barriers
                .into_iter()
                .map(|(primitive, ordinal, callback)| WindowsCreateOnlyBarrier {
                    primitive,
                    ordinal,
                    callback,
                })
                .collect(),
        });
    });
}

#[cfg(any(test, feature = "test-hooks"))]
fn finish_trace() -> WindowsCreateOnlyTrace {
    let state = WINDOWS_CREATE_ONLY_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("Windows create-only trace remains active")
    });
    WindowsCreateOnlyTrace {
        attempted: state.attempted,
        real_moves: state.real_moves,
        backoffs: state.backoffs,
        faults_consumed: state.faults.is_empty(),
        barriers_fired: state.barriers.is_empty(),
    }
}

pub(super) fn write_bytes_exclusive(
    path: &Path,
    contents: &[u8],
    options: crate::atomic::AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let mut reader = Cursor::new(contents);
    write_reader_exclusive(path, &mut reader, options).map(|_| ())
}

pub(super) fn write_reader_exclusive(
    path: &Path,
    reader: &mut impl Read,
    options: crate::atomic::AtomicWriteOptions,
) -> Result<u64, AtomicWriteError> {
    if let Some(mode) = options.mode
        && mode > 0o777
    {
        return Err(io_error(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "mode exceeds 0o777"),
        ));
    }
    let input = path.to_str().ok_or_else(|| {
        io_error(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "publication path is not valid UTF-8",
            ),
        )
    })?;
    let leaf = path.file_name().ok_or_else(|| {
        io_error(
            path,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        )
    })?;
    let stage_shape =
        publication_candidate_name(leaf, ATOMIC_CANDIDATE_MARKER, &[u128::MAX, u128::MAX]);
    let stage_shape = stage_shape.to_str().ok_or_else(|| {
        io_error(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "stage name is not valid UTF-8"),
        )
    })?;
    let capability = prepare_publication_path_with_terminals(input, &[stage_shape])
        .map_err(|error| map_prepare_error(path, error))?;
    let dest_name = capability.leaf_name();
    if destination_present(capability.terminal_parent(), dest_name)
        .map_err(|source| io_error(path, source))?
    {
        return Err(already_exists(path));
    }

    let (stage_name, mut stage) = allocate_stage(capability.terminal_parent(), dest_name)
        .map_err(|source| io_error(path, source))?;
    let copied = match copy_and_flush(reader, &mut stage) {
        Ok(copied) => copied,
        Err(source) => {
            return Err(fail_with_cleanup(path, &stage_name, &stage, source));
        }
    };
    let stage_identity = match file_identity(stage.as_raw_handle()) {
        Ok(identity) => identity,
        Err(source) => {
            return Err(fail_with_cleanup(path, &stage_name, &stage, source));
        }
    };

    for attempt in 1..=CREATE_ONLY_MAX_ATTEMPTS {
        if let Err(source) =
            admit_before_move(&capability, dest_name, &stage_name, &stage, stage_identity)
        {
            return Err(fail_with_cleanup(path, &stage_name, &stage, source));
        }
        match move_stage(&capability, &stage_name, dest_name) {
            Ok(()) => {
                return finish_published(path, &capability, dest_name, &stage, copied);
            }
            Err(source) => {
                match classify_after_failed_move(
                    &capability,
                    dest_name,
                    &stage_name,
                    stage_identity,
                    &source,
                ) {
                    AfterMove::Published => {
                        return finish_published(path, &capability, dest_name, &stage, copied);
                    }
                    AfterMove::Retry(failure, reclass) => {
                        match decide_create_only_retry(failure, reclass, attempt) {
                            CreateOnlyRetry::Retry { wait: true } => {
                                sleep_or_record_backoff(PUBLICATION_RETRY_DELAY);
                            }
                            CreateOnlyRetry::Retry { wait: false } => {}
                            CreateOnlyRetry::Stop => {
                                return Err(stop_after_move(
                                    path,
                                    &stage_name,
                                    &stage,
                                    failure,
                                    reclass,
                                    source,
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    Err(fail_with_cleanup(
        path,
        &stage_name,
        &stage,
        io::Error::new(io::ErrorKind::TimedOut, "create-only publication exhausted"),
    ))
}

pub(super) fn write_bytes_exclusive_detailed(
    path: &Path,
    contents: &[u8],
    options: crate::atomic::AtomicWriteOptions,
) -> Result<ExclusivePublication, DetailedAtomicError> {
    let mut reader = Cursor::new(contents);
    write_reader_exclusive_detailed(path, &mut reader, options)
}

pub(super) fn write_reader_exclusive_detailed(
    path: &Path,
    reader: &mut impl Read,
    options: crate::atomic::AtomicWriteOptions,
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
    let input = path.to_str().ok_or_else(|| {
        detailed_error(
            path,
            "validate destination",
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "publication path is not valid UTF-8",
            ),
        )
    })?;
    let leaf = path.file_name().ok_or_else(|| {
        detailed_error(
            path,
            "validate destination",
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        )
    })?;
    let stage_shape =
        publication_candidate_name(leaf, ATOMIC_CANDIDATE_MARKER, &[u128::MAX, u128::MAX]);
    let stage_shape = stage_shape.to_str().ok_or_else(|| {
        detailed_error(
            path,
            "validate destination",
            io::Error::new(io::ErrorKind::InvalidInput, "stage name is not valid UTF-8"),
        )
    })?;
    let capability = prepare_publication_path_with_terminals(input, &[stage_shape])
        .map_err(|error| map_prepare_error_detailed(path, error))?;
    let dest_name = capability.leaf_name().to_os_string();
    if destination_present(capability.terminal_parent(), &dest_name)
        .map_err(|source| detailed_error(path, "inspect destination", source))?
    {
        return Err(already_exists_detailed(path));
    }

    let (stage_name, mut stage) = allocate_stage(capability.terminal_parent(), &dest_name)
        .map_err(|source| detailed_error(path, "create stage", source))?;
    let copied = match copy_and_flush(reader, &mut stage) {
        Ok(copied) => copied,
        Err(source) => {
            return Err(fail_with_cleanup_detailed(
                path,
                "prepare stage",
                capability.terminal_parent(),
                &stage_name,
                None,
                source,
            ));
        }
    };
    let expected_bytes = match snapshot_stage_bytes(&mut stage) {
        Ok(bytes) => bytes,
        Err(source) => {
            return Err(fail_with_cleanup_detailed(
                path,
                "prepare stage",
                capability.terminal_parent(),
                &stage_name,
                None,
                source,
            ));
        }
    };
    let stage_identity = match file_identity(stage.as_raw_handle()) {
        Ok(identity) => identity,
        Err(source) => {
            return Err(fail_with_cleanup_detailed(
                path,
                "prepare stage",
                capability.terminal_parent(),
                &stage_name,
                None,
                source,
            ));
        }
    };

    for attempt in 1..=CREATE_ONLY_MAX_ATTEMPTS {
        if let Err(source) =
            admit_before_move(&capability, &dest_name, &stage_name, &stage, stage_identity)
        {
            return Err(fail_with_cleanup_detailed(
                path,
                "reverify before publication",
                capability.terminal_parent(),
                &stage_name,
                Some(stage_identity),
                source,
            ));
        }
        if let Err(source) = super::windows_atomic::checkpoint("exclusive-publish") {
            return Err(fail_with_cleanup_detailed(
                path,
                "publish stage",
                capability.terminal_parent(),
                &stage_name,
                Some(stage_identity),
                source,
            ));
        }
        if !stage_held(capability.terminal_parent(), &stage_name, stage_identity) {
            return Err(fail_with_cleanup_detailed(
                path,
                "verify stage",
                capability.terminal_parent(),
                &stage_name,
                Some(stage_identity),
                io::Error::other("stage identity changed before publication"),
            ));
        }
        match move_stage(&capability, &stage_name, &dest_name) {
            Ok(()) => {
                return finish_published_detailed(
                    path,
                    &capability,
                    &dest_name,
                    &stage_name,
                    &stage,
                    stage_identity,
                    &expected_bytes,
                    copied,
                );
            }
            Err(source) => {
                match classify_after_failed_move(
                    &capability,
                    &dest_name,
                    &stage_name,
                    stage_identity,
                    &source,
                ) {
                    AfterMove::Published => {
                        return finish_published_detailed(
                            path,
                            &capability,
                            &dest_name,
                            &stage_name,
                            &stage,
                            stage_identity,
                            &expected_bytes,
                            copied,
                        );
                    }
                    AfterMove::Retry(failure, reclass) => {
                        match decide_create_only_retry(failure, reclass, attempt) {
                            CreateOnlyRetry::Retry { wait: true } => {
                                thread::sleep(PUBLICATION_RETRY_DELAY);
                            }
                            CreateOnlyRetry::Retry { wait: false } => {}
                            CreateOnlyRetry::Stop => {
                                return Err(stop_after_move_detailed(
                                    path,
                                    capability.terminal_parent(),
                                    &stage_name,
                                    stage_identity,
                                    failure,
                                    reclass,
                                    source,
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    Err(fail_with_cleanup_detailed(
        path,
        "publish stage",
        capability.terminal_parent(),
        &stage_name,
        Some(stage_identity),
        io::Error::new(io::ErrorKind::TimedOut, "create-only publication exhausted"),
    ))
}

enum AfterMove {
    Published,
    Retry(CreateOnlyMoveFailure, CreateOnlyReclass),
}

fn copy_and_flush(reader: &mut impl Read, stage: &mut File) -> io::Result<u64> {
    let copied = copy_exclusive(reader, stage)?;
    stage.flush()?;
    flush_handle(stage.as_raw_handle())?;
    Ok(copied)
}

fn admit_before_move(
    capability: &crate::windows_publication_path::WindowsPublicationPath,
    dest_name: &OsStr,
    stage_name: &OsStr,
    stage: &File,
    expected: WindowsFileIdentity,
) -> io::Result<()> {
    checkpoint(WindowsCreateOnlyPrimitive::BeforeMove)?;
    capability.revalidate().map_err(io::Error::other)?;
    if destination_present(capability.terminal_parent(), dest_name)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    let observed = file_identity(stage.as_raw_handle())?;
    if observed != expected {
        return Err(io::Error::other(
            "stage identity changed before publication",
        ));
    }
    match inspect_stage(capability.terminal_parent(), stage_name)? {
        Some(identity) if identity == expected => {}
        Some(_) => {
            return Err(io::Error::other(
                "stage name identifies a different file before publication",
            ));
        }
        None => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "stage name is absent before publication",
            ));
        }
    }
    Ok(())
}

fn classify_move_error(error: &io::Error) -> CreateOnlyMoveFailure {
    match error.raw_os_error() {
        Some(code) if code == ERROR_SHARING_VIOLATION as i32 => {
            CreateOnlyMoveFailure::SharingViolation
        }
        Some(code) if code == ERROR_LOCK_VIOLATION as i32 => CreateOnlyMoveFailure::LockViolation,
        Some(code) if code == ERROR_ACCESS_DENIED as i32 => CreateOnlyMoveFailure::AccessDenied,
        Some(code) if code == ERROR_ALREADY_EXISTS as i32 || code == ERROR_FILE_EXISTS as i32 => {
            CreateOnlyMoveFailure::AlreadyExists
        }
        _ => CreateOnlyMoveFailure::Other,
    }
}

fn classify_after_failed_move(
    capability: &crate::windows_publication_path::WindowsPublicationPath,
    dest_name: &OsStr,
    stage_name: &OsStr,
    expected: WindowsFileIdentity,
    error: &io::Error,
) -> AfterMove {
    if destination_matches(capability.terminal_parent(), dest_name, expected)
        && !stage_held(capability.terminal_parent(), stage_name, expected)
    {
        return AfterMove::Published;
    }
    AfterMove::Retry(
        classify_move_error(error),
        reclassify(capability, dest_name, stage_name, expected),
    )
}

fn reclassify(
    capability: &crate::windows_publication_path::WindowsPublicationPath,
    dest_name: &OsStr,
    stage_name: &OsStr,
    expected: WindowsFileIdentity,
) -> CreateOnlyReclass {
    if checkpoint(WindowsCreateOnlyPrimitive::ReclassifyCapability)
        .and_then(|()| capability.revalidate().map_err(io::Error::other))
        .is_err()
    {
        return CreateOnlyReclass::CapabilityChanged;
    }
    match checkpoint(WindowsCreateOnlyPrimitive::ReclassifyDestination)
        .and_then(|()| destination_present(capability.terminal_parent(), dest_name))
    {
        Ok(true) => CreateOnlyReclass::DestinationOccupied,
        Err(_) => CreateOnlyReclass::Indeterminate,
        Ok(false) => match checkpoint(WindowsCreateOnlyPrimitive::ReclassifyStage)
            .and_then(|()| inspect_stage(capability.terminal_parent(), stage_name))
        {
            Ok(Some(identity)) if identity == expected => CreateOnlyReclass::StillHeld,
            Ok(None) => CreateOnlyReclass::StageMissing,
            Ok(Some(_)) | Err(_) => CreateOnlyReclass::Indeterminate,
        },
    }
}

fn stop_after_move(
    path: &Path,
    stage_name: &OsStr,
    stage: &File,
    failure: CreateOnlyMoveFailure,
    reclass: CreateOnlyReclass,
    source: io::Error,
) -> AtomicWriteError {
    let source = if failure == CreateOnlyMoveFailure::AlreadyExists
        || reclass == CreateOnlyReclass::DestinationOccupied
    {
        io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists")
    } else {
        source
    };
    fail_with_cleanup(path, stage_name, stage, source)
}

fn finish_published(
    path: &Path,
    capability: &crate::windows_publication_path::WindowsPublicationPath,
    dest_name: &OsStr,
    stage: &File,
    copied: u64,
) -> Result<u64, AtomicWriteError> {
    if let Err(source) = checkpoint(WindowsCreateOnlyPrimitive::PostMoveCapability)
        .and_then(|()| capability.revalidate().map_err(io::Error::other))
    {
        return Err(AtomicWriteError::PublicationUncertain {
            path: path.to_path_buf(),
            operation: "revalidate publication path after move",
            source,
        });
    }
    let live = match file_identity(stage.as_raw_handle()) {
        Ok(identity) => identity,
        Err(source) => {
            return Err(AtomicWriteError::PublicationUncertain {
                path: path.to_path_buf(),
                operation: "observe published destination after move",
                source,
            });
        }
    };
    match checkpoint(WindowsCreateOnlyPrimitive::PostMoveDestination)
        .and_then(|()| destination_identity(capability.terminal_parent(), dest_name))
    {
        Ok(Some(identity)) if identity == live => Ok(copied),
        Ok(Some(_)) => Err(AtomicWriteError::PublicationUncertain {
            path: path.to_path_buf(),
            operation: "observe published destination after move",
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "published destination identity does not match the live stage",
            ),
        }),
        Ok(None) => Err(AtomicWriteError::PublicationUncertain {
            path: path.to_path_buf(),
            operation: "observe published destination after move",
            source: io::Error::new(
                io::ErrorKind::NotFound,
                "published destination name is absent",
            ),
        }),
        Err(source) => Err(AtomicWriteError::PublicationUncertain {
            path: path.to_path_buf(),
            operation: "observe published destination after move",
            source,
        }),
    }
}

fn allocate_stage(parent: &OwnedHandle, destination: &OsStr) -> io::Result<(OsString, File)> {
    for _ in 0..STAGE_CREATE_ATTEMPTS {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let stage_name = publication_candidate_name(
            destination,
            ATOMIC_CANDIDATE_MARKER,
            &[u128::from(std::process::id()), u128::from(sequence)],
        );
        match nt_create_relative_share_read_delete(
            parent.as_raw_handle(),
            &stage_name,
            STAGE_ACCESS,
            FILE_CREATE,
            STAGE_OPTIONS,
        ) {
            Ok(handle) => {
                let stage = File::from(handle);
                checkpoint(WindowsCreateOnlyPrimitive::StageReady)?;
                return Ok((stage_name, stage));
            }
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(code)
                        if code == ERROR_FILE_EXISTS as i32
                            || code == ERROR_ALREADY_EXISTS as i32
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate stage",
    ))
}

fn move_stage(
    capability: &crate::windows_publication_path::WindowsPublicationPath,
    stage_name: &OsStr,
    dest_name: &OsStr,
) -> io::Result<()> {
    let source = join_move_spelling(capability.move_spelling(), stage_name)?;
    let destination = join_move_spelling(capability.move_spelling(), dest_name)?;
    checkpoint(WindowsCreateOnlyPrimitive::Move)?;
    record_real_move();
    // SAFETY: both buffers are NUL-terminated and remain live for the synchronous call.
    #[allow(unsafe_code)]
    let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn join_move_spelling(parent: &OsStr, name: &OsStr) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = parent.encode_wide().collect();
    if wide.contains(&0) || name.encode_wide().any(|unit| unit == 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "move spelling contains an interior NUL",
        ));
    }
    while matches!(wide.last(), Some(&unit) if unit == u16::from(b'\\') || unit == u16::from(b'/'))
    {
        wide.pop();
    }
    wide.push(u16::from(b'\\'));
    wide.extend(name.encode_wide());
    wide.push(0);
    Ok(wide)
}

fn destination_present(parent: &OwnedHandle, name: &OsStr) -> io::Result<bool> {
    match open_destination(parent, name, FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
        Ok(_) => Ok(true),
        Err(error) if error.raw_os_error() == Some(ERROR_DIRECTORY as i32) => Ok(true),
        Err(error) if is_not_found(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn destination_identity(
    parent: &OwnedHandle,
    name: &OsStr,
) -> io::Result<Option<WindowsFileIdentity>> {
    match open_destination(parent, name, FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
        Ok(handle) => Ok(Some(file_identity(handle.as_raw_handle())?)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn destination_matches(parent: &OwnedHandle, name: &OsStr, expected: WindowsFileIdentity) -> bool {
    matches!(destination_identity(parent, name), Ok(Some(identity)) if identity == expected)
}

fn inspect_stage(parent: &OwnedHandle, name: &OsStr) -> io::Result<Option<WindowsFileIdentity>> {
    match open_named(parent, name, FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
        Ok(handle) => Ok(Some(file_identity(handle.as_raw_handle())?)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn stage_held(parent: &OwnedHandle, name: &OsStr, expected: WindowsFileIdentity) -> bool {
    matches!(inspect_stage(parent, name), Ok(Some(identity)) if identity == expected)
}

fn open_destination(parent: &OwnedHandle, name: &OsStr, access: u32) -> io::Result<OwnedHandle> {
    nt_create_relative(
        parent.as_raw_handle(),
        name,
        access,
        FILE_OPEN,
        ENTRY_OPTIONS,
    )
}

fn open_named(parent: &OwnedHandle, name: &OsStr, access: u32) -> io::Result<OwnedHandle> {
    nt_create_relative(
        parent.as_raw_handle(),
        name,
        access,
        FILE_OPEN,
        STAGE_OPTIONS,
    )
}

fn is_not_found(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32)
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
    stage_name: &OsStr,
    stage: &File,
    primary: io::Error,
) -> AtomicWriteError {
    let source = cleanup_stage(stage, stage_name, primary);
    io_error(path, source)
}

fn cleanup_stage(stage: &File, stage_name: &OsStr, primary: io::Error) -> io::Error {
    match delete_by_handle(stage) {
        Ok(()) => primary,
        Err(cleanup) => compose_exclusive_cleanup(primary, stage_name, cleanup),
    }
}

fn delete_by_handle(handle: &impl AsRawHandle) -> io::Result<()> {
    checkpoint(WindowsCreateOnlyPrimitive::Cleanup)?;
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
fn checkpoint(primitive: WindowsCreateOnlyPrimitive) -> io::Result<()> {
    let (fault, barrier) = WINDOWS_CREATE_ONLY_TRACE.with(|trace| {
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
fn checkpoint(_primitive: WindowsCreateOnlyPrimitive) -> io::Result<()> {
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
fn record_real_move() {
    WINDOWS_CREATE_ONLY_TRACE.with(|trace| {
        if let Some(state) = trace.borrow_mut().as_mut() {
            state.real_moves += 1;
        }
    });
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn record_real_move() {}

#[cfg(any(test, feature = "test-hooks"))]
fn sleep_or_record_backoff(delay: Duration) {
    let recorded = WINDOWS_CREATE_ONLY_TRACE.with(|trace| {
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
    let kind = if matches!(error, PublicationPathError::PathTooLong) {
        io::ErrorKind::InvalidInput
    } else {
        io::ErrorKind::Other
    };
    io_error(path, io::Error::new(kind, error))
}

fn already_exists(path: &Path) -> AtomicWriteError {
    io_error(
        path,
        io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
    )
}

fn already_exists_detailed(path: &Path) -> DetailedAtomicError {
    detailed_error(
        path,
        "inspect destination",
        io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
    )
}

fn map_prepare_error_detailed(path: &Path, error: PublicationPathError) -> DetailedAtomicError {
    let kind = if matches!(error, PublicationPathError::PathTooLong) {
        io::ErrorKind::InvalidInput
    } else {
        io::ErrorKind::Other
    };
    detailed_error(
        path,
        "prepare publication path",
        io::Error::new(kind, error),
    )
}

fn snapshot_stage_bytes(stage: &mut File) -> io::Result<Vec<u8>> {
    stage.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    stage.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn stop_after_move_detailed(
    path: &Path,
    parent: &OwnedHandle,
    stage_name: &OsStr,
    expected: WindowsFileIdentity,
    failure: CreateOnlyMoveFailure,
    reclass: CreateOnlyReclass,
    source: io::Error,
) -> DetailedAtomicError {
    let source = if failure == CreateOnlyMoveFailure::AlreadyExists
        || reclass == CreateOnlyReclass::DestinationOccupied
    {
        io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists")
    } else {
        source
    };
    fail_with_cleanup_detailed(
        path,
        "publish stage",
        parent,
        stage_name,
        Some(expected),
        source,
    )
}

fn fail_with_cleanup_detailed(
    path: &Path,
    operation: &'static str,
    parent: &OwnedHandle,
    stage_name: &OsStr,
    expected: Option<WindowsFileIdentity>,
    primary: io::Error,
) -> DetailedAtomicError {
    let cleanup = match expected {
        Some(expected) => try_cleanup_stage(parent, stage_name, expected),
        None => match inspect_stage(parent, stage_name) {
            Ok(Some(identity)) => try_cleanup_stage(parent, stage_name, identity),
            _ => Ok(()),
        },
    };
    match cleanup {
        Ok(()) => detailed_error(path, operation, primary),
        Err(cleanup_error) => DetailedAtomicError {
            path: path.to_path_buf(),
            operation,
            source: primary,
            orphan_stage: Some(stage_name.to_os_string()),
            cleanup_error: Some(cleanup_error),
        },
    }
}

fn try_cleanup_stage(
    parent: &OwnedHandle,
    stage_name: &OsStr,
    expected: WindowsFileIdentity,
) -> io::Result<()> {
    match open_named(
        parent,
        stage_name,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
    ) {
        Ok(handle) => match file_identity(handle.as_raw_handle()) {
            Ok(identity) if identity == expected => delete_by_handle(&handle),
            Ok(_) | Err(_) => Ok(()),
        },
        Err(_) => Ok(()),
    }
}

/// Assemble the three post-publication facts after a create-only move has landed.
///
/// A successful `MoveFileExW(..., 0)` is a true rename: the stage spelling and
/// the destination spelling are never simultaneously live. Cleanup therefore
/// probes the *original* stage spelling and treats `ERROR_FILE_NOT_FOUND` as
/// [`StageCleanup::Removed`]. Directory-entry durability is always
/// [`MetadataDurability::Unproven`] with `source: None`; there is no documented
/// flush primitive on the retained parent handle.
#[allow(clippy::too_many_arguments)]
fn finish_published_detailed(
    path: &Path,
    capability: &crate::windows_publication_path::WindowsPublicationPath,
    dest_name: &OsStr,
    stage_name: &OsStr,
    stage: &File,
    stage_identity: WindowsFileIdentity,
    expected_bytes: &[u8],
    copied: u64,
) -> Result<ExclusivePublication, DetailedAtomicError> {
    let first = match capability.revalidate() {
        Err(source) => {
            let _ = super::windows_atomic::checkpoint("exclusive-observe-1");
            Err(io::Error::other(source))
        }
        Ok(()) => match super::windows_atomic::checkpoint("exclusive-observe-1") {
            Err(source) => Err(source),
            Ok(()) => observe_exclusive_dest(
                capability.terminal_parent(),
                dest_name,
                stage,
                expected_bytes,
            ),
        },
    };

    let cleanup =
        dispose_stage_name_if_still_ours(capability.terminal_parent(), stage_name, stage_identity);
    let durability = MetadataDurability::Unproven { source: None };

    let second = match super::windows_atomic::checkpoint("exclusive-observe-2") {
        Err(source) => Err(source),
        Ok(()) => observe_exclusive_dest(
            capability.terminal_parent(),
            dest_name,
            stage,
            expected_bytes,
        ),
    };

    Ok(ExclusivePublication {
        bytes_written: copied,
        final_name: select_exclusive_final_name(path.to_path_buf(), first, second),
        durability,
        cleanup,
    })
}

fn observe_exclusive_dest(
    parent: &OwnedHandle,
    dest_name: &OsStr,
    stage: &File,
    expected_bytes: &[u8],
) -> io::Result<FileObservation> {
    let live = file_identity(stage.as_raw_handle())?;
    let handle = open_destination(
        parent,
        dest_name,
        FILE_READ_ATTRIBUTES | FILE_READ_DATA | SYNCHRONIZE,
    )?;
    let identity = file_identity(handle.as_raw_handle())?;
    if identity != live {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "published destination identity does not match the live stage",
        ));
    }
    let entry = exclusive_entry_from_handle(dest_name.to_os_string(), handle.as_raw_handle())?;
    let mut file = File::from(handle);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes != expected_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "published destination bytes changed before verification",
        ));
    }
    Ok(FileObservation { entry, bytes })
}

const WINDOWS_TO_UNIX_EPOCH_100NS: i128 = 116_444_736_000_000_000;
const HUNDRED_NANOSECONDS_PER_SECOND: i128 = 10_000_000;

// Intentionally self-contained: windows_sync_dir::entry_from_handle/native_mtime
// return FlatDirectoryError, classify reparse/directory as kinds rather than
// refusing them, and are module-private.
fn exclusive_entry_from_handle(
    name: OsString,
    handle: RawHandle,
) -> io::Result<FlatDirectoryEntry> {
    let attributes = exclusive_attribute_tag(handle)?;
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("entry is a reparse point"));
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(io::Error::other("entry is not a regular file"));
    }
    let identity = file_identity(handle)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `info` is writable for its exact size and `handle` remains valid.
    #[allow(unsafe_code)]
    let result = unsafe { GetFileInformationByHandle(handle, &mut info) };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    let last_write = ((info.ftLastWriteTime.dwHighDateTime as u64) << 32)
        | u64::from(info.ftLastWriteTime.dwLowDateTime);
    Ok(FlatDirectoryEntry {
        name,
        kind: JournalEntryKind::RegularFile,
        device: identity.volume_serial(),
        inode: identity.folded_file_id(),
        size: ((info.nFileSizeHigh as u64) << 32) | u64::from(info.nFileSizeLow),
        mtime: exclusive_native_mtime(last_write)?,
    })
}

fn exclusive_attribute_tag(handle: RawHandle) -> io::Result<FILE_ATTRIBUTE_TAG_INFO> {
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
        .ok_or_else(io::Error::last_os_error)
}

fn exclusive_native_mtime(value: u64) -> io::Result<NativeMtime> {
    let ticks = i128::from(value) - WINDOWS_TO_UNIX_EPOCH_100NS;
    let seconds = ticks.div_euclid(HUNDRED_NANOSECONDS_PER_SECOND);
    let nanoseconds = ticks.rem_euclid(HUNDRED_NANOSECONDS_PER_SECOND) * 100;
    Ok(NativeMtime {
        seconds: i64::try_from(seconds).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "modification time exceeds i64")
        })?,
        nanoseconds: i64::try_from(nanoseconds).expect("Windows FILETIME remainder fits i64"),
    })
}

/// Probe the original stage spelling after a create-only rename.
///
/// On a clean `MoveFileExW` success the stage name is the dest name, so this
/// probe returns `ERROR_FILE_NOT_FOUND` and reports [`StageCleanup::Removed`].
/// [`StageCleanup::Retained`] only happens when the spelling still resolves to
/// this invocation's file ID (unreachable on a real clean rename; tests plant
/// an extra hard link) or when the probe/delete fails with any error other
/// than not-found — an inconclusive probe cannot certify the name is clear.
fn dispose_stage_name_if_still_ours(
    parent: &OwnedHandle,
    stage_name: &OsStr,
    expected: WindowsFileIdentity,
) -> StageCleanup {
    if let Err(source) = super::windows_atomic::checkpoint("stage-cleanup") {
        return StageCleanup::Retained {
            stage: stage_name.to_os_string(),
            source,
        };
    }
    match open_named(
        parent,
        stage_name,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
    ) {
        Err(error) if is_not_found(&error) => StageCleanup::Removed,
        Err(error) => StageCleanup::Retained {
            stage: stage_name.to_os_string(),
            source: error,
        },
        Ok(handle) => match file_identity(handle.as_raw_handle()) {
            Ok(identity) if identity == expected => match delete_by_handle(&handle) {
                Ok(()) => StageCleanup::Removed,
                Err(source) => StageCleanup::Retained {
                    stage: stage_name.to_os_string(),
                    source,
                },
            },
            Ok(_) | Err(_) => StageCleanup::Removed,
        },
    }
}
