// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What an agent's `body` permission covers, by where it is kept: everything a
//! body import brought into the journal.
//!
//! The index keeps the category each path was stored under, and a body import's
//! files were stored as transcripts before `body` existed. So `body` is never
//! read from a stored category: every reader decides it from the path, and a row
//! indexed before the change is gated exactly like one indexed after it.

use std::path::Path;

/// The streams a body import writes its windows, workouts and talent outputs to.
pub const BODY_IMPORT_STREAMS: [&str; 3] = ["import.apple_health", "import.oura", "import.strava"];

/// The import sources whose summaries describe body data.
pub const BODY_IMPORT_SOURCES: [&str; 3] = ["apple_health", "oura", "strava"];

/// The producer records the output paths in the manifest, and stamps the
/// import ID and kind on each tile. Both records must describe the same job.
pub(super) fn text_summary_permission(
    journal: &Path,
    rel: &str,
    manifest: &serde_json::Value,
) -> Option<super::AdmittedCategory> {
    use super::AdmittedCategory;
    let id = summary_import_id(rel)?;
    use solstone_core_journal_io::media::MediaDirectory;
    let root = std::sync::Arc::new(solstone_core_journal_io::JournalRoot::open(journal).ok()?);
    let mut kind = None;
    for file in manifest["files_created"].as_array()? {
        let path = file.as_str()?.replace('\\', "/");
        let parts: Vec<_> = path.rsplit('/').take(5).collect();
        let [name, segment, stream, day, "chronicle"] = parts.as_slice() else {
            return None;
        };
        if !day.bytes().all(|c| c.is_ascii_digit())
            || day.len() != 8
            || crate::segment::segment_parse(segment).is_none()
        {
            return None;
        }
        let expected = match *name {
            "note_transcript.md" => ("notes", AdmittedCategory::Notes),
            "document_transcript.md" => ("documents", AdmittedCategory::Documents),
            _ => return None,
        };
        let tile_dir = Path::new("chronicle").join(day).join(stream).join(segment);
        if super::kinds::segment_file_permission(&format!(
            "chronicle/{day}/{stream}/{segment}/{name}"
        )) != expected.1
        {
            return None;
        }
        let directory = MediaDirectory::open(std::sync::Arc::clone(&root), &tile_dir).ok()?;
        directory
            .read_versioned(std::ffi::OsStr::new(name), |_, _| ())
            .ok()?;
        let metadata = tile_dir.join(super::IMPORT_KIND_FILENAME);
        let bytes =
            solstone_core_journal_io::read_relative_file_bounded(&root, &metadata, 1024 * 1024)
                .ok()??;
        let tile: serde_json::Value = serde_json::from_slice(&bytes.bytes).ok()?;
        if tile["kind"] != expected.0
            || tile["import_id"] != id
            || kind.is_some_and(|kind| kind != expected.1)
        {
            return None;
        }
        kind = Some(expected.1);
    }
    kind
}

/// Whether an indexed path is body material by its path alone: anything under a
/// body import's stream, or a day's body document.
///
/// A body import's summary is body material too, but only its manifest says so:
/// see [`is_body_import_summary`].
#[must_use]
pub fn is_body_path(rel: &str) -> bool {
    let rel = rel.strip_prefix("chronicle/").unwrap_or(rel);
    let mut parts = rel.split('/');
    let Some(day) = parts.next() else {
        return false;
    };
    match parts.next() {
        Some("body.md") => {
            parts.next().is_none()
                && day.len() == 8
                && day.bytes().all(|byte| byte.is_ascii_digit())
        }
        Some(stream) => BODY_IMPORT_STREAMS.contains(&stream) && parts.next().is_some(),
        None => false,
    }
}

/// The import id of `imports/<id>/summary.md`.
pub(super) fn summary_import_id(rel: &str) -> Option<&str> {
    let id = rel.strip_prefix("imports/")?.strip_suffix("/summary.md")?;
    (!id.is_empty() && !id.contains('/') && !id.starts_with('.')).then_some(id)
}

/// Whether `rel` is the summary of a body import: what it read, by kind and
/// count. An unreadable manifest says nothing, so its summary stays what it was.
#[must_use]
pub fn is_body_import_summary(journal: &Path, rel: &str) -> bool {
    import_summary_source(journal, rel)
        .is_some_and(|source| BODY_IMPORT_SOURCES.contains(&source.as_str()))
}

pub(super) fn import_summary_source(journal: &Path, rel: &str) -> Option<String> {
    let manifest = import_summary_manifest(journal, rel)?;
    Some(manifest["source_type"].as_str()?.to_owned())
}

pub(super) fn import_summary_manifest(journal: &Path, rel: &str) -> Option<serde_json::Value> {
    let id = summary_import_id(rel)?;
    let root = solstone_core_journal_io::JournalRoot::open(journal).ok()?;
    let path = Path::new("imports").join(id).join("manifest.json");
    let bytes =
        solstone_core_journal_io::read_relative_file_bounded(&root, &path, 1024 * 1024).ok()??;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes.bytes).ok()?;
    if manifest
        .get("import_id")
        .is_some_and(|stored| stored.as_str() != Some(id))
    {
        return None;
    }
    Some(manifest)
}

/// Whether an indexed path is body material, its path read first and a summary's
/// manifest only when the path is a summary's.
#[must_use]
pub fn is_body_material(journal: Option<&Path>, rel: &str) -> bool {
    is_body_path(rel) || journal.is_some_and(|journal| is_body_import_summary(journal, rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_paths_are_a_body_imports_streams_and_the_day_document() {
        for rel in [
            "20260301/import.apple_health/090000_3600/body.jsonl",
            "20260301/import.oura/000000_86400/body.jsonl",
            "20260301/import.strava/070000_300/workout.json",
            "20260301/import.strava/070000_300/talents/activity.md",
            "chronicle/20260301/import.strava/070000_300/talents/work/x.md",
            "20260301/body.md",
        ] {
            assert!(is_body_path(rel), "{rel}");
        }
        for rel in [
            "20260301/location.md",
            "20260301/import.granola/090000_300/imported.md",
            "20260301/audio/090000_300/talents/transcript.md",
            "facets/work/activities/20260301.jsonl",
            "imports/abc/summary.md",
            "2026030/body.md",
            "20260301/import.strava",
        ] {
            assert!(!is_body_path(rel), "{rel}");
        }
    }

    #[test]
    fn a_summary_is_body_material_only_when_its_manifest_names_a_body_source() {
        let journal = tempfile::tempdir().unwrap();
        for (id, source) in [("one", "oura"), ("two", "granola")] {
            let dir = journal.path().join("imports").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("manifest.json"),
                format!(r#"{{"source_type":"{source}"}}"#),
            )
            .unwrap();
        }
        assert!(is_body_material(
            Some(journal.path()),
            "imports/one/summary.md"
        ));
        assert!(!is_body_material(
            Some(journal.path()),
            "imports/two/summary.md"
        ));
        assert!(!is_body_material(
            Some(journal.path()),
            "imports/none/summary.md"
        ));
        assert!(!is_body_material(None, "imports/one/summary.md"));
    }
}
