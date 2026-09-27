// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-entity enumeration, one entity per effective id.

use std::path::Path;

use serde_json::Value;

use crate::EntityResolutionEntity;

use super::error::EntityStoreError;
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

/// The journal's entities, one per effective id, in id order: the entity the
/// identity map resolves each id to. Of two folders claiming one id, only that
/// one is listed; the map reports the other. A missing, damaged or unreadable
/// identity holds no entity. A failure listing `entities/` is returned.
pub fn load_all_journal_entities(
    journal_root: &Path,
) -> Result<Vec<JournalEntity>, EntityStoreError> {
    Ok(live_journal_entities(journal_root)?
        .into_iter()
        .map(|(_, entity)| entity)
        .collect())
}

/// Like `load_all_journal_entities`, with the folder that holds each entity.
pub fn live_journal_entities(
    journal_root: &Path,
) -> Result<Vec<(String, JournalEntity)>, EntityStoreError> {
    let (groups, losers) = super::map::identity_groups(journal_root)?;
    for loser in losers {
        if let super::map::IdentityMapLoserReason::Malformed { message } = loser.reason {
            log::warn!(
                "failed to load journal entity {}: {message}",
                loser.entity_dir
            );
        }
    }
    Ok(groups
        .into_iter()
        .filter_map(|(id, members)| {
            let (dir, identity) = members.into_iter().next()?;
            Some((
                dir,
                JournalEntity {
                    id,
                    value: identity.value().clone(),
                },
            ))
        })
        .collect())
}

/// Every folder's readable entity, collision losers included, in id order
/// and, within an id, the entity the identity map resolves it to first: for a
/// caller that must account for each folder, such as an import reading
/// another journal. A lookup by id wants `load_all_journal_entities`.
pub fn every_journal_entity(journal_root: &Path) -> Result<Vec<JournalEntity>, EntityStoreError> {
    let (groups, _) = super::map::identity_groups(journal_root)?;
    Ok(groups
        .into_iter()
        .flat_map(|(id, members)| {
            members.into_iter().map(move |(_, identity)| JournalEntity {
                id: id.clone(),
                value: identity.value().clone(),
            })
        })
        .collect())
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
