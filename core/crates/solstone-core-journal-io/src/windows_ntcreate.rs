// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Minimal descriptor-relative `NtCreateFile` binding shared by Windows I/O paths.

use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::NtCreateFile;
use windows_sys::Win32::Foundation::{
    INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, STATUS_SUCCESS,
    UNICODE_STRING,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// Open or create one native-name child relative to an already retained directory handle.
pub(crate) fn nt_create_relative(
    parent: RawHandle,
    name: &OsStr,
    desired_access: u32,
    disposition: u32,
    options: u32,
) -> io::Result<OwnedHandle> {
    let wide = name.encode_wide().collect::<Vec<_>>();
    let byte_length = wide
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "relative name is too long"))?;
    let mut object_name = UNICODE_STRING {
        Length: byte_length,
        MaximumLength: byte_length,
        Buffer: wide.as_ptr().cast_mut(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent,
        ObjectName: &mut object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle = INVALID_HANDLE_VALUE;
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: `attributes` refers to the live UTF-16 component and retained parent handle;
    // all output pointers refer to initialized local storage, and the synchronous request
    // does not outlive them.
    #[allow(unsafe_code)]
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access,
            &attributes,
            &mut status,
            std::ptr::null(),
            0,
            windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
            disposition,
            options,
            std::ptr::null(),
            0,
        )
    };
    if result != STATUS_SUCCESS {
        // SAFETY: `RtlNtStatusToDosError` converts the just-returned NTSTATUS without
        // borrowing caller memory.
        #[allow(unsafe_code)]
        let error = unsafe { RtlNtStatusToDosError(result) };
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    // SAFETY: `NtCreateFile` returned one owned valid handle and the conversion occurs
    // exactly once.
    #[allow(unsafe_code)]
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
