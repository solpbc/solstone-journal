// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows handle-relative no-replace rename primitive.

use std::ffi::OsStr;
use std::io;
use std::mem::{align_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::RawHandle;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_RENAME_INFORMATION, FileRenameInformation, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{HANDLE, RtlNtStatusToDosError, STATUS_SUCCESS};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// Rename an already-opened stage handle relative to a retained directory handle without replacing.
pub(crate) fn rename_handle_no_replace(
    parent: RawHandle,
    stage: RawHandle,
    dest_name: &OsStr,
) -> io::Result<()> {
    let wide: Vec<u16> = dest_name.encode_wide().collect();
    let extra = wide
        .len()
        .saturating_sub(1)
        .saturating_mul(size_of::<u16>());
    let bytes = size_of::<FILE_RENAME_INFORMATION>()
        .checked_add(extra)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename buffer too large"))?;
    let words = bytes.div_ceil(size_of::<u64>());
    let mut buffer = vec![0_u64; words];
    let pointer = buffer.as_mut_ptr();
    debug_assert_eq!(
        pointer
            .cast::<u8>()
            .align_offset(align_of::<FILE_RENAME_INFORMATION>()),
        0
    );
    // SAFETY: `buffer` is zeroed, aligned to `FILE_RENAME_INFORMATION` (the
    // `Vec<u64>` allocation is at least pointer-width), sized for the fixed header plus
    // the inline filename, and live for this synchronous native request. `status` is
    // writable output storage for that synchronous request.
    #[allow(unsafe_code)]
    unsafe {
        let info = pointer.cast::<FILE_RENAME_INFORMATION>();
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = parent as HANDLE;
        (*info).FileNameLength = (wide.len() * size_of::<u16>()) as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), (*info).FileName.as_mut_ptr(), wide.len());
        let mut status = IO_STATUS_BLOCK::default();
        let result = NtSetInformationFile(
            stage,
            &mut status,
            pointer.cast(),
            bytes as u32,
            FileRenameInformation,
        );
        if result != STATUS_SUCCESS {
            Err(io::Error::from_raw_os_error(
                RtlNtStatusToDosError(result) as i32
            ))
        } else {
            Ok(())
        }
    }
}
