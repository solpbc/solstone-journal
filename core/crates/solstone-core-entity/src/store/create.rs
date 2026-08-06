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
    if !skip_principal
        && entity_matches_identity_name(name, aka, identity_names)
        && !has_journal_principal(journal_root)?
    {
        identity.insert("is_principal".to_owned(), Value::Bool(true));
    }

    save_entity_identity(journal_root, entity_id, &Value::Object(identity), operation)
        .map_err(Into::into)
}
