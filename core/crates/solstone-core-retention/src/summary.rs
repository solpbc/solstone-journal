// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only storage accounting for the Settings surface.

use std::fs;
use std::io;
use std::path::Path;

use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

use crate::{ContentName, MediaClassifier, content::JournalMedia};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageSummary {
    pub raw_media_bytes: u64,
    pub derived_bytes: u64,
    pub total_segments: u64,
    pub segments_with_raw: u64,
    pub segments_purged: u64,
}

impl StorageSummary {
    pub fn raw_media_human(self) -> String {
        human_bytes(self.raw_media_bytes)
    }

    pub fn derived_human(self) -> String {
        human_bytes(self.derived_bytes)
    }
}

pub fn compute_storage_summary(journal_root: &Path) -> io::Result<StorageSummary> {
    let chronicle = journal_root.join("chronicle");
    let days = match fs::read_dir(chronicle) {
        Ok(days) => days,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(StorageSummary::default());
        }
        Err(error) => return Err(error),
    };
    let mut summary = StorageSummary::default();
    for day in days {
        let day = day?;
        if !day.file_type()?.is_dir() {
            continue;
        }
        let segments = iter_segments(journal_root, PathOrDay::Directory(&day.path()))
            .map_err(io::Error::other)?;
        for segment in segments {
            summary.total_segments = summary.total_segments.saturating_add(1);
            let (raw_bytes, derived_bytes, has_index) = segment_bytes(segment.path(), true)?;
            summary.raw_media_bytes = summary.raw_media_bytes.saturating_add(raw_bytes);
            summary.derived_bytes = summary.derived_bytes.saturating_add(derived_bytes);
            if raw_bytes > 0 {
                summary.segments_with_raw = summary.segments_with_raw.saturating_add(1);
            } else if has_index {
                summary.segments_purged = summary.segments_purged.saturating_add(1);
            }
        }
    }
    Ok(summary)
}

// A single traversal counts both categories and propagates its read errors.
// Symlink entries encountered within a segment are skipped, avoiding recursive
// cycles. Segment discovery retains the shared iter_segments contract.
fn segment_bytes(path: &Path, immediate: bool) -> io::Result<(u64, u64, bool)> {
    let (mut raw, mut derived, mut has_index) = (0_u64, 0_u64, false);
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            let (_, bytes, _) = segment_bytes(&entry.path(), false)?;
            derived = derived.saturating_add(bytes);
        } else if kind.is_file() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str().and_then(ContentName::new) else {
                continue;
            };
            let bytes = entry.metadata()?.len();
            if JournalMedia.is_owner_media(&name) {
                if immediate {
                    raw = raw.saturating_add(bytes);
                }
            } else {
                derived = derived.saturating_add(bytes);
            }
            has_index |= immediate && (file_name == "audio.jsonl" || file_name == "screen.jsonl");
        }
    }
    Ok((raw, derived, has_index))
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "KiB";
    for candidate in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = candidate;
    }
    if value >= 1024.0 && unit == "TiB" {
        format!("{:.1} PiB", value / 1024.0)
    } else {
        format!("{value:.1} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{compute_storage_summary, human_bytes};
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn recursive_accounting_skips_descendant_symlink_cycles() -> std::io::Result<()> {
        let temporary = TempDir::new()?;
        let segment = temporary.path().join("chronicle/20260810/tmux/090000_300");
        fs::create_dir_all(&segment)?;
        fs::write(segment.join("notes.md"), "abc")?;
        std::os::unix::fs::symlink(&segment, segment.join("cycle"))?;
        let summary = compute_storage_summary(temporary.path())?;
        assert_eq!(summary.derived_bytes, 3);
        assert_eq!(summary.total_segments, 1);
        Ok(())
    }

    #[test]
    fn unavailable_chronicle_is_not_reported_as_empty() -> std::io::Result<()> {
        let temporary = TempDir::new()?;
        assert_eq!(compute_storage_summary(temporary.path())?.total_segments, 0);
        fs::write(temporary.path().join("chronicle"), "not a directory")?;
        assert!(compute_storage_summary(temporary.path()).is_err());
        Ok(())
    }

    #[test]
    fn populated_storage_arithmetic_matches_the_corpus() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = TempDir::new()?;
        let with_raw = temporary.path().join("chronicle/20260810/tmux/090000_300");
        let purged = temporary.path().join("chronicle/20260810/tmux/100000_300");
        let bare = temporary
            .path()
            .join("chronicle/20260811/screen/090000_120");
        for path in [&with_raw, &purged, &bare] {
            fs::create_dir_all(path)?;
        }
        fs::write(with_raw.join("audio.flac"), vec![0_u8; 4096])?;
        fs::write(with_raw.join("monitor_1_diff.png"), vec![0_u8; 2048])?;
        fs::write(with_raw.join("audio.jsonl"), b"{\"seeded\": true}\n")?;
        fs::write(purged.join("audio.jsonl"), b"{\"seeded\": true}\n")?;
        fs::write(bare.join("notes.md"), b"seeded\n")?;

        let summary = compute_storage_summary(temporary.path())?;
        assert_eq!(summary.total_segments, 3);
        assert_eq!(summary.segments_with_raw, 1);
        assert_eq!(summary.segments_purged, 1);
        assert_eq!(summary.raw_media_bytes, 6144);
        assert_eq!(summary.derived_bytes, 41);
        assert_eq!(summary.raw_media_human(), "6.0 KiB");
        assert_eq!(summary.derived_human(), "41 B");
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1024_u64.pow(5)), "1.0 PiB");
        Ok(())
    }
}
