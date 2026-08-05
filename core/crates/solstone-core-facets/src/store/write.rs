// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_entity::{EntityAmbiguityRescopeReport, rescope_facet_ambiguities};
use solstone_core_journal_io::{
    JsonWriteOptions, LockOptions, MalformedPolicy, hold_lock, path_lexists, read_json,
    remove_dir_all, write_json,
};

use crate::hold_facet_trust_lock;

use super::declaration::read_facet_declaration;
use super::error::{FacetRenameError, FacetStoreError, FacetWriteError};
use super::identity::read_facet_entity_link;
use super::paths::{convey_config_path, declaration_path, facet_dir_path, facet_entity_link_path};

/// Structured successful result for a physical facet-directory rename.
#[derive(Debug, Clone, PartialEq)]
pub struct FacetRenameResult {
    pub old_name: String,
    pub new_name: String,
    pub ambiguity_rescope: EntityAmbiguityRescopeReport,
    pub reindex_required: bool,
}

/// Create a declaration with a durable identity equal to its initial directory.
pub fn create_facet(
    journal_root: &Path,
    facet_dir: &str,
    title: &str,
    description: &str,
    color: &str,
    emoji: &str,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let mut declaration = Map::new();
    declaration.insert("id".to_owned(), Value::String(facet_dir.to_owned()));
    declaration.insert("title".to_owned(), Value::String(title.to_owned()));
    declaration.insert(
        "description".to_owned(),
        Value::String(description.to_owned()),
    );
    declaration.insert("color".to_owned(), Value::String(color.to_owned()));
    declaration.insert("emoji".to_owned(), Value::String(emoji.to_owned()));
    save_facet_declaration(journal_root, facet_dir, &Value::Object(declaration))
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
    let ambiguity_rescope = match rescope_facet_ambiguities(journal_root, old_name, new_name) {
        Ok(report) => report,
        Err(source) => {
            return Err(FacetRenameError::AmbiguityRescope {
                source,
                rollback: fs::rename(&new_path, &old_path).err(),
            });
        }
    };
    update_convey_facet_references(journal_root, old_name, new_name)?;
    Ok(FacetRenameResult {
        old_name: old_name.to_owned(),
        new_name: new_name.to_owned(),
        ambiguity_rescope,
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

fn update_convey_facet_references(
    journal_root: &Path,
    old_name: &str,
    new_name: &str,
) -> Result<(), FacetRenameError> {
    let path = convey_config_path(journal_root).map_err(FacetRenameError::Path)?;
    if !path_lexists(&path)
        .map_err(FacetStoreError::from)
        .map_err(FacetRenameError::Path)?
    {
        return Ok(());
    }
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(FacetRenameError::ConveyConfigLock)?;
    let mut config: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)
        .map_err(FacetStoreError::from)
        .map_err(FacetRenameError::ConveyConfigRead)?;
    let mut changed = false;
    if let Some(facets) = config.get_mut("facets").and_then(Value::as_object_mut) {
        if facets.get("selected").and_then(Value::as_str) == Some(old_name) {
            facets.insert("selected".to_owned(), Value::String(new_name.to_owned()));
            changed = true;
        }
        if let Some(order) = facets.get_mut("order").and_then(Value::as_array_mut) {
            for item in order {
                if item.as_str() == Some(old_name) {
                    *item = Value::String(new_name.to_owned());
                    changed = true;
                }
            }
        }
    }
    if changed {
        write_json(
            &path,
            &config,
            JsonWriteOptions {
                mode: Some(0o600),
                indent: Some(2),
                sort_keys: false,
            },
        )
        .map_err(FacetRenameError::ConveyConfigWrite)?;
    }
    Ok(())
}
