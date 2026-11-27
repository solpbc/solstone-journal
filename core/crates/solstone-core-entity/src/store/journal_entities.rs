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

    /// Return whether the durable identity is the journal principal: a
    /// `Person` marked `is_principal: true` (see `identity_is_principal`).
    pub fn is_principal(&self) -> bool {
        super::lifecycle::identity_is_principal(&self.value)
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
///
/// A Person named with a placeholder such as "Speaker 2" stands for someone a
/// moment never named, so it never identifies a voice.
pub fn is_admissible_person(entity: &JournalEntity) -> bool {
    entity.entity_type() == Some("Person")
        && !entity.is_blocked()
        && !entity
            .value
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(is_placeholder_name)
}

/// Return whether an entity name is a placeholder label, not a name.
pub fn is_placeholder_name(name: &str) -> bool {
    super::review_policy::is_placeholder_query(
        &solstone_core_entity_matching::normalize_resolution_query(name),
    )
}

/// The journal's entities, one per effective id, in id order: the entity the
/// identity map resolves each id to. Of two folders claiming one id, only that
/// one is listed; the map reports the other. A missing, damaged or unreadable
/// identity holds no entity. A failure listing `entities/` is returned.
pub fn load_all_journal_entities(
    journal_root: &Path,
) -> Result<Vec<JournalEntity>, EntityStoreError> {
    load_all_journal_entities_in(journal_root, solstone_core_cli_copy::ENGLISH)
}

/// Enumerate the same identities, reporting unreadable folders in the calling
/// command's language.
pub fn load_all_journal_entities_in(
    journal_root: &Path,
    locale: &str,
) -> Result<Vec<JournalEntity>, EntityStoreError> {
    Ok(live_journal_entities_in(journal_root, locale)?
        .into_iter()
        .map(|(_, entity)| entity)
        .collect())
}

/// Like `load_all_journal_entities`, with the folder that holds each entity.
pub fn live_journal_entities(
    journal_root: &Path,
) -> Result<Vec<(String, JournalEntity)>, EntityStoreError> {
    live_journal_entities_in(journal_root, solstone_core_cli_copy::ENGLISH)
}

fn live_journal_entities_in(
    journal_root: &Path,
    locale: &str,
) -> Result<Vec<(String, JournalEntity)>, EntityStoreError> {
    let (groups, losers) = super::map::identity_groups_in(journal_root, locale)?;
    for loser in losers {
        if let super::map::IdentityMapLoserReason::Malformed { message } = loser.reason {
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.share.provider.entity.invalid",
                    [("entity", &loser.entity_dir), ("detail", &message)],
                )
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
    let (entities, _) = every_journal_entity_with_resolved_identities(journal_root)?;
    Ok(entities)
}

/// Read every journal entity once, returning both the flat entity list and the
/// resolved `entity_id -> (winner_entity_dir, identity_json)` map so callers
/// avoid re-scanning `entities/*/identity.json` per facet.
pub fn every_journal_entity_with_resolved_identities(
    journal_root: &Path,
) -> Result<
    (
        Vec<JournalEntity>,
        std::collections::HashMap<String, (String, Value)>,
    ),
    EntityStoreError,
> {
    let (groups, _) = super::map::identity_groups(journal_root)?;
    let mut resolved = std::collections::HashMap::with_capacity(groups.len());
    let mut entities = Vec::with_capacity(groups.len());
    for (id, members) in groups {
        if let Some((winner_dir, winner_identity)) = members.first() {
            resolved.insert(
                id.clone(),
                (winner_dir.clone(), winner_identity.value().clone()),
            );
        }
        for (_, identity) in members {
            entities.push(JournalEntity {
                id: id.clone(),
                value: identity.value().clone(),
            });
        }
    }
    Ok((entities, resolved))
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
        assert!(is_admissible_person(&entity(
            json!({"type":"Person","name":"Ada Lovelace"})
        )));
        for name in ["Speaker 3", "speaker 12", "Speaker A", " Unknown Speaker "] {
            assert!(
                !is_admissible_person(&entity(json!({"type":"Person","name":name}))),
                "{name}"
            );
        }
    }
}
