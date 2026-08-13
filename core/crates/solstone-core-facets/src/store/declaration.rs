// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{MalformedPolicy, read_json};

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
    let value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)?;
    if value.is_null() {
        return Ok(None);
    }
    let Some(object) = value.as_object() else {
        return Err(FacetStoreError::DeclarationNotObject { path });
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

fn string_field(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}
