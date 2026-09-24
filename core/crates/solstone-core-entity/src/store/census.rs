// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::durability::{ArtifactId, DurableObservation, observe_json_durable};
use solstone_core_journal_io::{DirEntryKind, contained_path, list_dir_entries};

use super::error::EntityStoreError;
use super::lifecycle::value_is_truthy;
use super::paths::identity_path;

/// One proved entity identity discovered during a census scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CensusEntity {
    pub id: String,
    pub dir_name: String,
    pub entity_type: Option<String>,
    pub blocked: bool,
    pub is_principal: bool,
    pub name: String,
    pub aka: Vec<String>,
    pub emails: Vec<String>,
}

/// Result of an authoritative identity census across all entity directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityCensus {
    pub entities: HashMap<String, CensusEntity>,
    pub complete: bool,
}

impl IdentityCensus {
    pub fn is_proved_present(&self, id: &str) -> bool {
        self.entities.contains_key(id)
    }

    pub fn is_proved_absent(&self, id: &str) -> bool {
        self.complete && !self.entities.contains_key(id)
    }

    pub fn is_eligible_person(&self, id: &str) -> bool {
        self.entities.get(id).is_some_and(|entity| {
            entity.entity_type.as_deref() == Some("Person") && !entity.blocked
        })
    }

    pub fn get_entity(&self, id: &str) -> Option<&CensusEntity> {
        self.entities.get(id)
    }
}

/// Run an authoritative identity census using durable observations directly.
pub fn scan_identity_census(journal_root: &Path) -> Result<IdentityCensus, EntityStoreError> {
    let entities_dir = contained_path(journal_root, "entities")?;

    let entries = match list_dir_entries(&entities_dir) {
        Ok(entries) => entries,
        Err(solstone_core_journal_io::PathError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(IdentityCensus {
                entities: HashMap::new(),
                complete: true,
            });
        }
        Err(err) => return Err(err.into()),
    };

    let mut entities = HashMap::new();
    let mut complete = true;

    for entry in entries {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let dir_name = entry.name.to_string_lossy().into_owned();
        let Ok(path) = identity_path(journal_root, &dir_name) else {
            complete = false;
            continue;
        };

        match observe_json_durable::<Value>(ArtifactId::Entity, &path) {
            DurableObservation::Absent => {
                // Not an identity directory, not a failure.
            }
            DurableObservation::Malformed { .. } | DurableObservation::Unreadable { .. } => {
                complete = false;
            }
            DurableObservation::Present(value) => {
                let Some(object) = value.as_object() else {
                    complete = false;
                    continue;
                };
                let id = object
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| dir_name.clone());
                let entity_type = object
                    .get("type")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned);
                let blocked = object.get("blocked").is_some_and(value_is_truthy);
                let is_principal = object.get("is_principal").is_some_and(value_is_truthy);
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let aka = object
                    .get("aka")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let emails = object
                    .get("emails")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();

                entities.insert(
                    id.clone(),
                    CensusEntity {
                        id,
                        dir_name,
                        entity_type,
                        blocked,
                        is_principal,
                        name,
                        aka,
                        emails,
                    },
                );
            }
        }
    }

    Ok(IdentityCensus { entities, complete })
}
