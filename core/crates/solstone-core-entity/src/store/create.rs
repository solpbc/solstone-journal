// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Guarded construction of journal entity identities.

use std::path::Path;

use chrono::Utc;
use serde_json::{Map, Value};

use crate::hold_entity_trust_lock;

use super::derived::entity_matches_identity_name;
use super::lifecycle::{EntityLifecycleError, has_journal_principal};
use super::map::read_identity_map;
use super::write::{EntityOperationContext, EntitySaveResult, save_entity_identity};

/// Create a new entity only when its identity id is not already resolved.
#[allow(clippy::too_many_arguments)] // Public API mirrors the Python construction inputs.
pub fn create_journal_entity(
    journal_root: &Path,
    entity_id: &str,
    name: &str,
    entity_type: &str,
    aka: Option<&[String]>,
    emails: Option<&[String]>,
    identity_names: &[String],
    skip_principal: bool,
    operation: Option<&EntityOperationContext>,
) -> Result<EntitySaveResult, EntityLifecycleError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    if read_identity_map(journal_root)?
        .resolved
        .contains_key(entity_id)
    {
        return Err(EntityLifecycleError::EntityAlreadyExists {
            entity_id: entity_id.to_owned(),
        });
    }

    let mut identity = Map::from_iter([
        ("id".to_owned(), Value::String(entity_id.to_owned())),
        ("name".to_owned(), Value::String(name.to_owned())),
        ("type".to_owned(), Value::String(entity_type.to_owned())),
        (
            "created_at".to_owned(),
            Value::Number(Utc::now().timestamp_millis().into()),
        ),
    ]);
    if let Some(aka) = aka.filter(|aka| !aka.is_empty()) {
        identity.insert(
            "aka".to_owned(),
            Value::Array(aka.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(emails) = emails.filter(|emails| !emails.is_empty()) {
        identity.insert(
            "emails".to_owned(),
            Value::Array(
                emails
                    .iter()
                    .map(|email| Value::String(email.to_lowercase()))
                    .collect(),
            ),
        );
    }
    if !skip_principal && becomes_journal_principal(journal_root, name, aka, identity_names)? {
        identity.insert("is_principal".to_owned(), Value::Bool(true));
    }

    save_entity_identity(journal_root, entity_id, &Value::Object(identity), operation)
        .map_err(Into::into)
}

/// Whether a new entity named `name` becomes the journal's principal: it
/// matches one of the owner's names and the journal has no principal yet.
/// With no names given, the owner's configured names are used. Callers hold
/// the entity trust lock.
pub fn becomes_journal_principal(
    journal_root: &Path,
    name: &str,
    aka: Option<&[String]>,
    identity_names: &[String],
) -> Result<bool, EntityLifecycleError> {
    let configured;
    let identity_names = if identity_names.is_empty() {
        configured = journal_identity_names(journal_root);
        configured.as_slice()
    } else {
        identity_names
    };
    Ok(entity_matches_identity_name(name, aka, identity_names)
        && !has_journal_principal(journal_root)?)
}

/// The owner's configured names: preferred name, full name, then aliases,
/// without blanks or repeats. Empty when the config is absent or unreadable.
pub fn journal_identity_names(journal_root: &Path) -> Vec<String> {
    let config = solstone_core_journal_config::read_journal_config(journal_root)
        .ok()
        .and_then(|read| read.config);
    let Some(identity) = config
        .as_ref()
        .and_then(|config| config.get("identity"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let aliases = identity
        .get("aliases")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for value in [identity.get("preferred"), identity.get("name")]
        .into_iter()
        .flatten()
        .chain(aliases)
    {
        let Some(name) = value
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    names
}
