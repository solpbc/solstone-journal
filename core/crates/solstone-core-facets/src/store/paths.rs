// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_journal_io::contained_path;

use super::error::FacetStoreError;

#[derive(Clone, Copy)]
pub(super) enum FacetContentKind {
    Activities,
    News,
    Logs,
}

impl FacetContentKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Activities => "activities",
            Self::News => "news",
            Self::Logs => "logs",
        }
    }
}

pub(super) fn declaration_path(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<PathBuf, FacetStoreError> {
    contained_path(journal_root, &format!("facets/{facet_dir}/facet.json")).map_err(Into::into)
}

pub(super) fn facet_dir_path(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<PathBuf, FacetStoreError> {
    contained_path(journal_root, &format!("facets/{facet_dir}")).map_err(Into::into)
}

pub(super) fn facet_entities_dir(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<PathBuf, FacetStoreError> {
    contained_path(journal_root, &format!("facets/{facet_dir}/entities")).map_err(Into::into)
}

pub(super) fn facets_dir(journal_root: &Path) -> Result<PathBuf, FacetStoreError> {
    contained_path(journal_root, "facets").map_err(Into::into)
}

pub(super) fn review_candidates_path(journal_root: &Path) -> Result<PathBuf, FacetStoreError> {
    contained_path(journal_root, "facets/review-candidates.jsonl").map_err(Into::into)
}

pub(super) fn facet_entity_link_path(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<PathBuf, FacetStoreError> {
    contained_path(
        journal_root,
        &format!("facets/{facet_dir}/entities/{entity_dir}/entity.json"),
    )
    .map_err(Into::into)
}

pub(super) fn facet_entity_observations_path(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<PathBuf, FacetStoreError> {
    contained_path(
        journal_root,
        &format!("facets/{facet_dir}/entities/{entity_dir}/observations.jsonl"),
    )
    .map_err(Into::into)
}

pub(super) fn facet_entity_link_repair_marker_path(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<PathBuf, FacetStoreError> {
    contained_path(
        journal_root,
        &format!("facets/{facet_dir}/health/migrations/entity-link-repair.json"),
    )
    .map_err(Into::into)
}

pub(super) fn facet_entity_link_journal_wide_repair_marker_path(
    journal_root: &Path,
) -> Result<PathBuf, FacetStoreError> {
    contained_path(
        journal_root,
        "health/migrations/facet-entity-link-repair.json",
    )
    .map_err(Into::into)
}

pub(super) fn activities_dir(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<PathBuf, FacetStoreError> {
    content_dir(journal_root, facet_dir, FacetContentKind::Activities)
}

pub(super) fn news_dir(journal_root: &Path, facet_dir: &str) -> Result<PathBuf, FacetStoreError> {
    content_dir(journal_root, facet_dir, FacetContentKind::News)
}

pub(super) fn logs_dir(journal_root: &Path, facet_dir: &str) -> Result<PathBuf, FacetStoreError> {
    content_dir(journal_root, facet_dir, FacetContentKind::Logs)
}

pub(super) fn content_file_path(
    journal_root: &Path,
    facet_dir: &str,
    kind: FacetContentKind,
    relative_path: &str,
) -> Result<PathBuf, FacetStoreError> {
    let directory = match kind {
        FacetContentKind::Activities => activities_dir(journal_root, facet_dir)?,
        FacetContentKind::News => news_dir(journal_root, facet_dir)?,
        FacetContentKind::Logs => logs_dir(journal_root, facet_dir)?,
    };
    contained_path(&directory, relative_path).map_err(Into::into)
}

fn content_dir(
    journal_root: &Path,
    facet_dir: &str,
    kind: FacetContentKind,
) -> Result<PathBuf, FacetStoreError> {
    contained_path(
        journal_root,
        &format!("facets/{facet_dir}/{}", kind.directory()),
    )
    .map_err(Into::into)
}
