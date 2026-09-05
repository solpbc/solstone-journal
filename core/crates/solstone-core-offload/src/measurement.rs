// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only raw-media and device-space measurement.

use std::fs;
use std::path::Path;

#[cfg(unix)]
use nix::sys::statvfs::statvfs;
use solstone_core_journal_io::paths::{PathOrDay, day_dirs, iter_segments};

pub const MIN_FLOOR_BYTES: u64 = 20_000_000_000;
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RawMediaDayUsage {
    pub day: String,
    pub bytes: u64,
    pub files: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RawMediaUsage {
    pub total_bytes: u64,
    pub total_files: u64,
    pub per_day: Vec<RawMediaDayUsage>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestedOffloadDefaults {
    pub budget_bytes: u64,
    pub floor_bytes: u64,
}
fn raw_files(segment: &Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(segment)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_none_or(|extension| extension != "jsonl")
        })
        .collect()
}
pub fn measure_raw_media_usage(journal: &Path) -> RawMediaUsage {
    let mut days = day_dirs(journal)
        .unwrap_or_default()
        .into_keys()
        .collect::<Vec<_>>();
    days.sort();
    let per_day = days
        .into_iter()
        .map(|day| {
            let mut bytes = 0;
            let mut files = 0;
            for segment in iter_segments(journal, PathOrDay::Day(&day)).unwrap_or_default() {
                for file in raw_files(segment.path()) {
                    if let Ok(metadata) = file.metadata() {
                        bytes += metadata.len();
                        files += 1
                    }
                }
            }
            RawMediaDayUsage { day, bytes, files }
        })
        .collect::<Vec<_>>();
    RawMediaUsage {
        total_bytes: per_day.iter().map(|day| day.bytes).sum(),
        total_files: per_day.iter().map(|day| day.files).sum(),
        per_day,
    }
}
// `statvfs`'s block counts are u64 on Linux and u32 on Darwin, so both of these
// only compile on Linux unconverted. See the note in
// `solstone-core-backup-web::measurement` for why the lint has to be silenced
// rather than the cast dropped.
#[cfg(unix)]
pub fn device_free_bytes(journal: &Path) -> Result<u64, String> {
    let stat = statvfs(journal).map_err(|error| error.to_string())?;
    #[allow(clippy::unnecessary_cast)]
    let blocks = stat.blocks_available() as u64;
    Ok(blocks.saturating_mul(stat.fragment_size()))
}
#[cfg(unix)]
pub fn device_total_bytes(journal: &Path) -> Result<u64, String> {
    let stat = statvfs(journal).map_err(|error| error.to_string())?;
    #[allow(clippy::unnecessary_cast)]
    let blocks = stat.blocks() as u64;
    Ok(blocks.saturating_mul(stat.fragment_size()))
}

#[cfg(windows)]
pub fn device_free_bytes(journal: &Path) -> Result<u64, String> {
    solstone_core_journal_io::windows_disk_space(journal)
        .map(|space| space.available_bytes)
        .map_err(|error| error.to_string())
}

#[cfg(windows)]
pub fn device_total_bytes(journal: &Path) -> Result<u64, String> {
    solstone_core_journal_io::windows_disk_space(journal)
        .map(|space| space.total_bytes)
        .map_err(|error| error.to_string())
}

#[cfg(not(any(unix, windows)))]
pub fn device_free_bytes(_journal: &Path) -> Result<u64, String> {
    Err("device capacity measurement is unsupported on this platform".into())
}

#[cfg(not(any(unix, windows)))]
pub fn device_total_bytes(_journal: &Path) -> Result<u64, String> {
    Err("device capacity measurement is unsupported on this platform".into())
}
pub fn suggest_offload_defaults(total: u64) -> Result<SuggestedOffloadDefaults, String> {
    if total == 0 {
        return Err("total_bytes must be a positive integer".into());
    }
    Ok(SuggestedOffloadDefaults {
        budget_bytes: total / 2,
        floor_bytes: (total / 10).max(MIN_FLOOR_BYTES).min(total / 4),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinct_multiday_usage_aggregates_real_tree() {
        let journal = tempfile::tempdir().unwrap();
        let first = journal.path().join("chronicle/20260101/010000_001");
        let second = journal.path().join("chronicle/20260102/020000_001");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("raw.webm"), b"one").unwrap();
        fs::write(second.join("raw.webm"), b"012345").unwrap();
        let usage = measure_raw_media_usage(journal.path());
        assert_eq!(usage.total_bytes, 9);
        assert_eq!(
            usage
                .per_day
                .iter()
                .map(|day| day.bytes)
                .collect::<Vec<_>>(),
            vec![3, 6]
        );
    }
    #[test]
    fn defaults_match_floor_rule() {
        assert_eq!(
            suggest_offload_defaults(100_000_000_000)
                .unwrap()
                .floor_bytes,
            20_000_000_000
        );
    }
}
