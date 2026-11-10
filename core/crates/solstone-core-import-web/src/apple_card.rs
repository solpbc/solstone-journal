// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the Apple Health card adds to the import page's own preview and start:
//! whether the import fits on the disk, said at the preview and checked again
//! at the start.

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_import::{ImportMetadata, ProjectionStatus, read_import_metadata};
use solstone_core_import_sources::body_space::{Space, apple_space, size_words, too_full_words};

use crate::lifecycle::is_apple_card;

/// The largest `export.xml` an Apple Health import that finished in this
/// journal read, as its preview recorded it. A finished import that recorded
/// none (one run from the command line) covers nothing here, so a later import
/// is asked for the room a first one needs.
fn apple_export_covered_here(root: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(root.join("imports")) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            if id.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                return None;
            }
            let metadata = read_import_metadata(root, &id).ok()?;
            if metadata.get("source_hint").and_then(Value::as_str) != Some("apple_health")
                || solstone_core_import::project_import_result(root, &id).status
                    != ProjectionStatus::Success
            {
                return None;
            }
            metadata
                .get("preview")?
                .get("size")?
                .get("apple_health")?
                .get("export_bytes")?
                .as_u64()
        })
        .max()
        .unwrap_or(0)
}

/// The space an Apple Health card import needs and has now, from the size its
/// preview read. `None` when the record isn't the card's, has no preview yet,
/// or free space can't be read here, which blocks nothing.
pub(crate) fn space(root: &Path, metadata: &ImportMetadata, size: &Value) -> Option<Space> {
    if !is_apple_card(metadata) {
        return None;
    }
    let xml = size.get("apple_health")?.get("export_bytes")?.as_u64()?;
    apple_space(root, xml, apple_export_covered_here(root)).ok()
}

/// Add the space line to a card's preview response, beside its size: never
/// inside it, because a start compares sizes and free space moves.
pub(crate) fn add_space(root: &Path, import_id: &str, preview: &mut Value) {
    let Ok(metadata) = read_import_metadata(root, import_id) else {
        return;
    };
    if crate::lifecycle::is_apple_card(&metadata) {
        crate::lifecycle::sweep_ended_apple_uploads(root);
    }
    let Some(space) = space(root, &metadata, &preview["size"]) else {
        return;
    };
    preview["space"] = json!({
        "need": space.need,
        "free": space.free,
        "fits": space.fits,
        "need_words": size_words(space.need),
        "free_words": size_words(space.free),
        "words": (!space.fits).then(|| too_full_words(&space)),
    });
}

/// The words a start is refused with when the import doesn't fit now.
pub(crate) fn refuse_start_for_space(root: &Path, import_id: &str) -> Option<String> {
    let metadata = read_import_metadata(root, import_id).ok()?;
    let size = metadata.get("preview")?.get("size")?.clone();
    let space = space(root, &metadata, &size)?;
    (!space.fits).then(|| too_full_words(&space))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(root: &Path, id: &str, source: &str, export_bytes: Option<u64>, done: bool) {
        let mut metadata = ImportMetadata::new();
        metadata.insert("source_hint".into(), json!(source));
        if let Some(bytes) = export_bytes {
            metadata.insert(
                "preview".into(),
                json!({"size": {"apple_health": {"export_bytes": bytes}}}),
            );
        }
        solstone_core_import::write_import_metadata(root, id, &metadata).unwrap();
        if done {
            let facts =
                solstone_core_import::admit_running_attempt(root, id, 1_000, Some(source)).unwrap();
            solstone_core_import::record_completed_attempt(
                root,
                id,
                facts.generation,
                1_001,
                None,
                None,
            )
            .unwrap();
            solstone_core_import::metadata::release_attempt(root, id);
        }
    }

    #[test]
    fn what_earlier_apple_imports_covered_is_the_largest_finished_one_that_says() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        std::fs::create_dir_all(root.join("imports")).unwrap();
        assert_eq!(apple_export_covered_here(root), 0, "none yet");
        finished(root, "20261001_120000", "apple_health", Some(1_000), true);
        finished(root, "20261002_120000", "apple_health", Some(5_000), true);
        assert_eq!(
            solstone_core_import::project_import_result(root, "20261002_120000").status,
            ProjectionStatus::Success
        );
        // Not finished, another source, or a command-line run that recorded no size.
        finished(root, "20261003_120000", "apple_health", Some(9_000), false);
        finished(root, "20261004_120000", "strava", Some(99_000), true);
        finished(root, "20261005_120000", "apple_health", None, true);
        assert_eq!(apple_export_covered_here(root), 5_000);
    }
}
