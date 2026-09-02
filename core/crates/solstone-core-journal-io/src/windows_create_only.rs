// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows create-only exclusive publication. Separate from the replace backend.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Cursor, Read, Write};
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
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_DISPOSITION_INFO, FILE_READ_ATTRIBUTES, FileDispositionInfo, FlushFileBuffers,
    MoveFileExW, SYNCHRONIZE, SetFileInformationByHandle,
};

use super::{ATOMIC_CANDIDATE_MARKER, TEMP_SEQUENCE, io_error, publication_candidate_name};
use crate::create_only_retry::{
    CREATE_ONLY_MAX_ATTEMPTS, CreateOnlyMoveFailure, CreateOnlyReclass, CreateOnlyRetry,
    decide_create_only_retry,
};
use crate::errors::{AtomicWriteError, compose_exclusive_cleanup};
use crate::exclusive_copy::copy_exclusive;
use crate::windows_identity::{WindowsFileIdentity, file_identity};
use crate::windows_ntcreate::{nt_create_relative, nt_create_relative_share_read_delete};
use crate::windows_publication_path::{
    PublicationPathError, prepare_publication_path_with_terminals,
};

const PUBLICATION_RETRY_DELAY: Duration = Duration::from_millis(250);
const STAGE_CREATE_ATTEMPTS: usize = 100;
const STAGE_OPTIONS: u32 =
    FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
const STAGE_ACCESS: u32 = GENERIC_READ | GENERIC_WRITE | DELETE | SYNCHRONIZE;

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
            return Err(fail_with_cleanup(
                path,
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
            return Err(fail_with_cleanup(
                path,
                capability.terminal_parent(),
                &stage_name,
                None,
                source,
            ));
        }
    };

    for attempt in 1..=CREATE_ONLY_MAX_ATTEMPTS {
        if let Err(source) = admit_before_move(&capability, dest_name, &stage, stage_identity) {
            return Err(fail_with_cleanup(
                path,
                capability.terminal_parent(),
                &stage_name,
                Some(stage_identity),
                source,
            ));
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
                                thread::sleep(PUBLICATION_RETRY_DELAY);
                            }
                            CreateOnlyRetry::Retry { wait: false } => {}
                            CreateOnlyRetry::Stop => {
                                return Err(stop_after_move(
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
    Err(fail_with_cleanup(
        path,
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
    stage: &File,
    expected: WindowsFileIdentity,
) -> io::Result<()> {
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
    if capability.revalidate().is_err() {
        return CreateOnlyReclass::CapabilityChanged;
    }
    match destination_present(capability.terminal_parent(), dest_name) {
        Ok(true) => CreateOnlyReclass::DestinationOccupied,
        Err(_) => CreateOnlyReclass::Indeterminate,
        Ok(false) => match inspect_stage(capability.terminal_parent(), stage_name) {
            Ok(Some(identity)) if identity == expected => CreateOnlyReclass::StillHeld,
            Ok(None) => CreateOnlyReclass::StageMissing,
            Ok(Some(_)) | Err(_) => CreateOnlyReclass::Indeterminate,
        },
    }
}

fn stop_after_move(
    path: &Path,
    parent: &OwnedHandle,
    stage_name: &OsStr,
    expected: WindowsFileIdentity,
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
    fail_with_cleanup(path, parent, stage_name, Some(expected), source)
}

fn finish_published(
    path: &Path,
    capability: &crate::windows_publication_path::WindowsPublicationPath,
    dest_name: &OsStr,
    stage: &File,
    copied: u64,
) -> Result<u64, AtomicWriteError> {
    if let Err(source) = capability.revalidate() {
        return Err(AtomicWriteError::PublicationUncertain {
            path: path.to_path_buf(),
            operation: "revalidate publication path after move",
            source: io::Error::other(source),
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
    match destination_identity(capability.terminal_parent(), dest_name) {
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
            Ok(handle) => return Ok((stage_name, File::from(handle))),
            Err(error) if error.raw_os_error() == Some(ERROR_FILE_EXISTS as i32) => continue,
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
    match open_named(parent, name, FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
        Ok(_) => Ok(true),
        Err(error) if is_not_found(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn destination_identity(
    parent: &OwnedHandle,
    name: &OsStr,
) -> io::Result<Option<WindowsFileIdentity>> {
    match open_named(parent, name, FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
        Ok(handle) => Ok(Some(file_identity(handle.as_raw_handle())?)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn destination_matches(parent: &OwnedHandle, name: &OsStr, expected: WindowsFileIdentity) -> bool {
    matches!(destination_identity(parent, name), Ok(Some(identity)) if identity == expected)
}

fn inspect_stage(parent: &OwnedHandle, name: &OsStr) -> io::Result<Option<WindowsFileIdentity>> {
    destination_identity(parent, name)
}

fn stage_held(parent: &OwnedHandle, name: &OsStr, expected: WindowsFileIdentity) -> bool {
    matches!(inspect_stage(parent, name), Ok(Some(identity)) if identity == expected)
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
    parent: &OwnedHandle,
    stage_name: &OsStr,
    expected: Option<WindowsFileIdentity>,
    primary: io::Error,
) -> AtomicWriteError {
    let source = match expected {
        Some(expected) => cleanup_stage(parent, stage_name, expected, primary),
        None => match inspect_stage(parent, stage_name) {
            Ok(Some(identity)) => cleanup_stage(parent, stage_name, identity, primary),
            _ => primary,
        },
    };
    io_error(path, source)
}

fn cleanup_stage(
    parent: &OwnedHandle,
    stage_name: &OsStr,
    expected: WindowsFileIdentity,
    primary: io::Error,
) -> io::Error {
    match open_named(
        parent,
        stage_name,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
    ) {
        Ok(handle) => match file_identity(handle.as_raw_handle()) {
            Ok(identity) if identity == expected => match delete_by_handle(handle) {
                Ok(()) => primary,
                Err(cleanup) => compose_exclusive_cleanup(primary, stage_name, cleanup),
            },
            Ok(_) | Err(_) => primary,
        },
        Err(_) => primary,
    }
}

fn delete_by_handle(handle: OwnedHandle) -> io::Result<()> {
    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `handle` is owned and opened with DELETE; `disposition` is the exact
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
    drop(handle);
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
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
