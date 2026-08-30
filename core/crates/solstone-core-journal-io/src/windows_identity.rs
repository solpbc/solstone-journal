// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows file-identity queries shared by no-follow I/O primitives.

use std::io;
use std::mem::size_of;
use std::os::windows::io::RawHandle;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowsFileIdentity {
    volume_serial: u64,
    file_id: [u8; 16],
}

impl WindowsFileIdentity {
    /// Reconstruct an identity decoded from a fixed-width on-disk representation.
    pub(crate) const fn from_parts(volume_serial: u64, file_id: [u8; 16]) -> Self {
        Self {
            volume_serial,
            file_id,
        }
    }

    pub(crate) const fn volume_serial(self) -> u64 {
        self.volume_serial
    }

    pub(crate) const fn file_id(self) -> [u8; 16] {
        self.file_id
    }

    pub(crate) fn folded_file_id(self) -> u64 {
        let first = u64::from_le_bytes(
            self.file_id[..8]
                .try_into()
                .expect("a Windows file ID has a 64-bit first half"),
        );
        let second = u64::from_le_bytes(
            self.file_id[8..]
                .try_into()
                .expect("a Windows file ID has a 64-bit second half"),
        );
        // This fold is collision-tolerant GC-candidate identity representation, not global uniqueness.
        first ^ second
    }
}

pub(crate) fn file_identity(handle: RawHandle) -> io::Result<WindowsFileIdentity> {
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
