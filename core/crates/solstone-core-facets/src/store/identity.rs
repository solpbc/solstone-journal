// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{MalformedPolicy, read_json};

use super::error::FacetStoreError;
use super::paths::facet_entity_link_path;

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
    let path = facet_entity_link_path(journal_root, facet_dir, entity_dir)?;
    let value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)?;
    if value.is_null() {
        return Ok(None);
    }
    let Some(object) = value.as_object() else {
        return Err(FacetStoreError::EntityLinkNotObject { path });
    };
    let stored = object
        .get("entity_id")
        .and_then(Value::as_str)
        .filter(|entity_id| !entity_id.is_empty())
        .map(str::to_owned);
    Ok(Some(FacetEntityLinkSnapshot {
        entity_id: stored.clone().unwrap_or_else(|| entity_dir.to_owned()),
        written: stored.is_some(),
        value,
    }))
}
