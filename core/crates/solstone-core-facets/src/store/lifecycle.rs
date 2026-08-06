// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-facing lifecycle operations that span facet and entity state.
//!
//! Deletion uses the proceed-and-report shape: it removes facet links and ambiguity
//! references, then removes the entity directory, while returning fixed per-surface
//! counts gathered before mutation. Remaining observation, activity, chronicle,
//! cross-reference, speaker, and review references deliberately remain dangling.
//! The scanner suite records each named surface (and one unreadable JSONL record):
//! unrecognized file, facet link, observation, activity, label, correction, AKA
//! cross-reference, speaker candidate, keep-separate assertion, identify operation,
//! ambiguity, each review surface, candidate pair, and dismissal.
//! Candidate-pair and dismissal rows normally retain pre-resolution audio-cluster
//! coordinates rather than journal entity ids, so those counts are rarely nonzero.
//! Identify-operation and segment-correction counts are intentionally conservative:
//! restored speaker operations are not modeled here, so their matching rows remain
//! visible in the advisory report rather than being riskily excluded.
//! Block can leave a blocked entity with only some links detached if interrupted;
//! rerunning it completes idempotently (`block_detaches_links_by_stored_entity_id_and_reports_only_new_detaches`).
//! Delete removes links and repairs ambiguities before the entity directory, so a
//! partial delete remains distinguishable by its surviving entity directory. Both
//! operations take facet trust before entity trust, matching `rename_facet` and
//! preventing inversion. Delete leaves no durable trace beyond removed entity history
//! and the deliberately dangling references.

use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_entity::{
    EntityLifecycleError, EntityOperationContext, EntityOperationKind, EntityStoreError,
    EntityTrustLockError, EntityWriteError, delete_entity_directory, read_entity_identity,
    read_identity_map, remove_entity_ambiguity_references, save_entity_identity,
};
use solstone_core_journal_io::{DirEntryKind, list_dir_entries};

use crate::{FacetTrustLockError, hold_facet_trust_lock};

use super::error::{FacetStoreError, FacetWriteError};
use super::identity::read_facet_entity_link;
use super::map::list_facet_entity_directories;
use super::paths::facets_dir;
use super::reference_scan::{EntityReferenceBreakdown, scan_entity_references};
use super::write::{delete_facet_entity_link, set_facet_entity_link_detached};

/// Facets newly detached while blocking one journal entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityBlockReport {
    pub facets_detached: Vec<String>,
}

/// Facet links removed and references observed during an entity deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDeleteReport {
    pub facets_deleted: Vec<String>,
    pub references: EntityReferenceBreakdown,
}

/// Failure while blocking a journal entity and detaching its facet relationships.
#[derive(Debug)]
pub enum FacetEntityLifecycleError {
    FacetTrustLock(FacetTrustLockError),
    EntityTrustLock(EntityTrustLockError),
    Entity(EntityLifecycleError),
    FacetStore(FacetStoreError),
    FacetWrite(FacetWriteError),
    PrincipalEntityProtected { entity_id: String },
}

impl fmt::Display for FacetEntityLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FacetTrustLock(error) => error.fmt(formatter),
            Self::EntityTrustLock(error) => error.fmt(formatter),
            Self::Entity(error) => error.fmt(formatter),
            Self::FacetStore(error) => error.fmt(formatter),
            Self::FacetWrite(error) => error.fmt(formatter),
            Self::PrincipalEntityProtected { entity_id } => {
                write!(
                    formatter,
                    "principal entity is protected from this operation: {entity_id}"
                )
            }
        }
    }
}

impl Error for FacetEntityLifecycleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::FacetTrustLock(error) => Some(error),
            Self::EntityTrustLock(error) => Some(error),
            Self::Entity(error) => Some(error),
            Self::FacetStore(error) => Some(error),
            Self::FacetWrite(error) => Some(error),
            Self::PrincipalEntityProtected { .. } => None,
        }
    }
}

impl From<FacetStoreError> for FacetEntityLifecycleError {
    fn from(error: FacetStoreError) -> Self {
        Self::FacetStore(error)
    }
}

impl From<FacetWriteError> for FacetEntityLifecycleError {
    fn from(error: FacetWriteError) -> Self {
        Self::FacetWrite(error)
    }
}

/// Block an entity and detach every facet relationship that names its effective id.
///
/// The outer facet guard is acquired before the entity guard, matching the only
/// established cross-store lock order. Both remain live until this function returns;
/// nested entity and facet writer guards merely increment their reentrant depths.
pub fn block_journal_entity(
    journal_root: &Path,
    entity_id: &str,
) -> Result<EntityBlockReport, FacetEntityLifecycleError> {
    let _facet_trust =
        hold_facet_trust_lock(journal_root).map_err(FacetEntityLifecycleError::FacetTrustLock)?;
    let _entity_trust = solstone_core_entity::hold_entity_trust_lock(journal_root)
        .map_err(FacetEntityLifecycleError::EntityTrustLock)?;

    let entity_dir = read_identity_map(journal_root)
        .map_err(entity_store_error)?
        .resolved
        .get(entity_id)
        .cloned()
        .ok_or_else(|| {
            FacetEntityLifecycleError::Entity(EntityLifecycleError::EntityNotFound {
                entity_id: entity_id.to_owned(),
            })
        })?;
    let identity = read_entity_identity(journal_root, &entity_dir)
        .map_err(entity_store_error)?
        .expect("resolved identity-map directory contains an identity");
    if identity
        .value()
        .get("is_principal")
        .is_some_and(value_is_truthy)
    {
        return Err(FacetEntityLifecycleError::PrincipalEntityProtected {
            entity_id: entity_id.to_owned(),
        });
    }

    let mut blocked_identity = identity.value().clone();
    let object = blocked_identity
        .as_object_mut()
        .expect("identity reader returns an object");
    object.insert("blocked".to_owned(), Value::Bool(true));
    object.insert("updated_at".to_owned(), Value::String(now_iso()));
    let operation = EntityOperationContext {
        kind: EntityOperationKind::Update,
        caller: Value::Null,
        actor: Value::Null,
        metadata: json!({}),
    };
    save_entity_identity(journal_root, entity_id, &blocked_identity, Some(&operation))
        .map_err(entity_write_error)?;

    let mut facets_detached = Vec::new();
    for entry in list_dir_entries(&facets_dir(journal_root)?)
        .map_err(FacetStoreError::from)?
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::Directory)
    {
        let facet_dir = entry.name.to_string_lossy().into_owned();
        let mut detached_here = false;
        for relationship_dir in list_facet_entity_directories(journal_root, &facet_dir)? {
            let Some(link) = read_facet_entity_link(journal_root, &facet_dir, &relationship_dir)?
            else {
                continue;
            };
            if link.entity_id() == entity_id
                && set_facet_entity_link_detached(
                    journal_root,
                    &facet_dir,
                    &relationship_dir,
                    true,
                )?
            {
                detached_here = true;
            }
        }
        if detached_here {
            facets_detached.push(facet_dir);
        }
    }
    Ok(EntityBlockReport { facets_detached })
}

/// Delete an entity after surveying references, removing links and ambiguities first.
pub fn delete_journal_entity(
    journal_root: &Path,
    entity_id: &str,
) -> Result<EntityDeleteReport, FacetEntityLifecycleError> {
    let _facet_trust =
        hold_facet_trust_lock(journal_root).map_err(FacetEntityLifecycleError::FacetTrustLock)?;
    let _entity_trust = solstone_core_entity::hold_entity_trust_lock(journal_root)
        .map_err(FacetEntityLifecycleError::EntityTrustLock)?;
    let entity_dir = read_identity_map(journal_root)
        .map_err(entity_store_error)?
        .resolved
        .get(entity_id)
        .cloned()
        .ok_or_else(|| {
            FacetEntityLifecycleError::Entity(EntityLifecycleError::EntityNotFound {
                entity_id: entity_id.to_owned(),
            })
        })?;
    let identity = read_entity_identity(journal_root, &entity_dir)
        .map_err(entity_store_error)?
        .expect("resolved identity-map directory contains an identity");
    if identity
        .value()
        .get("is_principal")
        .is_some_and(value_is_truthy)
    {
        return Err(FacetEntityLifecycleError::PrincipalEntityProtected {
            entity_id: entity_id.to_owned(),
        });
    }
    let references = scan_entity_references(journal_root, entity_id, &entity_dir)?;
    let mut facets_deleted = Vec::new();
    for entry in list_dir_entries(&facets_dir(journal_root)?)
        .map_err(FacetStoreError::from)?
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::Directory)
    {
        let facet_dir = entry.name.to_string_lossy().into_owned();
        let mut deleted_here = false;
        for relationship_dir in list_facet_entity_directories(journal_root, &facet_dir)? {
            let Some(link) = read_facet_entity_link(journal_root, &facet_dir, &relationship_dir)?
            else {
                continue;
            };
            if link.entity_id() == entity_id
                && delete_facet_entity_link(journal_root, &facet_dir, &relationship_dir)?
            {
                deleted_here = true;
            }
        }
        if deleted_here {
            facets_deleted.push(facet_dir);
        }
    }
    remove_entity_ambiguity_references(journal_root, entity_id).map_err(entity_write_error)?;
    delete_entity_directory(journal_root, entity_id).map_err(FacetEntityLifecycleError::Entity)?;
    Ok(EntityDeleteReport {
        facets_deleted,
        references,
    })
}

fn entity_store_error(error: EntityStoreError) -> FacetEntityLifecycleError {
    FacetEntityLifecycleError::Entity(EntityLifecycleError::Store(error))
}

fn entity_write_error(error: EntityWriteError) -> FacetEntityLifecycleError {
    FacetEntityLifecycleError::Entity(EntityLifecycleError::Write(error))
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
