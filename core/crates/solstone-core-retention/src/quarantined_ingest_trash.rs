// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Quarantined failed-ingest files in the trash (`chronicle/<day>/observer/failed/...`).
//!
//! Files quarantined by failed ingest live outside segment content and are never
//! indexed or consumed by processing. Trashing one moves it through the standard
//! recoverable Trash pipeline so the owner can recover or erase it like any other
//! trash item.

use std::fs;
use std::io;
use std::path::Path;

use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_trash::{Change, Hold, Item, Kind, TrashError};

use crate::layout::{
    parse_quarantined_ingest_rel, quarantined_ingest_day_rel, quarantined_ingest_rel,
};

/// One quarantined failed-ingest file found on disk under
/// `chronicle/<day>/observer/failed/<segment>/<stamp>/<file>`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantinedIngestEntry {
    pub rel_path: String,
    pub day: String,
    pub segment: String,
    pub stamp: String,
    pub file: String,
    pub size_bytes: u64,
}

/// List all valid quarantined failed-ingest regular files for `day`.
/// Returns an empty list when the quarantine directory does not exist.
pub fn list_quarantined_ingest_for_day(
    journal: &Path,
    day: &str,
) -> io::Result<Vec<QuarantinedIngestEntry>> {
    let failed_root = journal.join(quarantined_ingest_day_rel(day));
    let seg_dirs = match fs::read_dir(&failed_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut found = Vec::new();
    for seg_entry in seg_dirs {
        let seg_entry = seg_entry?;
        if !seg_entry.file_type()?.is_dir() {
            continue;
        }
        let Some(segment) = seg_entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        for stamp_entry in fs::read_dir(seg_entry.path())? {
            let stamp_entry = stamp_entry?;
            if !stamp_entry.file_type()?.is_dir() {
                continue;
            }
            let Some(stamp) = stamp_entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            for file_entry in fs::read_dir(stamp_entry.path())? {
                let file_entry = file_entry?;
                if !file_entry.file_type()?.is_file() {
                    continue;
                }
                let Some(file) = file_entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let rel_path = quarantined_ingest_rel(day, &segment, &stamp, &file);
                if parse_quarantined_ingest_rel(&rel_path).is_none() {
                    continue;
                }
                let size_bytes = fs::metadata(file_entry.path())
                    .map(|m| m.len())
                    .unwrap_or(0);
                found.push(QuarantinedIngestEntry {
                    rel_path,
                    day: day.to_owned(),
                    segment: segment.clone(),
                    stamp: stamp.clone(),
                    file,
                    size_bytes,
                });
            }
        }
    }
    found.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(found)
}

struct QuarantinedHold;

impl Hold for QuarantinedHold {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

/// The trash's kind for quarantined failed-ingest files under
/// `chronicle/<day>/observer/failed/<segment>/<stamp>/<file>`.
pub struct QuarantinedIngest;

impl QuarantinedIngest {
    /// Move one validated quarantined file to recoverable trash.
    pub fn trash(
        journal: &Path,
        rel: &str,
        action: Action,
        surface: Surface,
    ) -> Result<Item, TrashError> {
        let Some((day, segment, stamp, file)) = parse_quarantined_ingest_rel(rel) else {
            return Err(TrashError::Missing("file".to_owned()));
        };
        let canonical_rel = quarantined_ingest_rel(&day, &segment, &stamp, &file);
        let unit = Unit::QuarantinedIngest {
            day,
            segment,
            stamp,
            file: file.clone(),
        };
        solstone_core_trash::detach_labelled(
            journal,
            &Self,
            unit,
            action,
            surface,
            vec![canonical_rel],
            Some(file),
        )
    }
}

impl Kind for QuarantinedIngest {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::QuarantinedIngest { .. })
    }

    fn hold<'h>(&'h self, _journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let valid = item
            .paths
            .first()
            .and_then(|path| parse_quarantined_ingest_rel(path))
            .is_some();
        if !valid {
            return Err("this isn't a quarantined ingest file your journal can place".to_owned());
        }
        Ok(Box::new(QuarantinedHold))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        parse_quarantined_ingest_rel(path).is_some()
            && fs::symlink_metadata(journal.join(path))
                .is_ok_and(|metadata| metadata.file_type().is_file())
    }

    fn references(
        &self,
        _journal: &Path,
        _item: &Item,
        _change: Change,
        _files: &[String],
    ) -> Result<(), String> {
        // Quarantined ingest files are outside segment content and search indexes.
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]
mod tests {
    use super::*;
    use solstone_core_removals::Page;
    use tempfile::TempDir;

    #[test]
    fn quarantined_ingest_file_trashes_recovers_and_erases() {
        let root = TempDir::new().unwrap();
        let rel = "chronicle/20261120/observer/failed/120000_300/1763640000000/audio.flac";
        let full = root.path().join(rel);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, b"quarantined audio").unwrap();

        let item = QuarantinedIngest::trash(
            root.path(),
            rel,
            Action::QuarantinedIngestDiscard,
            Surface::Web { page: Page::Health },
        )
        .unwrap();
        assert!(!full.exists());
        assert_eq!(
            item.unit,
            Unit::QuarantinedIngest {
                day: "20261120".to_owned(),
                segment: "120000_300".to_owned(),
                stamp: "1763640000000".to_owned(),
                file: "audio.flac".to_owned(),
            }
        );

        solstone_core_trash::attach(
            root.path(),
            &[&QuarantinedIngest],
            &item.id,
            Action::Recover,
            Surface::Web {
                page: Page::Settings,
            },
        )
        .unwrap();
        assert_eq!(fs::read(&full).unwrap(), b"quarantined audio");

        let again = QuarantinedIngest::trash(
            root.path(),
            rel,
            Action::QuarantinedIngestDiscard,
            Surface::Web { page: Page::Health },
        )
        .unwrap();
        solstone_core_trash::erase(
            root.path(),
            &[&QuarantinedIngest],
            &again.id,
            Action::Erase,
            Surface::Web {
                page: Page::Settings,
            },
        )
        .unwrap();
        assert!(!full.exists());
        assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn traversal_or_non_quarantine_paths_are_rejected() {
        let root = TempDir::new().unwrap();
        for bad in [
            "chronicle/20261120/field/120000_300/audio.flac",
            "chronicle/20261120/observer/failed/../120000_300/100/audio.flac",
            "chronicle/20261120/observer/failed/120000_300/notdigits/audio.flac",
            "chronicle/20261120/observer/failed/120000_300/100/.hidden",
        ] {
            assert!(
                QuarantinedIngest::trash(
                    root.path(),
                    bad,
                    Action::QuarantinedIngestDiscard,
                    Surface::Web { page: Page::Health },
                )
                .is_err(),
                "{bad}"
            );
        }
    }
}
