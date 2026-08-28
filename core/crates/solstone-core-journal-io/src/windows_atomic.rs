// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows detailed publication with descriptor-bound admission and path-based replacement.

use std::ffi::{OsStr, OsString};
#[cfg(any(test, feature = "test-hooks"))]
use std::fs;
use std::fs::File;
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
#[cfg(any(test, feature = "test-hooks"))]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_LOCK_VIOLATION,
    ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo,
    FileDispositionInfo, FlushFileBuffers, GetFileInformationByHandleEx, MOVEFILE_REPLACE_EXISTING,
    MoveFileExW, OPEN_EXISTING, SYNCHRONIZE, SetFileInformationByHandle,
};

use super::{
    ATOMIC_CANDIDATE_MARKER, DetailedAtomicError, DetailedAtomicOutcome, TEMP_SEQUENCE,
    detailed_error, pause_at, publication_candidate_name,
};
use crate::windows_identity::{WindowsFileIdentity, file_identity};
use crate::windows_ntcreate::nt_create_relative;

const MAX_PUBLICATION_ATTEMPTS: usize = 4;
const PUBLICATION_RETRY_DELAY: Duration = Duration::from_millis(250);

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
    let (stage_name, mut stage_writer) = allocate_stage(&parent, destination_name)
        .map_err(|source| detailed_error(path, "create stage", source))?;
    let stage_identity = match validate_regular_handle(stage_writer.as_raw_handle())
        .and_then(|()| file_identity(stage_writer.as_raw_handle()))
    {
        Ok(identity) => identity,
        Err(source) => {
            return Err(cleanup_stage_handle_error(
                path,
                stage_writer,
                stage_name,
                "validate stage",
                source,
            ));
        }
    };
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
        if let Err(source) = checkpoint("before-publication-revalidation") {
            return Err(cleanup_stage_handle_error(
                path,
                stage.take().expect("stage remains live before publication"),
                stage_name,
                "reverify before publication",
                source,
            ));
        }
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

        match move_stage_to_destination(parent_path, &stage_name, destination_name) {
            Ok(()) => break,
            Err(source)
                if is_retryable_publication_error(&source)
                    && attempt + 1 < MAX_PUBLICATION_ATTEMPTS =>
            {
                sleep_or_record_publication_backoff(PUBLICATION_RETRY_DELAY);
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

    if let Err(observation) =
        observe_published_destination(&parent, destination_name, stage_identity, contents)
    {
        return Ok(DetailedAtomicOutcome::PublishedParentPathUnverified {
            observation,
            sync_error: None,
        });
    }
    pause_at("post-publication-observation");

    match open_parent_no_follow(parent_path) {
        Ok((_, identity)) if identity == parent_identity => Ok(DetailedAtomicOutcome::Published),
        Ok(_) => Ok(DetailedAtomicOutcome::PublishedParentPathRaced { sync_error: None }),
        Err(observation) => Ok(DetailedAtomicOutcome::PublishedParentPathUnverified {
            observation,
            sync_error: None,
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

fn allocate_stage(parent: &OwnedHandle, destination: &OsStr) -> io::Result<(OsString, File)> {
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

fn move_stage_to_destination(
    parent_path: &Path,
    stage_name: &OsStr,
    destination_name: &OsStr,
) -> io::Result<()> {
    checkpoint("rename")?;
    let source = parent_path.join(stage_name);
    let destination = parent_path.join(destination_name);
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    pause_at_terminal_move_receipt(parent_path, stage_name);
    // SAFETY: both path buffers are NUL-terminated and remain live for the synchronous call.
    #[allow(unsafe_code)]
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING,
        )
    };
    #[cfg(any(test, feature = "test-hooks"))]
    record_windows_real_move();
    (result != 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)?;
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
    stage: impl AsRawHandle,
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

fn cleanup_stage_handle(stage: impl AsRawHandle) -> io::Result<()> {
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

fn flush_handle(handle: RawHandle) -> io::Result<()> {
    // SAFETY: `handle` remains valid for the synchronous FlushFileBuffers call.
    #[allow(unsafe_code)]
    let result = unsafe { FlushFileBuffers(handle) };
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
                || code == ERROR_ACCESS_DENIED as i32
    )
}

#[cfg(any(test, feature = "test-hooks"))]
struct WindowsPublicationTraceState {
    attempted: Vec<&'static str>,
    real_moves: usize,
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
    static WINDOWS_PUBLICATION_BACKOFFS: std::cell::RefCell<Option<Vec<Duration>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
static WINDOWS_TERMINAL_MOVE_RECEIPT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Run `op` while recording requested publication backoffs without sleeping.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_detailed_atomic_backoffs<T>(op: impl FnOnce() -> T) -> (T, Vec<Duration>) {
    WINDOWS_PUBLICATION_BACKOFFS.with(|backoffs| {
        assert!(
            backoffs.borrow().is_none(),
            "Windows detailed publication backoff recorder is already active"
        );
        *backoffs.borrow_mut() = Some(Vec::new());
    });
    let result = op();
    let backoffs = WINDOWS_PUBLICATION_BACKOFFS.with(|backoffs| {
        backoffs
            .borrow_mut()
            .take()
            .expect("Windows detailed publication backoff recorder remains active")
    });
    (result, backoffs)
}

#[cfg(any(test, feature = "test-hooks"))]
fn sleep_or_record_publication_backoff(delay: Duration) {
    let recorded = WINDOWS_PUBLICATION_BACKOFFS.with(|backoffs| {
        let mut backoffs = backoffs.borrow_mut();
        let Some(backoffs) = backoffs.as_mut() else {
            return false;
        };
        backoffs.push(delay);
        true
    });
    if !recorded {
        thread::sleep(delay);
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn sleep_or_record_publication_backoff(delay: Duration) {
    thread::sleep(delay);
}

/// Inject Windows publication failures and return every checkpoint reached by `op`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_detailed_atomic_faults<T>(
    faults: impl IntoIterator<Item = (&'static str, usize, i32)>,
    op: impl FnOnce() -> T,
) -> (T, Vec<&'static str>, usize) {
    WINDOWS_PUBLICATION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Windows detailed publication trace is already active"
        );
        *trace.borrow_mut() = Some(WindowsPublicationTraceState {
            attempted: Vec::new(),
            real_moves: 0,
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
    (result, state.attempted, state.real_moves)
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
            real_moves: 0,
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

/// Run `op` with injected Windows detailed-publication faults and one barrier callback.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_windows_detailed_atomic_faults_and_barrier<T>(
    faults: impl IntoIterator<Item = (&'static str, usize, i32)>,
    step: &'static str,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, Vec<&'static str>, bool) {
    WINDOWS_PUBLICATION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Windows detailed publication trace is already active"
        );
        *trace.borrow_mut() = Some(WindowsPublicationTraceState {
            attempted: Vec::new(),
            real_moves: 0,
            faults: faults
                .into_iter()
                .map(|(step, ordinal, raw_error)| WindowsPublicationFault {
                    step,
                    ordinal,
                    raw_error,
                })
                .collect(),
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
    (result, state.attempted, state.barriers.is_empty())
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

#[cfg(any(test, feature = "test-hooks"))]
fn record_windows_real_move() {
    WINDOWS_TERMINAL_MOVE_RECEIPT_COUNT.fetch_add(1, Ordering::Relaxed);
    WINDOWS_PUBLICATION_TRACE.with(|trace| {
        if let Some(state) = trace.borrow_mut().as_mut() {
            state.real_moves += 1;
        }
    });
}

#[cfg(any(test, feature = "test-hooks"))]
fn pause_at_terminal_move_receipt(parent_path: &Path, stage_name: &OsStr) {
    if std::env::var("JOURNAL_IO_TEST_PAUSE_AT").ok().as_deref() != Some("terminal-move") {
        return;
    }
    if let Ok(marker) = std::env::var("JOURNAL_IO_TEST_MARKER") {
        if let Ok(file) = File::open(parent_path.join(stage_name)) {
            if let Ok(identity) = file_identity(file.as_raw_handle()) {
                let moves_so_far = WINDOWS_TERMINAL_MOVE_RECEIPT_COUNT.load(Ordering::Relaxed);
                let file_id_hex = identity
                    .file_id()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                let payload = format!(
                    "pid={}\nstage={}\nvolume_serial={}\nfile_id={}\nterminal_move_snapshot_present=1\nterminal_move_snapshot_count={}\n",
                    std::process::id(),
                    stage_name.to_string_lossy(),
                    identity.volume_serial(),
                    file_id_hex,
                    moves_so_far,
                );
                let _ = fs::write(marker, payload);
            }
        }
    }
    loop {
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn pause_at_terminal_move_receipt(_parent_path: &Path, _stage_name: &OsStr) {}

#[cfg(not(any(test, feature = "test-hooks")))]
fn checkpoint(_step: &'static str) -> io::Result<()> {
    Ok(())
}
