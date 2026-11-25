// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-platform disk space queries and shared journal low-space threshold.

use std::io;
use std::path::Path;

/// Minimum free-space floor on the journal volume (20 GB decimal, capped at a
/// quarter of smaller disks). Shared across offload, supervisor, and health checks.
pub const JOURNAL_DISK_MIN_FLOOR_BYTES: u64 = 20_000_000_000;

/// Free and total bytes on the volume holding the journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalDiskSpace {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// Hysteresis level for low-space owner notices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalDiskLevel {
    Low,
    Back,
}

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

/// Returns total capacity in bytes on the volume containing `path`.
#[cfg(unix)]
pub fn total_disk_bytes(path: &Path) -> io::Result<u64> {
    let stats = nix::sys::statvfs::statvfs(path).map_err(io::Error::from)?;
    #[allow(clippy::unnecessary_cast)]
    let blocks = stats.blocks() as u64;
    #[allow(clippy::unnecessary_cast)]
    let fragment = stats.fragment_size() as u64;
    Ok(blocks.saturating_mul(fragment))
}

/// Returns total capacity in bytes on the volume containing `path`.
#[cfg(windows)]
pub fn total_disk_bytes(path: &Path) -> io::Result<u64> {
    crate::windows_disk_space(path).map(|space| space.total_bytes)
}

/// Reads both free and total bytes for the volume containing `path` in one query.
#[cfg(unix)]
pub fn read_journal_disk_space(path: &Path) -> io::Result<JournalDiskSpace> {
    let stats = nix::sys::statvfs::statvfs(path).map_err(io::Error::from)?;
    #[allow(clippy::unnecessary_cast)]
    let free_blocks = stats.blocks_available() as u64;
    #[allow(clippy::unnecessary_cast)]
    let total_blocks = stats.blocks() as u64;
    #[allow(clippy::unnecessary_cast)]
    let fragment = stats.fragment_size() as u64;
    Ok(JournalDiskSpace {
        free_bytes: free_blocks.saturating_mul(fragment),
        total_bytes: total_blocks.saturating_mul(fragment),
    })
}

/// Reads both free and total bytes for the volume containing `path` in one query.
#[cfg(windows)]
pub fn read_journal_disk_space(path: &Path) -> io::Result<JournalDiskSpace> {
    crate::windows_disk_space(path).map(|space| JournalDiskSpace {
        free_bytes: space.available_bytes,
        total_bytes: space.total_bytes,
    })
}

/// Effective low-space floor for a volume of `total_bytes`:
/// `min(JOURNAL_DISK_MIN_FLOOR_BYTES, total_bytes / 4)`.
#[must_use]
pub const fn journal_disk_floor_bytes(total_bytes: u64) -> u64 {
    let quarter = total_bytes / 4;
    if JOURNAL_DISK_MIN_FLOOR_BYTES < quarter {
        JOURNAL_DISK_MIN_FLOOR_BYTES
    } else {
        quarter
    }
}

/// Raise below the floor (`min(20 GB, total / 4)`); clear at or above 1.5x the floor;
/// return `None` in between or when total disk size is zero.
#[must_use]
pub const fn journal_disk_level(space: JournalDiskSpace) -> Option<JournalDiskLevel> {
    if space.total_bytes == 0 {
        return None;
    }
    let floor = journal_disk_floor_bytes(space.total_bytes);
    let clear = floor.saturating_add(floor / 2);
    if space.free_bytes < floor {
        Some(JournalDiskLevel::Low)
    } else if space.free_bytes >= clear {
        Some(JournalDiskLevel::Back)
    } else {
        None
    }
}

/// Whether `space` is strictly below the journal low-space floor.
#[must_use]
pub const fn journal_disk_is_low(space: JournalDiskSpace) -> bool {
    matches!(journal_disk_level(space), Some(JournalDiskLevel::Low))
}

/// Percentage of volume capacity currently used, rounded to one decimal place.
#[must_use]
pub fn journal_disk_percent_used(space: JournalDiskSpace) -> f64 {
    if space.total_bytes == 0 {
        return 0.0;
    }
    let used = space.total_bytes.saturating_sub(space.free_bytes) as f64;
    let percent = (used / space.total_bytes as f64) * 100.0;
    (percent * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::{
        JOURNAL_DISK_MIN_FLOOR_BYTES, JournalDiskLevel, JournalDiskSpace, available_disk_bytes,
        journal_disk_floor_bytes, journal_disk_is_low, journal_disk_level,
        journal_disk_percent_used, read_journal_disk_space, total_disk_bytes,
    };

    #[test]
    fn reports_available_and_total_bytes_for_an_existing_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let available = available_disk_bytes(dir.path()).expect("available disk bytes");
        let total = total_disk_bytes(dir.path()).expect("total disk bytes");
        let space = read_journal_disk_space(dir.path()).expect("journal disk space");
        assert!(
            available > 0,
            "expected positive free space on tempdir volume"
        );
        assert!(total >= available);
        assert!(space.total_bytes > 0 && space.free_bytes <= space.total_bytes);
    }

    #[test]
    fn missing_directory_returns_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
        let error = available_disk_bytes(&missing).expect_err("missing path must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn threshold_matches_decimal_twenty_gb_and_quarter_cap() {
        const GB: u64 = 1_000_000_000;
        assert_eq!(JOURNAL_DISK_MIN_FLOOR_BYTES, 20 * GB);
        assert_eq!(journal_disk_floor_bytes(100 * GB), 20 * GB);
        assert_eq!(journal_disk_floor_bytes(40 * GB), 10 * GB);

        let large = |free| JournalDiskSpace {
            free_bytes: free,
            total_bytes: 100 * GB,
        };
        assert_eq!(
            journal_disk_level(large(20 * GB - 1)),
            Some(JournalDiskLevel::Low)
        );
        assert!(journal_disk_is_low(large(20 * GB - 1)));
        assert_eq!(journal_disk_level(large(20 * GB)), None);
        assert!(!journal_disk_is_low(large(20 * GB)));
        assert_eq!(
            journal_disk_level(large(30 * GB)),
            Some(JournalDiskLevel::Back)
        );
        assert_eq!(
            journal_disk_level(JournalDiskSpace {
                free_bytes: 0,
                total_bytes: 0
            }),
            None
        );
        assert_eq!(journal_disk_percent_used(large(25 * GB)), 75.0);
    }
}
