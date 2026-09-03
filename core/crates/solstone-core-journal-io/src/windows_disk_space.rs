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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_current_account_capacity_for_an_existing_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let space = windows_disk_space(temporary.path()).expect("volume capacity");
        assert!(space.total_bytes > 0);
        assert!(space.available_bytes <= space.total_bytes);
        assert_eq!(
            windows_available_disk_bytes(temporary.path()),
            Ok(space.available_bytes)
        );
    }
}
