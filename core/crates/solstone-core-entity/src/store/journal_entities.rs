// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct journal-entity enumeration without identity-map collision resolution.

use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{DirEntryKind, contained_path, list_dir_entries};

use crate::EntityResolutionEntity;

use super::error::EntityStoreError;
use super::identity::read_entity_identity;
use super::lifecycle::value_is_truthy;

/// One directly enumerated journal entity and its durable identity payload.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalEntity {
    /// Effective written-or-directory entity ID.
    pub id: String,
    /// Full durable identity object with its effective ID stamped into `id`.
    pub value: Value,
}

impl JournalEntity {
    /// Return the raw entity type when it is a string.
    pub fn entity_type(&self) -> Option<&str> {
        self.value.get("type").and_then(Value::as_str)
    }

    /// Return whether the durable identity is marked as the journal principal.
    pub fn is_principal(&self) -> bool {
        self.value.get("is_principal").is_some_and(value_is_truthy)
    }

    /// Return whether the durable identity is blocked.
    pub fn is_blocked(&self) -> bool {
        self.value.get("blocked").is_some_and(value_is_truthy)
    }

    /// Project this durable record into the name-resolution candidate shape.
    pub fn resolution_entity(&self) -> EntityResolutionEntity {
        EntityResolutionEntity {
            id: Some(self.id.clone()),
            name: string_field(&self.value, "name"),
            aka: string_list_field(&self.value, "aka"),
            emails: string_list_field(&self.value, "emails"),
            blocked: self.is_blocked(),
        }
    }
}

/// Return whether this entity may be used as an active speaker identity.
pub fn is_admissible_person(entity: &JournalEntity) -> bool {
    entity.entity_type() == Some("Person") && !entity.is_blocked()
}

/// Load all directly enumerable journal entities in deterministic ID order.
///
/// Individual missing, malformed, or unreadable identities are skipped, matching
/// the Python journal reader. A failure enumerating the entities directory is
/// returned to the caller.
pub fn load_all_journal_entities(
    journal_root: &Path,
) -> Result<Vec<JournalEntity>, EntityStoreError> {
    let entities_dir = contained_path(journal_root, "entities")?;
    let mut entities = Vec::new();
    for entry in list_dir_entries(&entities_dir)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let entity_dir = entry.name.to_string_lossy().into_owned();
        match read_entity_identity(journal_root, &entity_dir) {
            Ok(Some(identity)) => entities.push(JournalEntity {
                id: identity.entity_id().to_owned(),
                value: identity.value().clone(),
            }),
            Ok(None) => {}
            Err(error) => log::warn!("failed to load journal entity {}: {}", entity_dir, error),
        }
    }
    entities.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(entities)
}

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn string_list_field(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{JournalEntity, is_admissible_person};

    #[test]
    fn admissible_person_requires_an_unblocked_exact_person_type() {
        let entity = |value| JournalEntity {
            id: "entity".to_owned(),
            value,
        };

        assert!(is_admissible_person(&entity(json!({"type":"Person"}))));
        assert!(!is_admissible_person(&entity(json!({"type":"Tool"}))));
        assert!(!is_admissible_person(&entity(json!({"type":"person"}))));
        assert!(!is_admissible_person(&entity(
            json!({"type":"Person","blocked":true})
        )));
        assert!(!is_admissible_person(&entity(json!({}))));
    }
}
