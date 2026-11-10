// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the Apple Health card adds to the import page's own preview and start:
//! whether the import fits on the disk, said at the preview and checked again
//! at the start.

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_import::{ImportMetadata, ProjectionStatus, read_import_metadata};
use solstone_core_import_sources::body_space::{Space, apple_space, too_full_words};

use crate::lifecycle::is_apple_card;

/// Whether an Apple Health import has finished in this journal: after one has,
/// another writes mostly what's already there.
fn apple_import_finished_here(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root.join("imports")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let id = entry.file_name().to_string_lossy().into_owned();
        !id.starts_with('.')
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && read_import_metadata(root, &id).is_ok_and(|metadata| {
                metadata.get("source_hint").and_then(Value::as_str) == Some("apple_health")
            })
            && solstone_core_import::project_import_result(root, &id).status
                == ProjectionStatus::Success
    })
}

/// The space an Apple Health card import needs and has now, from the size its
/// preview read. `None` when the record isn't the card's, has no preview yet,
/// or free space can't be read here, which blocks nothing.
pub(crate) fn space(root: &Path, metadata: &ImportMetadata, size: &Value) -> Option<Space> {
    if !is_apple_card(metadata) {
        return None;
    }
    let xml = size.get("apple_health")?.get("export_bytes")?.as_u64()?;
    apple_space(root, xml, !apple_import_finished_here(root)).ok()
}

/// Add the space line to a card's preview response, beside its size: never
/// inside it, because a start compares sizes and free space moves.
pub(crate) fn add_space(root: &Path, import_id: &str, preview: &mut Value) {
    let Ok(metadata) = read_import_metadata(root, import_id) else {
        return;
    };
    let Some(space) = space(root, &metadata, &preview["size"]) else {
        return;
    };
    preview["space"] = json!({
        "need": space.need,
        "free": space.free,
        "fits": space.fits,
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
