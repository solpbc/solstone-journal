// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Linked-client source-delete HTTP surface. Selection lives here; removal is
//! retention's door.

use std::path::PathBuf;

use axum::Router;
use axum::routing::delete;

mod delete;
mod not_confirmed;
mod receipt;
mod select;

pub fn router(journal_root: PathBuf) -> Router {
    Router::new()
        .route(
            "/app/devices/source/{source}",
            delete(delete::delete_source),
        )
        .with_state(journal_root)
}

#[cfg(test)]
mod tests {
    use solstone_core_journal_io::{DirEntryKind, list_dir_entries};
    use tempfile::TempDir;

    use super::select::{segment_is_mixed, select_location_targets};

    #[test]
    fn mixed_predicate_counts_talents_and_exempts_item_json() {
        let root = TempDir::new_in("/var/tmp").expect("bed");
        let mixed = root.path().join("mixed");
        std::fs::create_dir_all(mixed.join("talents")).unwrap();
        std::fs::write(mixed.join("location.jsonl"), b"{}").unwrap();
        std::fs::write(mixed.join("talents/sense.json"), b"{}").unwrap();
        let entries = list_dir_entries(&mixed).unwrap();
        assert!(segment_is_mixed(&entries));

        let clean = root.path().join("clean");
        std::fs::create_dir_all(&clean).unwrap();
        std::fs::write(clean.join("location.jsonl"), b"{}").unwrap();
        std::fs::write(clean.join("stream.json"), b"{}").unwrap();
        std::fs::write(clean.join("item.json"), b"{}").unwrap();
        let entries = list_dir_entries(&clean).unwrap();
        assert!(!segment_is_mixed(&entries));
        assert!(
            entries.iter().any(|entry| entry.kind == DirEntryKind::File
                && entry.name.to_string_lossy() == "item.json")
        );
        assert!(select_location_targets(root.path()).targets.is_empty());
    }
}
