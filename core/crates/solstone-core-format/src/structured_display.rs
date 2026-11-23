// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-time display projection for structured JSON/JSONL journal families.
//!
//! Files stay on disk in their canonical structured schemas and SQLite FTS holds
//! canonical English chunks. At read time, Search verifies the file's SHA-256
//! digest against `files.display_stamp` and re-projects the matching chunk in
//! the owner's requested locale without touching stored files or index rows.

use std::path::Path;

use sha2::{Digest, Sha256};
use solstone_core_journal_io::{JournalRoot, read_relative_file_bounded};
use solstone_core_window_catalog::ENGLISH;

use crate::content::{Family, produce_chunks, produce_chunks_for_locale, resolve_spec};

const MAX_STRUCTURED_FILE_BYTES: usize = 32 * 1024 * 1024;

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn family(rel: &str) -> Option<Family> {
    let relative = rel.strip_prefix("chronicle/").unwrap_or(rel);
    let family = resolve_spec(relative)?.family;
    match family {
        Family::Event
        | Family::ActionLog
        | Family::StructuredImport
        | Family::AiChat
        | Family::Browser
        | Family::FacetEntity
        | Family::Observation
        | Family::Documents
        | Family::Screen
        | Family::Workout
        | Family::BodyRecords
        | Family::ProtectedVideo
        | Family::DesktopActivity
        | Family::PossibleNames => Some(family),
        _ => None,
    }
}

/// Whether `rel` belongs to a structured JSON/JSONL family projected from source bytes.
#[must_use]
pub fn indexed_path(rel: &str) -> bool {
    family(rel).is_some()
}

/// The 64-hex SHA-256 `files.display_stamp` for a structured source file.
#[must_use]
pub fn index_stamp(rel: &str, bytes: &[u8]) -> Option<String> {
    indexed_path(rel).then(|| hash(bytes))
}

/// English and localized chunk pairs verified against a structured source file's digest.
pub struct IndexedStructured(Vec<(String, String)>);

impl IndexedStructured {
    /// Return the localized text for chunk `idx` when its indexed English matches
    /// and the localized rendering differs.
    #[must_use]
    pub fn get(&self, idx: i64, english: &str) -> Option<&str> {
        let (source, shown) = self.0.get(usize::try_from(idx).ok()?)?;
        (source == english && source != shown).then_some(shown)
    }

    /// Load and verify `rel` under `root` against `stamp`, rendering chunks in `locale`.
    #[must_use]
    pub fn load(root: &Path, rel: &str, stamp: &str, locale: &str) -> Option<Self> {
        let locale = solstone_core_window_catalog::packaged_locale(Some(locale));
        if locale == ENGLISH {
            return None;
        }
        let family = family(rel)?;
        let rel_path = Path::new(rel);
        if rel_path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return None;
        }
        let disk_rel = if rel_path
            .components()
            .next()?
            .as_os_str()
            .to_str()
            .is_some_and(crate::segment::is_date_key)
        {
            Path::new("chronicle").join(rel_path)
        } else {
            rel_path.to_owned()
        };
        let journal = JournalRoot::open(root).ok()?;
        let bytes = read_relative_file_bounded(&journal, &disk_rel, MAX_STRUCTURED_FILE_BYTES)
            .ok()??
            .bytes;
        if hash(&bytes) != stamp {
            return None;
        }
        let text = std::str::from_utf8(&bytes).ok()?;
        let logical_rel = rel.strip_prefix("chronicle/").unwrap_or(rel);
        let original = produce_chunks(family, logical_rel, text).chunks;
        let shown = produce_chunks_for_locale(family, logical_rel, text, locale).chunks;
        if original.len() != shown.len() {
            return None;
        }
        Some(Self(
            original
                .into_iter()
                .zip(shown)
                .map(|(a, b)| (a.content.trim().to_owned(), b.content.trim().to_owned()))
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_structured_verifies_stamp_and_rejects_drift_and_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let rel = "20261004/import.strava/070200_300/workout.json";
        let path = dir.path().join("chronicle").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = br#"{"tile":{"index":0,"count":1},"workout":{"name":"Morning Run","type":"Run","start":"2026-10-04T07:02:00-06:00","elapsed_seconds":1800}}"#;
        std::fs::write(&path, bytes).unwrap();
        let stamp = index_stamp(rel, bytes).unwrap();
        assert_eq!(stamp.len(), 64);

        // English returns None (no-op projection).
        assert!(IndexedStructured::load(dir.path(), rel, &stamp, ENGLISH).is_none());
        // Traversal rejected.
        assert!(IndexedStructured::load(dir.path(), "../outside.json", &stamp, "en-XA").is_none());
        // Wrong stamp rejected.
        assert!(IndexedStructured::load(dir.path(), rel, &"0".repeat(64), "en-XA").is_none());

        // When packaged with a non-English locale (e.g. private en-XA build), projection succeeds.
        let locale = solstone_core_window_catalog::packaged_locale(Some("en-XA"));
        if locale != ENGLISH {
            let loaded = IndexedStructured::load(dir.path(), rel, &stamp, locale).unwrap();
            let english = produce_chunks(Family::Workout, rel, std::str::from_utf8(bytes).unwrap())
                .chunks[0]
                .content
                .trim()
                .to_owned();
            let shown = loaded.get(0, &english).expect("localized workout chunk");
            assert_ne!(shown, english);
            assert!(shown.contains("Morning Run"));
            assert!(loaded.get(0, "wrong english").is_none());
        }
    }
}
