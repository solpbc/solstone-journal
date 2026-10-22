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
//! operations take facet trust before entity trust, matching `delete_facet` and
//! preventing inversion. Delete leaves no durable trace beyond removed entity history
//! and the deliberately dangling references.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_entity::{
    EntityLifecycleError, EntityOperationContext, EntityOperationKind, EntityStoreError,
    EntityTrustLockError, EntityWriteError, delete_entity_directory, read_entity_identity,
    read_identity_map, read_visible_history, remove_entity_ambiguity_references,
    save_entity_identity,
};
use solstone_core_journal_io::{DirEntryKind, contained_path, list_dir_entries, path_lexists};

use crate::{FacetTrustLockError, hold_facet_trust_lock};

use super::error::{FacetStoreError, FacetWriteError};
use super::identity::read_facet_entity_link;
use super::map::list_facet_entity_directories;
use super::paths::facets_dir;
use super::reference_scan::{EntityReferenceBreakdown, scan_entity_references};
use super::write::{
    delete_facet_entity_link, set_facet_entity_link_detached, strip_facet_entity_link_detached_by,
};

pub(crate) const HIDE_DETACHED_BY: &str = "hide";

/// Facets newly detached while blocking one journal entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityBlockReport {
    pub facets_detached: Vec<String>,
}

/// Facets re-attached and unreadable relationship paths skipped while unblocking one journal entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityUnblockReport {
    pub event: Value,
    pub facets_reattached: Vec<String>,
    pub relationships_unreadable: Vec<String>,
}

/// Facet links removed and references observed during an entity deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDeleteReport {
    pub facets_deleted: Vec<String>,
    pub references: EntityReferenceBreakdown,
}

/// One expected visible identity-history reference for guarded created-entity undo.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntityHistoryReference {
    pub version_id: String,
    pub sequence: i128,
}

/// Result of the idempotent compare-and-swap deletion for an identify-created entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDeleteGuardOutcome {
    pub deleted: bool,
    pub already_gone: bool,
    pub identity_changed: bool,
    pub history_changed: bool,
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
    PrincipalEntityProtected {
        entity_id: String,
    },
    RelationshipsUnreadable {
        path: String,
    },
    /// The step the caller runs before anything is removed refused; nothing
    /// was removed.
    BeforeRemoval(String),
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
            Self::RelationshipsUnreadable { path } => formatter.write_str(path),
            Self::BeforeRemoval(detail) => formatter.write_str(detail),
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
            Self::PrincipalEntityProtected { .. }
            | Self::RelationshipsUnreadable { .. }
            | Self::BeforeRemoval(_) => None,
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
    block_journal_entity_inner(journal_root, entity_id, || {})
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn block_journal_entity_with_hook(
    journal_root: &Path,
    entity_id: &str,
    hook: impl FnOnce(),
) -> Result<EntityBlockReport, FacetEntityLifecycleError> {
    block_journal_entity_inner(journal_root, entity_id, hook)
}

fn block_journal_entity_inner(
    journal_root: &Path,
    entity_id: &str,
    hook: impl FnOnce(),
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
    // A flag at rest that names no one still protects the entity from being
    // blocked or deleted.
    if identity
        .value()
        .get("is_principal")
        .is_some_and(value_is_truthy)
        || identity.had_stray_principal_flag()
    {
        return Err(FacetEntityLifecycleError::PrincipalEntityProtected {
            entity_id: entity_id.to_owned(),
        });
    }

    struct PlannedRelationship {
        facet_dir: String,
        relationship_dir: String,
        is_detached: bool,
    }

    let mut planned = Vec::new();
    let facet_entries =
        list_dir_entries(&facets_dir(journal_root)?).map_err(FacetStoreError::from)?;
    for entry in facet_entries
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::Directory)
    {
        let facet_dir = entry.name.to_string_lossy().into_owned();
        let entities_rel = format!("facets/{facet_dir}/entities");
        let entities_path = match contained_path(journal_root, &entities_rel) {
            Ok(p) => p,
            Err(_) => {
                return Err(FacetEntityLifecycleError::RelationshipsUnreadable {
                    path: entities_rel,
                });
            }
        };
        let entities_exists = match path_lexists(&entities_path) {
            Ok(exists) => exists,
            Err(_) => {
                return Err(FacetEntityLifecycleError::RelationshipsUnreadable {
                    path: entities_rel,
                });
            }
        };
        if !entities_exists {
            continue;
        }
        let rel_entries = match list_dir_entries(&entities_path) {
            Ok(entries) => entries,
            Err(_) => {
                return Err(FacetEntityLifecycleError::RelationshipsUnreadable {
                    path: entities_rel,
                });
            }
        };
        for rel_entry in rel_entries
            .into_iter()
            .filter(|e| e.kind == DirEntryKind::Directory)
        {
            let relationship_dir = rel_entry.name.to_string_lossy().into_owned();
            let link_rel = format!("facets/{facet_dir}/entities/{relationship_dir}/entity.json");
            let link_path = match contained_path(journal_root, &link_rel) {
                Ok(p) => p,
                Err(_) => {
                    return Err(FacetEntityLifecycleError::RelationshipsUnreadable {
                        path: link_rel,
                    });
                }
            };
            let link_exists = match path_lexists(&link_path) {
                Ok(exists) => exists,
                Err(_) => {
                    return Err(FacetEntityLifecycleError::RelationshipsUnreadable {
                        path: link_rel,
                    });
                }
            };
            if !link_exists {
                continue;
            }
            let link = match read_facet_entity_link(journal_root, &facet_dir, &relationship_dir) {
                Ok(Some(link)) => link,
                Ok(None) => continue,
                Err(_) => {
                    return Err(FacetEntityLifecycleError::RelationshipsUnreadable {
                        path: link_rel,
                    });
                }
            };
            if link.entity_id() == entity_id {
                let is_detached = link.value().get("detached") == Some(&Value::Bool(true));
                planned.push(PlannedRelationship {
                    facet_dir: facet_dir.clone(),
                    relationship_dir,
                    is_detached,
                });
            }
        }
    }

    let was_hidden = solstone_core_entity::entity_is_hidden(identity.value());
    if !was_hidden {
        for rel in &planned {
            if rel.is_detached {
                strip_facet_entity_link_detached_by(
                    journal_root,
                    &rel.facet_dir,
                    &rel.relationship_dir,
                )?;
            }
        }
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
    hook();

    let mut facets_detached = Vec::new();
    for rel in &planned {
        if !rel.is_detached
            && set_facet_entity_link_detached(
                journal_root,
                &rel.facet_dir,
                &rel.relationship_dir,
                true,
                Some(HIDE_DETACHED_BY),
            )?
            && !facets_detached.contains(&rel.facet_dir)
        {
            facets_detached.push(rel.facet_dir.clone());
        }
    }
    Ok(EntityBlockReport { facets_detached })
}

/// Unblock an entity and re-attach relationships marked as detached by hide.
pub fn unblock_journal_entity(
    journal_root: &Path,
    entity_id: &str,
) -> Result<EntityUnblockReport, FacetEntityLifecycleError> {
    unblock_journal_entity_inner(journal_root, entity_id, || {})
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn unblock_journal_entity_with_hook(
    journal_root: &Path,
    entity_id: &str,
    hook: impl FnOnce(),
) -> Result<EntityUnblockReport, FacetEntityLifecycleError> {
    unblock_journal_entity_inner(journal_root, entity_id, hook)
}

fn unblock_journal_entity_inner(
    journal_root: &Path,
    entity_id: &str,
    hook: impl FnOnce(),
) -> Result<EntityUnblockReport, FacetEntityLifecycleError> {
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
    if !solstone_core_entity::entity_is_hidden(identity.value()) {
        return Err(FacetEntityLifecycleError::Entity(
            EntityLifecycleError::EntityNotBlocked {
                entity_id: entity_id.to_owned(),
            },
        ));
    }

    let mut facets_reattached = Vec::new();
    let mut relationships_unreadable = Vec::new();
    let mut to_reattach = Vec::new();

    let facet_entries =
        list_dir_entries(&facets_dir(journal_root)?).map_err(FacetStoreError::from)?;
    for entry in facet_entries
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::Directory)
    {
        let facet_dir = entry.name.to_string_lossy().into_owned();
        let entities_rel = format!("facets/{facet_dir}/entities");
        let entities_path = match contained_path(journal_root, &entities_rel) {
            Ok(p) => p,
            Err(_) => {
                relationships_unreadable.push(entities_rel);
                continue;
            }
        };
        let entities_exists = match path_lexists(&entities_path) {
            Ok(exists) => exists,
            Err(_) => {
                relationships_unreadable.push(entities_rel);
                continue;
            }
        };
        if !entities_exists {
            continue;
        }
        let rel_entries = match list_dir_entries(&entities_path) {
            Ok(entries) => entries,
            Err(_) => {
                relationships_unreadable.push(entities_rel);
                continue;
            }
        };
        for rel_entry in rel_entries
            .into_iter()
            .filter(|e| e.kind == DirEntryKind::Directory)
        {
            let relationship_dir = rel_entry.name.to_string_lossy().into_owned();
            let link_rel = format!("facets/{facet_dir}/entities/{relationship_dir}/entity.json");
            let link_path = match contained_path(journal_root, &link_rel) {
                Ok(p) => p,
                Err(_) => {
                    relationships_unreadable.push(link_rel);
                    continue;
                }
            };
            let link_exists = match path_lexists(&link_path) {
                Ok(exists) => exists,
                Err(_) => {
                    relationships_unreadable.push(link_rel);
                    continue;
                }
            };
            if !link_exists {
                continue;
            }
            let link = match read_facet_entity_link(journal_root, &facet_dir, &relationship_dir) {
                Ok(Some(link)) => link,
                Ok(None) => continue,
                Err(_) => {
                    relationships_unreadable.push(link_rel);
                    continue;
                }
            };
            if link.entity_id() == entity_id
                && link.value().get("detached_by").and_then(Value::as_str) == Some(HIDE_DETACHED_BY)
            {
                to_reattach.push((facet_dir.clone(), relationship_dir));
            }
        }
    }

    for (facet_dir, relationship_dir) in to_reattach {
        if set_facet_entity_link_detached(journal_root, &facet_dir, &relationship_dir, false, None)?
            && !facets_reattached.contains(&facet_dir)
        {
            facets_reattached.push(facet_dir);
        }
    }

    hook();

    let event = solstone_core_entity::unblock_journal_entity(journal_root, entity_id)
        .map_err(FacetEntityLifecycleError::Entity)?;

    Ok(EntityUnblockReport {
        event,
        facets_reattached,
        relationships_unreadable,
    })
}

/// Delete an entity after surveying references, removing links and ambiguities first.
pub fn delete_journal_entity(
    journal_root: &Path,
    entity_id: &str,
) -> Result<EntityDeleteReport, FacetEntityLifecycleError> {
    delete_journal_entity_inner(journal_root, entity_id, |_, _| Ok(()), || {})
}

/// The owner's delete: as [`delete_journal_entity`], and after its own
/// refusals (not found, the principal) and before anything is removed, it
/// runs `before_removal` with the entity's directory and identity. The
/// owner delete records the deleted id there; if that refuses, nothing is
/// removed.
pub fn delete_journal_entity_after(
    journal_root: &Path,
    entity_id: &str,
    before_removal: impl FnOnce(&str, &Value) -> Result<(), String>,
) -> Result<EntityDeleteReport, FacetEntityLifecycleError> {
    delete_journal_entity_inner(journal_root, entity_id, before_removal, || {})
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn delete_journal_entity_with_hook(
    journal_root: &Path,
    entity_id: &str,
    hook: impl FnOnce(),
) -> Result<EntityDeleteReport, FacetEntityLifecycleError> {
    delete_journal_entity_inner(journal_root, entity_id, |_, _| Ok(()), hook)
}

fn delete_journal_entity_inner(
    journal_root: &Path,
    entity_id: &str,
    before_removal: impl FnOnce(&str, &Value) -> Result<(), String>,
    hook: impl FnOnce(),
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
    // A flag at rest that names no one still protects the entity from being
    // blocked or deleted.
    if identity
        .value()
        .get("is_principal")
        .is_some_and(value_is_truthy)
        || identity.had_stray_principal_flag()
    {
        return Err(FacetEntityLifecycleError::PrincipalEntityProtected {
            entity_id: entity_id.to_owned(),
        });
    }
    before_removal(&entity_dir, identity.value())
        .map_err(FacetEntityLifecycleError::BeforeRemoval)?;
    let references = scan_entity_references(journal_root, entity_id, &entity_dir, None)?;
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
    hook();
    remove_entity_ambiguity_references(journal_root, entity_id).map_err(entity_write_error)?;
    delete_entity_directory(journal_root, entity_id).map_err(FacetEntityLifecycleError::Entity)?;
    Ok(EntityDeleteReport {
        facets_deleted,
        references,
    })
}

/// Delete an identify-created entity only when its original identity and history remain intact.
///
/// This lives beside [`delete_journal_entity`] because the guarded operation needs facet-store
/// reference access. Unlike the owner-facing delete, a missing entity is a successful,
/// idempotent-undo outcome; the owner-facing operation still errors for a missing entity. The
/// operation holds facet trust before entity trust for its full duration, matching the established
/// cross-store order and preventing inversion. Once it elects to delete, it delegates to
/// [`delete_journal_entity`], inheriting that operation's existing proceed-and-report
/// partial-completion behavior rather than adding new interruption or resume semantics.
pub fn delete_created_entity_if_unreferenced(
    journal_root: &Path,
    entity_id: &str,
    operation_id: &str,
    expected_identity: &Value,
    expected_history_refs: &[EntityHistoryReference],
) -> Result<EntityDeleteGuardOutcome, FacetEntityLifecycleError> {
    delete_created_entity_if_unreferenced_inner(
        journal_root,
        entity_id,
        operation_id,
        expected_identity,
        expected_history_refs,
        || {},
    )
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn delete_created_entity_if_unreferenced_with_hook(
    journal_root: &Path,
    entity_id: &str,
    operation_id: &str,
    expected_identity: &Value,
    expected_history_refs: &[EntityHistoryReference],
    hook: impl FnOnce(),
) -> Result<EntityDeleteGuardOutcome, FacetEntityLifecycleError> {
    delete_created_entity_if_unreferenced_inner(
        journal_root,
        entity_id,
        operation_id,
        expected_identity,
        expected_history_refs,
        hook,
    )
}

fn delete_created_entity_if_unreferenced_inner(
    journal_root: &Path,
    entity_id: &str,
    operation_id: &str,
    expected_identity: &Value,
    expected_history_refs: &[EntityHistoryReference],
    hook: impl FnOnce(),
) -> Result<EntityDeleteGuardOutcome, FacetEntityLifecycleError> {
    let _facet_trust =
        hold_facet_trust_lock(journal_root).map_err(FacetEntityLifecycleError::FacetTrustLock)?;
    let _entity_trust = solstone_core_entity::hold_entity_trust_lock(journal_root)
        .map_err(FacetEntityLifecycleError::EntityTrustLock)?;
    let Some(entity_dir) = read_identity_map(journal_root)
        .map_err(entity_store_error)?
        .resolved
        .get(entity_id)
        .cloned()
    else {
        return Ok(EntityDeleteGuardOutcome {
            deleted: true,
            already_gone: true,
            identity_changed: false,
            history_changed: false,
            references: EntityReferenceBreakdown::default(),
        });
    };

    let current = read_entity_identity(journal_root, &entity_dir)
        .map_err(entity_store_error)?
        .expect("resolved identity-map directory contains an identity");
    let identity_changed = !meaningful_identity_matches(current.value(), expected_identity);

    let mut history_unreadable = false;
    let history_changed = match read_visible_history(journal_root, &entity_dir) {
        Ok(events) => history_changed(&events, expected_history_refs, operation_id)
            .map_err(entity_store_error)?,
        Err(_) => {
            history_unreadable = true;
            false
        }
    };
    let mut references =
        scan_entity_references(journal_root, entity_id, &entity_dir, Some(operation_id))?;
    if history_unreadable {
        references.unreadable += 1;
    }

    if !identity_changed && !history_changed && references == EntityReferenceBreakdown::default() {
        hook();
        delete_journal_entity(journal_root, entity_id)?;
        return Ok(EntityDeleteGuardOutcome {
            deleted: true,
            already_gone: false,
            identity_changed: false,
            history_changed: false,
            references: EntityReferenceBreakdown::default(),
        });
    }

    Ok(EntityDeleteGuardOutcome {
        deleted: false,
        already_gone: false,
        identity_changed,
        history_changed,
        references,
    })
}

fn meaningful_identity_matches(current: &Value, expected: &Value) -> bool {
    [
        "id",
        "name",
        "type",
        "aka",
        "emails",
        "is_principal",
        "blocked",
    ]
    .iter()
    .all(|field| current.get(*field) == expected.get(*field))
}

fn history_changed(
    events: &[solstone_core_entity::HistoryEvent],
    expected_history_refs: &[EntityHistoryReference],
    operation_id: &str,
) -> Result<bool, EntityStoreError> {
    let current_refs = events
        .iter()
        .map(|event| Ok((event.version_id()?.to_owned(), event.sequence()?)))
        .collect::<Result<HashSet<_>, EntityStoreError>>()?;
    let expected_refs = expected_history_refs
        .iter()
        .map(|reference| (reference.version_id.clone(), reference.sequence))
        .collect::<HashSet<_>>();
    let Some(event) = events.first() else {
        return Ok(true);
    };
    Ok(events.len() != 1
        || current_refs != expected_refs
        || !event.value().get("operation").is_some_and(Value::is_object)
        || event
            .value()
            .get("operation")
            .and_then(|operation| operation.get("operation_kind"))
            .and_then(Value::as_str)
            != Some("speaker_identify")
        || event
            .value()
            .get("operation")
            .and_then(|operation| operation.get("operation_id"))
            .and_then(Value::as_str)
            != Some(operation_id))
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
