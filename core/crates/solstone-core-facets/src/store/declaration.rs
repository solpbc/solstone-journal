// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{MalformedPolicy, read_json};

use super::error::FacetStoreError;
use super::paths::declaration_path;

/// Complete read-compatible facet declaration with its effective identity.
#[derive(Debug, Clone, PartialEq)]
pub struct FacetDeclarationSnapshot {
    pub id: String,
    pub title: String,
    pub description: String,
    pub color: String,
    pub emoji: String,
    pub icon: Option<String>,
    pub muted: Option<bool>,
    written_id: bool,
    value: Value,
}

impl FacetDeclarationSnapshot {
    /// Whether `id` came from a non-empty durable field rather than `facet_dir`.
    pub fn was_written(&self) -> bool {
        self.written_id
    }

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
    let stored_id = non_empty_string(object.get("id")).map(str::to_owned);
    Ok(Some(FacetDeclarationSnapshot {
        id: stored_id.clone().unwrap_or_else(|| facet_dir.to_owned()),
        title: string_field(object.get("title")),
        description: string_field(object.get("description")),
        color: string_field(object.get("color")),
        emoji: string_field(object.get("emoji")),
        icon: non_empty_string(object.get("icon")).map(str::to_owned),
        muted: object.get("muted").and_then(Value::as_bool),
        written_id: stored_id.is_some(),
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
