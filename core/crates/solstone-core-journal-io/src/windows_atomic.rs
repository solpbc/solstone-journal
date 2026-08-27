// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Descriptor-bound detailed publication for Windows.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
    ERROR_USER_MAPPED_FILE, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_READ_DATA, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FileAttributeTagInfo, FileDispositionInfo, FileIdInfo, FileRenameInfo, FlushFileBuffers,
    GetFileInformationByHandleEx, OPEN_EXISTING, SYNCHRONIZE, SetFileInformationByHandle,
};

use super::{
    ATOMIC_CANDIDATE_MARKER, DetailedAtomicError, DetailedAtomicOutcome, TEMP_SEQUENCE,
    detailed_error, pause_at, publication_candidate_name,
};
use crate::windows_ntcreate::nt_create_relative;

const MAX_PUBLICATION_ATTEMPTS: usize = 3;
const PUBLICATION_RETRY_DELAY: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowsFileIdentity {
    volume_serial: u64,
    file_id: [u8; 16],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DestinationState {
    Absent,
    Present(WindowsFileIdentity),
}

/// Atomically replace one regular destination beneath an existing real parent.
pub(super) fn atomic_replace_detailed(
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
    let parent_path = path
        .parent()
        .filter(|item| !item.as_os_str().is_empty())
        .ok_or_else(|| {
            detailed_error(
                path,
                "validate destination",
                io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"),
            )
        })?;
    let destination_name = normal_name(path.file_name()).ok_or_else(|| {
        detailed_error(
            path,
            "validate destination",
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no normal name",
            ),
        )
    })?;

    let (parent, parent_identity) = open_parent_no_follow(parent_path)
        .map_err(|source| detailed_error(path, "open parent", source))?;
    let initial_destination = inspect_relative_destination(&parent, destination_name)
        .map_err(|source| detailed_error(path, "inspect destination", source))?;
    checkpoint("temp-create").map_err(|source| detailed_error(path, "create stage", source))?;
    let (stage_name, mut stage_writer, stage_identity) = allocate_stage(&parent, destination_name)
        .map_err(|source| detailed_error(path, "create stage", source))?;
    pause_at("temp-create");

    let prepared = (|| -> io::Result<()> {
        checkpoint("write")?;
        stage_writer.write_all(contents)?;
        pause_at("write");
        super::apply_mode(&stage_writer, mode)?;
        checkpoint("fsync-file")?;
        flush_handle(stage_writer.as_raw_handle())?;
        pause_at("fsync-file");
        validate_regular_handle(stage_writer.as_raw_handle())?;
        if file_identity(stage_writer.as_raw_handle())? != stage_identity {
            return Err(io::Error::other("stage identity changed while preparing"));
        }
        Ok(())
    })();
    if let Err(source) = prepared {
        drop(stage_writer);
        return Err(cleanup_stage_error(
            path,
            &parent,
            &stage_name,
            stage_identity,
            "prepare stage",
            source,
        ));
    }
    drop(stage_writer);
    checkpoint("close").map_err(|source| {
        cleanup_stage_error(
            path,
            &parent,
            &stage_name,
            stage_identity,
            "close stage",
            source,
        )
    })?;
    pause_at("close");

    let stage =
        open_stage_for_publication(&parent, &stage_name, stage_identity).map_err(|source| {
            cleanup_stage_error(
                path,
                &parent,
                &stage_name,
                stage_identity,
                "reopen stage",
                source,
            )
        })?;

    let mut stage = Some(stage);
    for attempt in 0..MAX_PUBLICATION_ATTEMPTS {
        let stage_handle = stage
            .as_ref()
            .expect("stage remains live until publication");
        if let Err(source) = revalidate_before_publication(
            parent_path,
            &parent,
            parent_identity,
            stage_handle,
            &stage_name,
            stage_identity,
            destination_name,
            initial_destination,
        ) {
            return Err(cleanup_stage_handle_error(
                path,
                stage.take().expect("stage remains live before publication"),
                stage_name,
                "reverify before publication",
                source,
            ));
        }

        match publish_stage(stage_handle, &parent, destination_name) {
            Ok(()) => break,
            Err(source)
                if is_retryable_publication_error(&source)
                    && attempt + 1 < MAX_PUBLICATION_ATTEMPTS =>
            {
                thread::sleep(PUBLICATION_RETRY_DELAY);
            }
            Err(source) => {
                return Err(cleanup_stage_handle_error(
                    path,
                    stage.take().expect("stage remains live before publication"),
                    stage_name,
                    "publish stage",
                    source,
                ));
            }
        }
    }
    drop(stage.take());

    let sync_error = match checkpoint("fsync-bound-parent-dir") {
        Ok(()) => flush_handle(parent.as_raw_handle()).err(),
        Err(source) => Some(source),
    };
    pause_at("fsync-bound-parent-dir");

    if let Err(observation) =
        observe_published_destination(&parent, destination_name, stage_identity, contents)
    {
        return Ok(DetailedAtomicOutcome::PublishedParentPathUnverified {
            observation,
            sync_error,
        });
    }
    pause_at("post-publication-observation");

    match open_parent_no_follow(parent_path) {
        Ok((_, identity)) if identity == parent_identity => match sync_error {
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

fn normal_name(value: Option<&OsStr>) -> Option<&OsStr> {
    let value = value?;
    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(component)), None) if component == value => Some(value),
        _ => None,
    }
}

fn open_parent_no_follow(path: &Path) -> io::Result<(OwnedHandle, WindowsFileIdentity)> {
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: `wide` is NUL-terminated and remains live through the synchronous CreateFileW call.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW returned one valid owned handle, converted exactly once.
    #[allow(unsafe_code)]
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    validate_directory_handle(handle.as_raw_handle())?;
    let identity = file_identity(handle.as_raw_handle())?;
    Ok((handle, identity))
}

fn inspect_relative_destination(
    parent: &OwnedHandle,
    name: &OsStr,
) -> io::Result<DestinationState> {
    match open_existing_relative(parent, name, FILE_READ_ATTRIBUTES | SYNCHRONIZE) {
        Ok(handle) => {
            validate_regular_handle(handle.as_raw_handle())?;
            Ok(DestinationState::Present(file_identity(
                handle.as_raw_handle(),
            )?))
        }
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
            Ok(DestinationState::Absent)
        }
        Err(error) => Err(error),
    }
}

fn allocate_stage(
    parent: &OwnedHandle,
    destination: &OsStr,
) -> io::Result<(OsString, File, WindowsFileIdentity)> {
    for _ in 0..100 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let stage_name = publication_candidate_name(
            destination,
            ATOMIC_CANDIDATE_MARKER,
            &[u128::from(std::process::id()), u128::from(sequence)],
        );
        match nt_create_relative(
            parent.as_raw_handle(),
            &stage_name,
            GENERIC_READ | GENERIC_WRITE | DELETE | SYNCHRONIZE,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        ) {
            Ok(handle) => {
                validate_regular_handle(handle.as_raw_handle())?;
                let identity = file_identity(handle.as_raw_handle())?;
                return Ok((stage_name, File::from(handle), identity));
            }
            Err(error) if error.raw_os_error() == Some(ERROR_FILE_EXISTS as i32) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate stage",
    ))
}

fn open_stage_for_publication(
    parent: &OwnedHandle,
    stage_name: &OsStr,
    expected_identity: WindowsFileIdentity,
) -> io::Result<OwnedHandle> {
    let handle = open_existing_relative(
        parent,
        stage_name,
        DELETE | FILE_READ_ATTRIBUTES | FILE_READ_DATA | SYNCHRONIZE,
    )?;
    validate_regular_handle(handle.as_raw_handle())?;
    if file_identity(handle.as_raw_handle())? != expected_identity {
        return Err(io::Error::other(
            "stage namespace changed before publication",
        ));
    }
    Ok(handle)
}

#[allow(clippy::too_many_arguments)]
fn revalidate_before_publication(
    parent_path: &Path,
    parent: &OwnedHandle,
    parent_identity: WindowsFileIdentity,
    stage: &OwnedHandle,
    stage_name: &OsStr,
    stage_identity: WindowsFileIdentity,
    destination_name: &OsStr,
    initial_destination: DestinationState,
) -> io::Result<()> {
    validate_directory_handle(parent.as_raw_handle())?;
    if file_identity(parent.as_raw_handle())? != parent_identity {
        return Err(io::Error::other(
            "bound parent identity changed before publication",
        ));
    }
    let (_, current_parent) = open_parent_no_follow(parent_path)?;
    if current_parent != parent_identity {
        return Err(io::Error::other(
            "parent pathname changed before publication",
        ));
    }
    validate_regular_handle(stage.as_raw_handle())?;
    if file_identity(stage.as_raw_handle())? != stage_identity {
        return Err(io::Error::other(
            "stage identity changed before publication",
        ));
    }
    let reopened_stage = open_stage_for_publication(parent, stage_name, stage_identity)?;
    drop(reopened_stage);
    if inspect_relative_destination(parent, destination_name)? != initial_destination {
        return Err(io::Error::other(
            "destination namespace changed before publication",
        ));
    }
    checkpoint("pre-publication-validation")?;
    pause_at("pre-publication-validation");
    Ok(())
}

fn publish_stage(stage: &OwnedHandle, parent: &OwnedHandle, destination: &OsStr) -> io::Result<()> {
    checkpoint("rename")?;
    set_rename_information(stage.as_raw_handle(), parent.as_raw_handle(), destination)?;
    pause_at("rename");
    Ok(())
}

fn observe_published_destination(
    parent: &OwnedHandle,
    destination: &OsStr,
    expected_identity: WindowsFileIdentity,
    expected_contents: &[u8],
) -> io::Result<()> {
    checkpoint("post-publication-observation")?;
    let handle = open_existing_relative(
        parent,
        destination,
        FILE_READ_ATTRIBUTES | FILE_READ_DATA | SYNCHRONIZE,
    )?;
    validate_regular_handle(handle.as_raw_handle())?;
    if file_identity(handle.as_raw_handle())? != expected_identity {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "published destination identity changed before verification",
        ));
    }
    let mut file = File::from(handle);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes != expected_contents {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "published destination bytes changed before verification",
        ));
    }
    Ok(())
}

fn cleanup_stage_error(
    path: &Path,
    parent: &OwnedHandle,
    stage_name: &OsStr,
    stage_identity: WindowsFileIdentity,
    operation: &'static str,
    source: io::Error,
) -> DetailedAtomicError {
    let cleanup = open_stage_for_publication(parent, stage_name, stage_identity)
        .and_then(cleanup_stage_handle)
        .err();
    DetailedAtomicError {
        path: path.to_path_buf(),
        operation,
        source,
        orphan_stage: cleanup.as_ref().map(|_| stage_name.to_os_string()),
        cleanup_error: cleanup,
    }
}

fn cleanup_stage_handle_error(
    path: &Path,
    stage: OwnedHandle,
    stage_name: OsString,
    operation: &'static str,
    source: io::Error,
) -> DetailedAtomicError {
    let cleanup = cleanup_stage_handle(stage).err();
    DetailedAtomicError {
        path: path.to_path_buf(),
        operation,
        source,
        orphan_stage: cleanup.as_ref().map(|_| stage_name),
        cleanup_error: cleanup,
    }
}

fn cleanup_stage_handle(stage: OwnedHandle) -> io::Result<()> {
    checkpoint("cleanup")?;
    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `stage` is an owned file handle opened with DELETE access; `disposition`
    // is initialized for the exact FileDispositionInfo buffer size and remains live for the call.
    #[allow(unsafe_code)]
    let result = unsafe {
        SetFileInformationByHandle(
            stage.as_raw_handle(),
            FileDispositionInfo,
            (&mut disposition as *mut FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    drop(stage);
    pause_at("cleanup");
    Ok(())
}

fn open_existing_relative(
    parent: &OwnedHandle,
    name: &OsStr,
    desired_access: u32,
) -> io::Result<OwnedHandle> {
    nt_create_relative(
        parent.as_raw_handle(),
        name,
        desired_access,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    )
}

fn validate_directory_handle(handle: RawHandle) -> io::Result<()> {
    let attributes = attribute_tag(handle)?;
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("parent is a reparse point"));
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(io::Error::other("parent is not a real directory"));
    }
    Ok(())
}

fn validate_regular_handle(handle: RawHandle) -> io::Result<()> {
    let attributes = attribute_tag(handle)?;
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("entry is a reparse point"));
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(io::Error::other("entry is not a regular file"));
    }
    Ok(())
}

fn attribute_tag(handle: RawHandle) -> io::Result<FILE_ATTRIBUTE_TAG_INFO> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: `info` is writable for its exact buffer size and `handle` remains valid
    // for the synchronous GetFileInformationByHandleEx call.
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

fn file_identity(handle: RawHandle) -> io::Result<WindowsFileIdentity> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `info` is writable for its exact buffer size and `handle` remains valid
    // for the synchronous GetFileInformationByHandleEx call.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    (result != 0)
        .then_some(WindowsFileIdentity {
            volume_serial: info.VolumeSerialNumber,
            file_id: info.FileId.Identifier,
        })
        .ok_or_else(io::Error::last_os_error)
}

fn flush_handle(handle: RawHandle) -> io::Result<()> {
    // SAFETY: `handle` remains valid for the synchronous FlushFileBuffers call.
    #[allow(unsafe_code)]
    let result = unsafe { FlushFileBuffers(handle) };
    (result != 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)
}

fn set_rename_information(
    stage: RawHandle,
    parent: RawHandle,
    destination: &OsStr,
) -> io::Result<()> {
    let name = destination.encode_wide().collect::<Vec<_>>();
    let name_bytes = name.len().checked_mul(size_of::<u16>()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination name is too long")
    })?;
    let buffer_size = size_of::<FILE_RENAME_INFO>()
        .checked_add(name_bytes)
        .and_then(|size| size.checked_sub(size_of::<u16>()))
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination name is too long")
        })?;
    let buffer_size_u32 = u32::try_from(buffer_size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "destination name is too long"))?;
    let word_size = size_of::<usize>();
    let words = buffer_size
        .checked_add(word_size - 1)
        .and_then(|size| size.checked_div(word_size))
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination name is too long")
        })?;
    let mut storage = vec![0usize; words];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `storage` is aligned for FILE_RENAME_INFO, has at least `buffer_size` initialized
    // bytes, and remains live through SetFileInformationByHandle. The variable-length UTF-16
    // name fits after the FILE_RENAME_INFO tail member; both handles remain valid for the call.
    #[allow(unsafe_code)]
    let result = unsafe {
        (*info).Anonymous.ReplaceIfExists = true;
        (*info).RootDirectory = parent;
        (*info).FileNameLength = u32::try_from(name_bytes).expect("checked UTF-16 name length");
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            name.len(),
        );
        SetFileInformationByHandle(stage, FileRenameInfo, info.cast(), buffer_size_u32)
    };
    (result != 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)
}

fn is_retryable_publication_error(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code)
            if code == ERROR_SHARING_VIOLATION as i32
                || code == ERROR_LOCK_VIOLATION as i32
                || code == ERROR_USER_MAPPED_FILE as i32
    )
}

#[cfg(any(test, feature = "test-hooks"))]
struct WindowsPublicationTraceState {
    attempted: Vec<&'static str>,
    faults: Vec<WindowsPublicationFault>,
    barriers: Vec<WindowsPublicationBarrier>,
}

#[cfg(any(test, feature = "test-hooks"))]
struct WindowsPublicationFault {
    step: &'static str,
    ordinal: usize,
    raw_error: i32,
}

#[cfg(any(test, feature = "test-hooks"))]
struct WindowsPublicationBarrier {
    step: &'static str,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static WINDOWS_PUBLICATION_TRACE: std::cell::RefCell<Option<WindowsPublicationTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Inject Windows publication failures and return every checkpoint reached by `op`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_detailed_atomic_faults<T>(
    faults: impl IntoIterator<Item = (&'static str, usize, i32)>,
    op: impl FnOnce() -> T,
) -> (T, Vec<&'static str>) {
    WINDOWS_PUBLICATION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Windows detailed publication trace is already active"
        );
        *trace.borrow_mut() = Some(WindowsPublicationTraceState {
            attempted: Vec::new(),
            faults: faults
                .into_iter()
                .map(|(step, ordinal, raw_error)| WindowsPublicationFault {
                    step,
                    ordinal,
                    raw_error,
                })
                .collect(),
            barriers: Vec::new(),
        });
    });
    let result = op();
    let state = WINDOWS_PUBLICATION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("Windows detailed publication trace remains active")
    });
    (result, state.attempted)
}

/// Run `op` with one deterministic Windows detailed-publication barrier callback.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_detailed_atomic_barrier<T>(
    step: &'static str,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    WINDOWS_PUBLICATION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Windows detailed publication trace is already active"
        );
        *trace.borrow_mut() = Some(WindowsPublicationTraceState {
            attempted: Vec::new(),
            faults: Vec::new(),
            barriers: vec![WindowsPublicationBarrier {
                step,
                ordinal,
                callback: Box::new(callback),
            }],
        });
    });
    let result = op();
    let state = WINDOWS_PUBLICATION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("Windows detailed publication trace remains active")
    });
    (result, state.barriers.is_empty())
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint(step: &'static str) -> io::Result<()> {
    let (fault, barrier) = WINDOWS_PUBLICATION_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return (None, None);
        };
        state.attempted.push(step);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == step)
            .count();
        let fault = state
            .faults
            .iter()
            .position(|fault| fault.step == step && fault.ordinal == ordinal)
            .map(|index| state.faults.remove(index).raw_error);
        if fault.is_some() {
            return (fault, None);
        }
        let barrier = state
            .barriers
            .iter()
            .position(|barrier| barrier.step == step && barrier.ordinal == ordinal)
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
fn checkpoint(_step: &'static str) -> io::Result<()> {
    Ok(())
}
