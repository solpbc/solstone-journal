// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_journal_io::contained_path;

use super::error::EntityStoreError;

pub(super) fn identity_path(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<PathBuf, EntityStoreError> {
    contained_path(journal_root, &format!("entities/{entity_dir}/entity.json")).map_err(Into::into)
}

pub(super) fn events_dir(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<PathBuf, EntityStoreError> {
    contained_path(
        journal_root,
        &format!("entities/{entity_dir}/history/events"),
    )
    .map_err(Into::into)
}

pub(super) fn prepared_dir(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<PathBuf, EntityStoreError> {
    contained_path(
        journal_root,
        &format!("entities/{entity_dir}/history/prepared"),
    )
    .map_err(Into::into)
}

pub(super) fn ambiguities_path(journal_root: &Path) -> Result<PathBuf, EntityStoreError> {
    contained_path(journal_root, "entities/ambiguities.jsonl").map_err(Into::into)
}

pub(super) fn review_candidates_path(journal_root: &Path) -> Result<PathBuf, EntityStoreError> {
    contained_path(journal_root, "entities/review-candidates.jsonl").map_err(Into::into)
}

pub(super) fn identity_map_cache_path(journal_root: &Path) -> Result<PathBuf, EntityStoreError> {
    contained_path(journal_root, "entities/.identity-map-cache.json").map_err(Into::into)
}
