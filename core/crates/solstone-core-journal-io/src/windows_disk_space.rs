// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows volume-capacity inspection for existing journal-owned directories.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

/// Capacity reported for the caller on the volume containing an existing path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsDiskSpace {
    pub available_bytes: u64,
    pub total_bytes: u64,
}

/// Returns the bytes available to the current account on the volume containing `path`.
///
/// Callers must create and validate `path` before this query so the capacity decision is
/// anchored to the exact volume where they will write.
pub fn windows_available_disk_bytes(path: &Path) -> io::Result<u64> {
    Ok(windows_disk_space(path)?.available_bytes)
}

/// Returns caller-available and total capacity for the volume containing `path`.
///
/// Callers must create and validate `path` before this query so the capacity decision is
/// anchored to the exact volume where they will write.
pub fn windows_disk_space(path: &Path) -> io::Result<WindowsDiskSpace> {
    let mut directory = path.as_os_str().encode_wide().collect::<Vec<_>>();
    directory.push(0);
    let mut available = 0_u64;
    let mut total = 0_u64;
    let mut free = 0_u64;
    // SAFETY: `directory` is nul-terminated and live for the synchronous call; the
    // three u64 values are writable Windows ULARGE_INTEGER-compatible output.
    #[allow(unsafe_code)]
    let result =
        unsafe { GetDiskFreeSpaceExW(directory.as_ptr(), &mut available, &mut total, &mut free) };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(WindowsDiskSpace {
        available_bytes: available,
        total_bytes: total,
    })
}

/// Returns the NTFS/ReFS volume serial number for the volume containing an existing
/// file or directory path (including mounted volume mount-points).
pub fn windows_volume_serial(path: &Path) -> io::Result<u64> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let handle = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    Ok(crate::windows_identity::windows_file_identity(&handle)?.volume_serial())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_current_account_capacity_for_an_existing_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let space = windows_disk_space(temporary.path()).expect("volume capacity");
        assert!(space.total_bytes > 0);
        assert!(space.available_bytes <= space.total_bytes);
        let available = windows_available_disk_bytes(temporary.path()).expect("available capacity");
        assert!(available <= space.total_bytes);
        let serial_a = windows_volume_serial(temporary.path()).expect("volume serial");
        let nested = temporary.path().join("child");
        std::fs::create_dir(&nested).expect("nested dir");
        let serial_b = windows_volume_serial(&nested).expect("nested volume serial");
        assert_eq!(serial_a, serial_b);
    }
}
