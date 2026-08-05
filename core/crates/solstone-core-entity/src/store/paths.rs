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
