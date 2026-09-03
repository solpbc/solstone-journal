// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows volume-capacity inspection for existing journal-owned directories.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

/// Returns the bytes available to the current account on the volume containing `path`.
///
/// Callers must create and validate `path` before this query so the capacity decision is
/// anchored to the exact volume where they will write.
pub fn windows_available_disk_bytes(path: &Path) -> io::Result<u64> {
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
    Ok(available)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_current_account_capacity_for_an_existing_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        assert!(windows_available_disk_bytes(temporary.path()).is_ok());
    }
}
