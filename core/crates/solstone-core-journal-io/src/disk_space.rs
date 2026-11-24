// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-platform available disk space query for an existing directory.

use std::io;
use std::path::Path;

/// Returns the bytes available to the current account on the volume containing `path`.
///
/// Callers must pass an existing directory so the capacity decision is anchored
/// to the exact volume where they will write.
#[cfg(unix)]
pub fn available_disk_bytes(path: &Path) -> io::Result<u64> {
    let stats = nix::sys::statvfs::statvfs(path).map_err(io::Error::from)?;
    #[allow(clippy::unnecessary_cast)]
    let blocks = stats.blocks_available() as u64;
    #[allow(clippy::unnecessary_cast)]
    let fragment = stats.fragment_size() as u64;
    Ok(blocks.saturating_mul(fragment))
}

/// Returns the bytes available to the current account on the volume containing `path`.
#[cfg(windows)]
pub fn available_disk_bytes(path: &Path) -> io::Result<u64> {
    crate::windows_available_disk_bytes(path)
}

#[cfg(test)]
mod tests {
    use super::available_disk_bytes;

    #[test]
    fn reports_available_bytes_for_an_existing_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let available = available_disk_bytes(dir.path()).expect("available disk bytes");
        assert!(
            available > 0,
            "expected positive free space on tempdir volume"
        );
    }

    #[test]
    fn missing_directory_returns_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
        let error = available_disk_bytes(&missing).expect_err("missing path must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
}
