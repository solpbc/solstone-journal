// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::durability::{ArtifactId, DurableRead, read_json_durable, set_aside};

use super::error::FacetStoreError;
use super::paths::declaration_path;

/// Complete read-compatible facet declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FacetDeclarationSnapshot {
    pub title: String,
    pub description: String,
    pub color: String,
    pub emoji: String,
    pub icon: Option<String>,
    pub muted: Option<bool>,
    value: Value,
}

impl FacetDeclarationSnapshot {
    /// The original durable declaration, including unknown fields.
    pub fn value(&self) -> &Value {
        &self.value
    }

    pub(super) fn into_value(self) -> Value {
        self.value
    }
}

/// Read one facet declaration without persisting a fallback identity.
pub fn read_facet_declaration(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<Option<FacetDeclarationSnapshot>, FacetStoreError> {
    let path = declaration_path(journal_root, facet_dir)?;
    let value: Value =
        match read_json_durable(ArtifactId::FacetDeclaration, &path).map_err(|e| {
            FacetStoreError::from(solstone_core_journal_io::ReadError::Io {
                path: path.clone(),
                source: e,
            })
        })? {
            DurableRead::Present(val) => val,
            DurableRead::Absent | DurableRead::SetAside(_) | DurableRead::Unreadable { .. } => {
                return Ok(None);
            }
        };
    if value.is_null() {
        return Ok(None);
    }
    let Some(object) = value.as_object() else {
        let _ = set_aside(&path);
        return Ok(None);
    };
    Ok(Some(FacetDeclarationSnapshot {
        title: string_field(object.get("title")),
        description: string_field(object.get("description")),
        color: string_field(object.get("color")),
        emoji: string_field(object.get("emoji")),
        icon: non_empty_string(object.get("icon")).map(str::to_owned),
        muted: object.get("muted").and_then(Value::as_bool),
        value,
    }))
}

/// Stable lifecycle identity for a prepared facet-owned mutation. This read
/// never allocates an identity for a missing or unadopted declaration.
pub fn facet_write_identity(root: &Path, facet: &str) -> Result<String, String> {
    let declaration = read_facet_declaration(root, facet)
        .map_err(|e| e.to_string())?
        .ok_or("conflict: owning facet no longer exists")?;
    let id = declaration
        .value()
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| super::facet_id::is_well_formed_facet_id(id))
        .ok_or("conflict: owning facet has no valid stable identity")?;
    Ok(id.to_owned())
}

/// The caller holds facet trust through this check, mutation and receipt.
pub fn require_facet_write_identity(
    root: &Path,
    facet: &str,
    expected: &str,
) -> Result<(), String> {
    if facet_write_identity(root, facet)? != expected {
        return Err("conflict: owning facet was replaced after preparation".into());
    }
    Ok(())
}

fn string_field(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}
