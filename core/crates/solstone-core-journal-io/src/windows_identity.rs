// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows file-identity queries shared by no-follow I/O primitives.

use std::io;
use std::mem::size_of;
use std::os::windows::io::RawHandle;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ID_INFO, FILE_STANDARD_INFO, FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowsFileIdentity {
    volume_serial: u64,
    file_id: [u8; 16],
}

impl WindowsFileIdentity {
    /// Reconstruct an identity decoded from a fixed-width on-disk representation.
    #[cfg(test)]
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

pub(crate) fn file_link_count(handle: RawHandle) -> io::Result<u64> {
    let mut info = FILE_STANDARD_INFO {
        AllocationSize: 0,
        EndOfFile: 0,
        NumberOfLinks: 0,
        DeletePending: false,
        Directory: false,
    };
    // SAFETY: `info` is writable for its exact buffer size and `handle` remains valid
    // for the synchronous GetFileInformationByHandleEx call.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            (&mut info as *mut FILE_STANDARD_INFO).cast(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    (result != 0)
        .then_some(u64::from(info.NumberOfLinks))
        .ok_or_else(io::Error::last_os_error)
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::fs::{self, File};
    use std::os::windows::io::AsRawHandle;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn link_count_reflects_a_new_hard_link() {
        let temporary = TempDir::new();
        let original = temporary.path().join("original.bin");
        fs::write(&original, b"content").unwrap();

        let handle = File::open(&original).unwrap();
        assert_eq!(file_link_count(handle.as_raw_handle()).unwrap(), 1);
        drop(handle);

        let linked = temporary.path().join("linked.bin");
        fs::hard_link(&original, &linked).unwrap();

        let handle = File::open(&original).unwrap();
        assert_eq!(file_link_count(handle.as_raw_handle()).unwrap(), 2);
    }
}
