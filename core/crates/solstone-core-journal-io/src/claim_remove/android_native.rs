// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Raw no-replace rename shared by the two owned Android authorities.

use super::*;

#[allow(unsafe_code)]
pub(super) fn rename_no_replace(
    directory: &FlatDirectory,
    from: &OsStr,
    to: &OsStr,
) -> io::Result<()> {
    let from = CString::new(from.as_bytes()).map_err(io::Error::other)?;
    let to = CString::new(to.as_bytes()).map_err(io::Error::other)?;
    // Both validated components remain relative to the same held descriptor.
    // No fallback may replace a name when the kernel/filesystem refuses this.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            directory.as_fd().as_raw_fd(),
            from.as_ptr(),
            directory.as_fd().as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
