// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Share-mode liveness probe via OpenFileById.

use std::ffi::OsStr;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

use windows_sys::Win32::Foundation::{ERROR_SHARING_VIOLATION, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    ExtendedFileIdType, FILE_APPEND_DATA, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_128,
    FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileNameInfo, GetFileInformationByHandleEx, OpenFileById,
    SYNCHRONIZE,
};

use super::reason::OplogFileIdentity;
use crate::lease::LeaseProbe;

const FILE_NAME_CHARS: usize = 1024;

#[repr(C, align(4))]
struct FileNameBuffer {
    length: u32,
    name: [u16; FILE_NAME_CHARS],
}

pub(super) fn classify_liveness_by_id(
    volume_hint: RawHandle,
    identity: OplogFileIdentity,
) -> LeaseProbe {
    let Ok(volume) = crate::windows_identity::file_identity(volume_hint) else {
        return LeaseProbe::Indeterminate;
    };
    if volume.volume_serial() != identity.volume_serial {
        return LeaseProbe::Indeterminate;
    }
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: ExtendedFileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 {
            ExtendedFileId: FILE_ID_128 {
                Identifier: identity.file_id,
            },
        },
    };
    // SAFETY: `volume_hint` is live on the target volume, `descriptor` identifies one file, and
    // a successful return transfers exactly one owned handle. The append request is the deliberate
    // liveness oracle: an admitted writer omits FILE_SHARE_WRITE, while the probe itself shares all
    // access and closes without writing.
    #[allow(unsafe_code)]
    let raw = unsafe {
        OpenFileById(
            volume_hint,
            &descriptor,
            FILE_APPEND_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(code) if code == ERROR_SHARING_VIOLATION as i32 => LeaseProbe::Active,
            _ => LeaseProbe::Indeterminate,
        };
    }
    // SAFETY: `raw` is a valid uniquely owned handle after the invalid sentinel check.
    #[allow(unsafe_code)]
    let opened = unsafe { OwnedHandle::from_raw_handle(raw) };
    let matches = crate::windows_identity::file_identity(opened.as_raw_handle())
        .map(|observed| {
            OplogFileIdentity::from_windows(observed.volume_serial(), observed.file_id())
                == identity
        })
        .unwrap_or(false);
    drop(opened);
    if matches {
        LeaseProbe::Released
    } else {
        LeaseProbe::Indeterminate
    }
}

pub(super) fn on_disk_leaf_matches(handle: RawHandle, leaf: &OsStr) -> bool {
    let mut buffer = FileNameBuffer {
        length: 0,
        name: [0; FILE_NAME_CHARS],
    };
    // SAFETY: `buffer` is writable for its exact supplied size and `handle` remains valid
    // for the synchronous GetFileInformationByHandleEx call.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileNameInfo,
            (&mut buffer as *mut FileNameBuffer).cast(),
            size_of::<FileNameBuffer>() as u32,
        )
    };
    if result == 0 {
        return false;
    }
    let byte_len = buffer.length as usize;
    if byte_len % 2 != 0 {
        return false;
    }
    let chars = byte_len / 2;
    if chars == 0 || chars > FILE_NAME_CHARS {
        return false;
    }
    let wide = &buffer.name[..chars];
    let end = wide
        .iter()
        .rposition(|unit| *unit != 0)
        .map(|index| index + 1)
        .unwrap_or(0);
    let wide = &wide[..end];
    let start = wide
        .iter()
        .rposition(|unit| *unit == b'\\' as u16 || *unit == b'/' as u16)
        .map(|index| index + 1)
        .unwrap_or(0);
    let on_disk = std::ffi::OsString::from_wide(&wide[start..]);
    on_disk.as_os_str() == leaf
}
