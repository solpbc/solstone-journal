// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only storage accounting for the Settings surface.

use std::fs;
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

pub fn compute_storage_summary(journal_root: &Path) -> StorageSummary {
    let chronicle = journal_root.join("chronicle");
    let Ok(days) = fs::read_dir(chronicle) else {
        return StorageSummary::default();
    };
    let mut summary = StorageSummary::default();
    for day in days.flatten().filter(|entry| entry.path().is_dir()) {
        let Ok(segments) = iter_segments(journal_root, PathOrDay::Directory(&day.path())) else {
            continue;
        };
        for segment in segments {
            summary.total_segments = summary.total_segments.saturating_add(1);
            let raw_bytes = immediate_raw_bytes(segment.path());
            summary.raw_media_bytes = summary.raw_media_bytes.saturating_add(raw_bytes);
            if raw_bytes > 0 {
                summary.segments_with_raw = summary.segments_with_raw.saturating_add(1);
            } else if segment.path().join("audio.jsonl").is_file()
                || segment.path().join("screen.jsonl").is_file()
            {
                summary.segments_purged = summary.segments_purged.saturating_add(1);
            }
            summary.derived_bytes = summary
                .derived_bytes
                .saturating_add(derived_bytes(segment.path()));
        }
    }
    summary
}

fn immediate_raw_bytes(segment: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(segment) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| {
            if !entry.path().is_file() {
                return None;
            }
            let name = entry.file_name();
            let name = name.to_str().and_then(ContentName::new)?;
            if !JournalMedia.is_owner_media(&name) {
                return None;
            }
            entry.metadata().ok().map(|metadata| metadata.len())
        })
        .sum()
}

fn derived_bytes(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                return derived_bytes(&path);
            }
            if !path.is_file() {
                return 0;
            }
            let Some(name) = entry.file_name().to_str().and_then(ContentName::new) else {
                return 0;
            };
            if JournalMedia.is_owner_media(&name) {
                0
            } else {
                entry
                    .metadata()
                    .ok()
                    .map(|metadata| metadata.len())
                    .unwrap_or(0)
            }
        })
        .sum()
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

        let summary = compute_storage_summary(temporary.path());
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
