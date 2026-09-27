// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_entity::facet_links::{LinkDirs, LinkFolderError};

use super::error::FacetStoreError;

/// One persisted facet-to-journal entity link with its original relationship object.
#[derive(Debug, Clone, PartialEq)]
pub struct FacetEntityLinkSnapshot {
    entity_id: String,
    written: bool,
    value: Value,
}

impl FacetEntityLinkSnapshot {
    /// Effective stored-or-directory journal entity identifier.
    pub fn entity_id(&self) -> &str {
        &self.entity_id
    }

    /// Whether the id was explicitly stored rather than falling back to the directory name.
    pub fn was_written(&self) -> bool {
        self.written
    }

    /// Full original relationship object, including unknown fields.
    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// Read a facet-scoped relationship and its durable cross-reference.
pub fn read_facet_entity_link(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<Option<FacetEntityLinkSnapshot>, FacetStoreError> {
    let link = LinkDirs::for_facet(journal_root, facet_dir)
        .read_link(entity_dir)
        .map_err(|error| match error {
            LinkFolderError::Path(error) => FacetStoreError::Path(error),
            LinkFolderError::Read(error) => FacetStoreError::Read(error),
            LinkFolderError::NotObject { path } => FacetStoreError::EntityLinkNotObject { path },
            other => unreachable!("reading a link only fails to read: {other}"),
        })?;
    Ok(link.map(|link| FacetEntityLinkSnapshot {
        entity_id: link.entity_id,
        written: link.id_written,
        value: link.value,
    }))
}
