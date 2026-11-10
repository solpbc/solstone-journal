// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whether an Apple Health import fits on the disk, said before it starts.
//!
//! An import stages the export sorted before it writes the journal, and that
//! staging is given back when it finishes. The need is worked out from the
//! size of `export.xml` unpacked, by factors measured on a whole synthetic
//! history (see `docs/health_imports.md`), against the free space on the
//! volume staging writes to and on the journal's, whichever is tighter.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::body_stream::APPLE;

const GIB: u64 = 1024 * 1024 * 1024;
/// Kept free beyond what the import needs.
pub const RESERVE: u64 = GIB;
/// Peak staging, per byte of `export.xml`, measured on a synthetic six-year
/// history (2.34 GB of `export.xml`, 2.09 million rows): 1.13 on a Mac (APFS),
/// 0.91 on Linux. The larger is used.
pub const STAGING_PER_XML_BYTE: f64 = 1.13;
/// What a first import leaves in the journal, per byte of `export.xml`, on the
/// same history: 1.29 on Linux, 0.78 on a Mac. The larger is used.
pub const GROWTH_PER_XML_BYTE: f64 = 1.30;
/// The most the disk held above where it began during that import, per byte of
/// `export.xml`: 1.83 on Linux, 1.32 on a Mac. Staging plus growth covers both.
#[cfg(test)]
const MEASURED_PEAKS: [f64; 2] = [1.83, 1.32];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Space {
    pub need: u64,
    pub free: u64,
    pub fits: bool,
}

/// The nearest existing directory at or above `path`.
fn existing(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.is_dir())
        .map(Path::to_path_buf)
}

/// The free space an Apple import of an `export.xml` this size can count on.
pub fn free_for_apple(journal: &Path) -> Result<u64, String> {
    free_for_apple_with(journal, &crate::archive::available_bytes)
}

fn free_for_apple_with(
    journal: &Path,
    available: &dyn Fn(&Path) -> Result<u64, String>,
) -> Result<u64, String> {
    let staging =
        existing(&journal.join(APPLE.run_dir_rel)).ok_or("the journal folder is missing")?;
    let at_staging = available(&staging)?;
    let at_journal = available(journal)?;
    Ok(at_staging.min(at_journal))
}

/// How many bytes are under `dir`, counting what can be read.
fn bytes_under(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => bytes_under(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |meta| meta.len()),
            _ => 0,
        })
        .sum()
}

/// What an import of an `export.xml` of `xml_bytes` needs, before anything is
/// counted as free. `covered` is the largest `export.xml` an Apple import that
/// finished here is known to have read (0 when none has, or none says): an
/// export holds the whole history, so what it leaves in the journal grows only
/// with what it holds beyond that. Staging a stopped run left is given back
/// when the next run starts, so it's taken off.
#[must_use]
pub fn need(xml_bytes: u64, covered: u64, staging_now: u64) -> u64 {
    let xml = xml_bytes as f64;
    let added = xml_bytes.saturating_sub(covered) as f64;
    let need = xml * STAGING_PER_XML_BYTE + added * GROWTH_PER_XML_BYTE;
    ((need as u64).saturating_add(RESERVE)).saturating_sub(staging_now)
}

/// Whether an Apple import fits now.
pub fn apple_space(journal: &Path, xml_bytes: u64, covered: u64) -> Result<Space, String> {
    apple_space_with(
        journal,
        xml_bytes,
        covered,
        &crate::archive::available_bytes,
    )
}

fn apple_space_with(
    journal: &Path,
    xml_bytes: u64,
    covered: u64,
    available: &dyn Fn(&Path) -> Result<u64, String>,
) -> Result<Space, String> {
    let staging_now = bytes_under(&journal.join(APPLE.run_dir_rel));
    let need = need(xml_bytes, covered, staging_now);
    let free = free_for_apple_with(journal, available)?;
    Ok(Space {
        need,
        free,
        fits: free >= need,
    })
}

/// A size as an owner reads it: gigabytes to one decimal, megabytes under one.
#[must_use]
pub fn size_words(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else {
        format!("{} MB", (bytes / 1_000_000.0).ceil() as u64)
    }
}

/// Why a start was refused for space, in the owner's words.
#[must_use]
pub fn too_full_words(space: &Space) -> String {
    format!(
        "this import needs about {} free, and this disk has {} free, as the system reports it. free up about {}, then start it.",
        size_words(space.need),
        size_words(space.free),
        size_words(space.need.saturating_sub(space.free))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tighter_of_the_two_volumes_decides() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path().to_path_buf();
        let staging_parent = root.join("imports");
        std::fs::create_dir_all(&staging_parent).unwrap();
        let tight_at_staging = |path: &Path| Ok(if path == staging_parent { 5 } else { 50 });
        assert_eq!(free_for_apple_with(&root, &tight_at_staging).unwrap(), 5);
        let tight_at_journal = |path: &Path| Ok(if path == root { 7 } else { 50 });
        assert_eq!(free_for_apple_with(&root, &tight_at_journal).unwrap(), 7);
    }

    #[test]
    fn a_first_import_counts_what_it_leaves_and_staging_already_there_is_taken_off() {
        let xml = 10 * GIB;
        assert!(need(xml, 0, 0) > need(xml, xml, 0));
        for peak in MEASURED_PEAKS {
            assert!(
                need(xml, 0, 0) as f64 >= xml as f64 * peak + RESERVE as f64,
                "a first import's need covers the measured peak {peak}, with the reserve kept"
            );
        }
        assert_eq!(need(xml, xml, GIB), need(xml, xml, 0) - GIB);
        assert_eq!(need(0, 0, 5 * GIB), 0);
        let journal = tempfile::tempdir().unwrap();
        let staging = journal.path().join(APPLE.run_dir_rel);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("run"), vec![0_u8; 4096]).unwrap();
        let roomy = |_: &Path| Ok(u64::MAX);
        let space = apple_space_with(journal.path(), 0, 0, &roomy).unwrap();
        assert_eq!(space.need, RESERVE - 4096);
        assert!(space.fits);
        let full = |_: &Path| Ok(RESERVE / 2);
        let short = apple_space_with(journal.path(), 0, 0, &full).unwrap();
        assert!(!short.fits);
        assert!(
            too_full_words(&short).contains("free up about 537 MB"),
            "{}",
            too_full_words(&short)
        );
    }

    #[test]
    fn a_later_import_counts_growth_for_what_it_holds_beyond_earlier_ones() {
        let small = GIB;
        let long = 8 * GIB;
        // The same history again: staging only.
        assert_eq!(
            need(long, long, 0),
            (long as f64 * STAGING_PER_XML_BYTE) as u64 + RESERVE
        );
        // A much longer export after a small first one: what it adds grows the
        // journal as a first import would.
        let added = long - small;
        assert_eq!(
            need(long, small, 0),
            (long as f64 * STAGING_PER_XML_BYTE + added as f64 * GROWTH_PER_XML_BYTE) as u64
                + RESERVE
        );
        assert!(need(long, small, 0) > need(long, long, 0));
        // A shorter export than one already held adds nothing.
        assert_eq!(need(small, long, 0), need(small, small, 0));
    }
}
