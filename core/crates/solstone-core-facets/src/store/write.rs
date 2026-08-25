// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_io::{JsonWriteOptions, path_lexists, remove_dir_all, write_json};

use crate::hold_facet_trust_lock;

use super::declaration::read_facet_declaration;
use super::error::{FacetRenameError, FacetStoreError, FacetWriteError};
use super::identity::read_facet_entity_link;
use super::paths::{declaration_path, facet_dir_path, facet_entity_link_path};

/// Structured successful result for a physical facet-directory rename.
#[derive(Debug, Clone, PartialEq)]
pub struct FacetRenameResult {
    pub old_name: String,
    pub new_name: String,
    pub reindex_required: bool,
}

/// Create a facet declaration in its requested directory.
pub fn create_facet(
    journal_root: &Path,
    facet_dir: &str,
    title: &str,
    description: &str,
    color: &str,
    emoji: &str,
    icon: Option<&str>,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let mut declaration = Map::new();
    declaration.insert("title".to_owned(), Value::String(title.to_owned()));
    declaration.insert(
        "description".to_owned(),
        Value::String(description.to_owned()),
    );
    declaration.insert("color".to_owned(), Value::String(color.to_owned()));
    declaration.insert("emoji".to_owned(), Value::String(emoji.to_owned()));
    if let Some(icon) = icon.filter(|icon| !icon.is_empty()) {
        declaration.insert("icon".to_owned(), Value::String(icon.to_owned()));
    }
    save_facet_declaration(journal_root, facet_dir, &Value::Object(declaration))
}

/// Delete a facet directory when its declaration exists.
pub fn delete_facet(journal_root: &Path, facet_dir: &str) -> Result<bool, FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = declaration_path(journal_root, facet_dir)?;
    if read_facet_declaration(journal_root, facet_dir)?.is_none() {
        return Ok(false);
    }
    remove_dir_all(journal_root, &format!("facets/{facet_dir}"))
        .map_err(FacetWriteError::EntityLinkRemoval)?;
    let _ = path;
    Ok(true)
}

/// Update facet metadata while preserving identity and unknown declaration fields.
pub fn update_facet(
    journal_root: &Path,
    facet_dir: &str,
    title: &str,
    description: &str,
    color: &str,
    emoji: &str,
    icon: Option<&str>,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = declaration_path(journal_root, facet_dir)?;
    let snapshot = read_facet_declaration(journal_root, facet_dir)?
        .ok_or(FacetWriteError::DeclarationMissing { path })?;
    let mut declaration = snapshot.into_value();
    let object = declaration
        .as_object_mut()
        .expect("facet declaration reader returns an object");
    object.insert("title".to_owned(), Value::String(title.to_owned()));
    object.insert(
        "description".to_owned(),
        Value::String(description.to_owned()),
    );
    object.insert("color".to_owned(), Value::String(color.to_owned()));
    object.insert("emoji".to_owned(), Value::String(emoji.to_owned()));
    match icon.filter(|icon| !icon.is_empty()) {
        Some(icon) => {
            object.insert("icon".to_owned(), Value::String(icon.to_owned()));
        }
        None => {
            object.remove("icon");
        }
    }
    save_facet_declaration(journal_root, facet_dir, &declaration)
}

/// Persist muted state using the compact true-or-absent representation.
pub fn set_facet_muted(
    journal_root: &Path,
    facet_dir: &str,
    muted: bool,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = declaration_path(journal_root, facet_dir)?;
    let snapshot = read_facet_declaration(journal_root, facet_dir)?
        .ok_or(FacetWriteError::DeclarationMissing { path })?;
    let mut declaration = snapshot.into_value();
    let object = declaration
        .as_object_mut()
        .expect("facet declaration reader returns an object");
    if muted {
        object.insert("muted".to_owned(), Value::Bool(true));
    } else {
        object.remove("muted");
    }
    save_facet_declaration(journal_root, facet_dir, &declaration)
}

/// Persist a facet relationship's resolved journal entity link.
pub fn save_facet_entity_link(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    journal_entity_id: &str,
    other_relationship_fields: &Map<String, Value>,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let mut relationship = other_relationship_fields.clone();
    relationship.insert(
        "entity_id".to_owned(),
        Value::String(journal_entity_id.to_owned()),
    );
    let path = facet_entity_link_path(journal_root, facet_dir, entity_dir)?;
    write_json(&path, &Value::Object(relationship), json_options())
        .map_err(FacetWriteError::EntityLinkWrite)
}

/// Set or clear a relationship's detached marker while retaining every other field.
pub fn set_facet_entity_link_detached(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    detached: bool,
) -> Result<bool, FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let Some(snapshot) = read_facet_entity_link(journal_root, facet_dir, entity_dir)? else {
        return Ok(false);
    };
    let mut relationship = snapshot.value().clone();
    let object = relationship
        .as_object_mut()
        .expect("facet entity link reader returns an object");
    let changed = if detached {
        if object.get("detached") == Some(&Value::Bool(true)) {
            false
        } else {
            object.insert("detached".to_owned(), Value::Bool(true));
            true
        }
    } else {
        object.remove("detached").is_some()
    };
    if !changed {
        return Ok(false);
    }
    let path = facet_entity_link_path(journal_root, facet_dir, entity_dir)?;
    write_json(&path, &relationship, json_options()).map_err(FacetWriteError::EntityLinkWrite)?;
    Ok(true)
}

/// Remove one complete facet relationship directory when its link exists.
pub fn delete_facet_entity_link(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<bool, FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    if read_facet_entity_link(journal_root, facet_dir, entity_dir)?.is_none() {
        return Ok(false);
    }
    remove_dir_all(
        journal_root,
        &format!("facets/{facet_dir}/entities/{entity_dir}"),
    )
    .map_err(FacetWriteError::EntityLinkRemoval)?;
    Ok(true)
}

/// Physically rename one facet directory and rescope durable ambiguity rows.
pub fn rename_facet(
    journal_root: &Path,
    old_name: &str,
    new_name: &str,
) -> Result<FacetRenameResult, FacetRenameError> {
    for name in [old_name, new_name] {
        if !valid_facet_name(name) {
            return Err(FacetRenameError::InvalidName {
                name: name.to_owned(),
            });
        }
    }
    let old_declaration =
        declaration_path(journal_root, old_name).map_err(FacetRenameError::Path)?;
    if read_facet_declaration(journal_root, old_name)
        .map_err(FacetRenameError::Path)?
        .is_none()
    {
        return Err(FacetRenameError::FacetMissing {
            path: old_declaration,
        });
    }
    let old_path = facet_dir_path(journal_root, old_name).map_err(FacetRenameError::Path)?;
    let new_path = facet_dir_path(journal_root, new_name).map_err(FacetRenameError::Path)?;
    if path_lexists(&new_path)
        .map_err(FacetStoreError::from)
        .map_err(FacetRenameError::Path)?
    {
        return Err(FacetRenameError::DestinationExists { path: new_path });
    }
    let _trust = hold_facet_trust_lock(journal_root).map_err(FacetRenameError::TrustLock)?;
    fs::rename(&old_path, &new_path).map_err(|source| FacetRenameError::DirectoryRename {
        old_path: old_path.clone(),
        new_path: new_path.clone(),
        source,
    })?;
    Ok(FacetRenameResult {
        old_name: old_name.to_owned(),
        new_name: new_name.to_owned(),
        reindex_required: true,
    })
}

pub(super) fn save_facet_declaration(
    journal_root: &Path,
    facet_dir: &str,
    declaration: &Value,
) -> Result<(), FacetWriteError> {
    let path = declaration_path(journal_root, facet_dir)?;
    write_json(&path, declaration, json_options()).map_err(FacetWriteError::DeclarationWrite)
}

fn json_options() -> JsonWriteOptions {
    JsonWriteOptions {
        mode: Some(0o600),
        indent: Some(2),
        sort_keys: false,
    }
}

fn valid_facet_name(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some(character) if character.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
}
