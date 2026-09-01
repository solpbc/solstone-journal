// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows exclusive stage, handle-bound no-replace rename, and bound lease probe.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HANDLE,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_DISPOSITION_INFO, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO,
    FileDispositionInfo, FileRenameInfo, SYNCHRONIZE, SetFileInformationByHandle,
};

use super::create::OplogCreateError;
use super::namespace::OplogDayHealth;
use crate::atomic::{ATOMIC_CANDIDATE_MARKER, publication_candidate_name};
use crate::lease::{LeaseProbe, SelfLease, acquire_self_lease, probe_file_lease};
use crate::windows_identity::{WindowsFileIdentity, file_identity};
use crate::windows_ntcreate::nt_create_relative;
use crate::windows_sync_dir::validate_windows_regular_handle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WindowsIdentity {
    identity: WindowsFileIdentity,
}

pub(super) struct StagedFile {
    pub file: File,
    #[allow(dead_code)]
    pub stage_name: OsString,
    pub identity: WindowsIdentity,
}

pub(super) fn stage_exclusive(
    health: &OplogDayHealth,
    dest: &OsStr,
) -> Result<StagedFile, OplogCreateError> {
    let sequence = std::process::id() as u128;
    let stage_name = publication_candidate_name(dest, ATOMIC_CANDIDATE_MARKER, &[sequence]);
    let handle = nt_create_relative(
        health.health().as_handle().as_raw_handle(),
        stage_name.as_os_str(),
        FILE_APPEND_DATA | DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    )
    .map_err(|_| OplogCreateError::io())?;
    let file = File::from(handle);
    let path = health
        .health()
        .diagnostic_entry_path(stage_name.as_os_str());
    let identity = validate_windows_regular_handle(file.as_raw_handle(), &path)
        .map_err(|_| OplogCreateError::io())?;
    Ok(StagedFile {
        file,
        stage_name,
        identity: WindowsIdentity { identity },
    })
}

pub(super) fn lease_staged(file: &File) -> Result<Option<SelfLease>, OplogCreateError> {
    acquire_self_lease(file).map_err(|_| OplogCreateError::io())
}

pub(super) fn publish_handle_bound(
    health: &OplogDayHealth,
    staged: StagedFile,
    dest: &OsStr,
) -> Result<File, PublishOutcome> {
    match rename_open_stage_no_replace(health, &staged.file, dest) {
        Ok(()) => Ok(staged.file),
        Err(error) if is_already_exists(&error) => Err(PublishOutcome::Occupied(staged)),
        Err(_) => Err(PublishOutcome::Io(staged)),
    }
}

pub(super) fn publish_name_based(
    health: &OplogDayHealth,
    staged: StagedFile,
    dest: &OsStr,
) -> Result<File, PublishOutcome> {
    publish_handle_bound(health, staged, dest)
}

pub(super) enum PublishOutcome {
    Occupied(StagedFile),
    #[allow(dead_code)]
    OccupiedName {
        identity: WindowsIdentity,
    },
    Io(StagedFile),
    #[allow(dead_code)]
    NameBasedIo,
    #[allow(dead_code)]
    WrongIdentityPublished {
        file: File,
    },
    #[allow(dead_code)]
    IoAfterPublish {
        file: File,
    },
}

pub(super) fn rollback_stage(
    health: &OplogDayHealth,
    staged: &StagedFile,
) -> Result<(), OplogCreateError> {
    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `staged.file` is an owned handle opened with DELETE.
    #[allow(unsafe_code)]
    let result = unsafe {
        SetFileInformationByHandle(
            staged.file.as_raw_handle(),
            FileDispositionInfo,
            (&mut disposition as *mut FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if result == 0 {
        Err(OplogCreateError::own_residue())
    } else {
        let _ = health;
        Ok(())
    }
}

pub(super) fn dest_is_foreign(
    health: &OplogDayHealth,
    dest: &OsStr,
    expected: WindowsIdentity,
) -> Result<bool, OplogCreateError> {
    let handle = match nt_create_relative(
        health.health().as_handle().as_raw_handle(),
        dest,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(handle) => handle,
        Err(_) => return Err(OplogCreateError::io()),
    };
    let path = health.health().diagnostic_entry_path(dest);
    let identity = validate_windows_regular_handle(handle.as_raw_handle(), &path)
        .map_err(|_| OplogCreateError::io())?;
    Ok(identity != expected.identity)
}

pub(super) fn probe_named(health: &OplogDayHealth, leaf: &OsStr) -> LeaseProbe {
    let handle = match nt_create_relative(
        health.health().as_handle().as_raw_handle(),
        leaf,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(handle) => handle,
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(code)
                    if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            return LeaseProbe::Indeterminate;
        }
        Err(_) => return LeaseProbe::Indeterminate,
    };
    let path = health.health().diagnostic_entry_path(leaf);
    if validate_windows_regular_handle(handle.as_raw_handle(), &path).is_err() {
        return LeaseProbe::Indeterminate;
    }
    let file = File::from(handle);
    probe_file_lease(&file)
}

fn rename_open_stage_no_replace(
    health: &OplogDayHealth,
    stage_file: &File,
    dest_name: &OsStr,
) -> io::Result<()> {
    let wide: Vec<u16> = dest_name.encode_wide().collect();
    let extra = wide
        .len()
        .saturating_sub(1)
        .saturating_mul(size_of::<u16>());
    let bytes = size_of::<FILE_RENAME_INFO>()
        .checked_add(extra)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename buffer too large"))?;
    let mut buffer = vec![0_u8; bytes];
    // SAFETY: buffer is zeroed, sized for FILE_RENAME_INFO plus the filename,
    // and live for this synchronous SetFileInformationByHandle call.
    #[allow(unsafe_code)]
    unsafe {
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = health.health().as_handle().as_raw_handle() as HANDLE;
        (*info).FileNameLength = (wide.len() * size_of::<u16>()) as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), (*info).FileName.as_mut_ptr(), wide.len());
        let result = SetFileInformationByHandle(
            stage_file.as_raw_handle(),
            FileRenameInfo,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        );
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn is_already_exists(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32)
}

#[allow(dead_code)]
fn _offset_of_filename() -> usize {
    offset_of!(FILE_RENAME_INFO, FileName)
}

#[allow(dead_code)]
fn _from_owned(handle: OwnedHandle) -> File {
    File::from(handle)
}

#[allow(dead_code)]
fn _from_raw(handle: std::os::windows::io::RawHandle) -> File {
    // SAFETY: caller owns the handle.
    #[allow(unsafe_code)]
    File::from(unsafe { OwnedHandle::from_raw_handle(handle) })
}

#[allow(dead_code)]
fn _identity(handle: std::os::windows::io::RawHandle) -> io::Result<WindowsFileIdentity> {
    file_identity(handle)
}
