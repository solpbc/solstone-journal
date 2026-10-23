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

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_entity::{
    EntityLifecycleError, EntityOperationContext, EntityOperationKind, EntityStoreError,
    EntityTrustLockError, EntityWriteError, HistoryEvent, IdentitySnapshot,
    delete_entity_directory, read_entity_identity, read_identity_map, read_visible_history,
    reconcile_prepared_history_reporting, remove_entity_ambiguity_references, save_entity_identity,
    save_entity_identity_with_map, write_resolved_identity_map_cache,
};
use solstone_core_journal_io::{DirEntryKind, contained_path, list_dir_entries, path_lexists};

use crate::{FacetTrustLockError, hold_facet_trust_lock};

use super::error::{FacetStoreError, FacetWriteError};
use super::identity::read_facet_entity_link;
use super::map::list_facet_entity_directories;
use super::paths::facets_dir;
use super::reference_scan::{EntityReferenceBreakdown, scan_entity_references};
use super::write::{
    delete_facet_entity_link, mutate_detach_link, mutate_reattach_link, mutate_strip_detached_by,
    set_facet_entity_link_detached, strip_facet_entity_link_detached_by,
    write_facet_entity_link_raw,
};

pub(crate) const HIDE_DETACHED_BY: &str = "hide";

static UNDONE_OPERATIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn undone_operations_set() -> &'static Mutex<HashSet<String>> {
    UNDONE_OPERATIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_operation_undone(operation: &str) -> bool {
    let guard = undone_operations_set()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.contains(operation)
}

fn mark_operation_undone(operation: &str) {
    let mut guard = undone_operations_set()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(operation.to_owned());
}

#[cfg(any(test, feature = "test-hooks"))]
pub fn reset_undone_operation_for_test(operation: &str) {
    let mut guard = undone_operations_set()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.remove(operation);
}

#[cfg(any(test, feature = "test-hooks"))]
pub mod test_hooks {
    use std::cell::{Cell, RefCell};

    thread_local! {
        static FAIL_BATCH_RELATIONSHIP_ENTITY: RefCell<Option<String>> = const { RefCell::new(None) };
        static FAIL_UNHIDE_SAVE_ENTITY: RefCell<Option<String>> = const { RefCell::new(None) };
        static FAIL_UNDO_AFTER_N_RESTORES: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub struct FailBatchRelationshipGuard;
    impl Drop for FailBatchRelationshipGuard {
        fn drop(&mut self) {
            FAIL_BATCH_RELATIONSHIP_ENTITY.with(|c| *c.borrow_mut() = None);
        }
    }
    pub fn arm_fail_batch_relationship(entity_id: &str) -> FailBatchRelationshipGuard {
        FAIL_BATCH_RELATIONSHIP_ENTITY.with(|c| *c.borrow_mut() = Some(entity_id.to_owned()));
        FailBatchRelationshipGuard
    }
    pub(crate) fn check_fail_batch_relationship(entity_id: &str) -> bool {
        FAIL_BATCH_RELATIONSHIP_ENTITY.with(|c| c.borrow().as_deref() == Some(entity_id))
    }

    pub struct FailUnhideSaveGuard;
    impl Drop for FailUnhideSaveGuard {
        fn drop(&mut self) {
            FAIL_UNHIDE_SAVE_ENTITY.with(|c| *c.borrow_mut() = None);
        }
    }
    pub fn arm_fail_unhide_save(entity_id: &str) -> FailUnhideSaveGuard {
        FAIL_UNHIDE_SAVE_ENTITY.with(|c| *c.borrow_mut() = Some(entity_id.to_owned()));
        FailUnhideSaveGuard
    }
    pub(crate) fn check_fail_unhide_save(entity_id: &str) -> bool {
        FAIL_UNHIDE_SAVE_ENTITY.with(|c| c.borrow().as_deref() == Some(entity_id))
    }

    pub struct FailUndoAfterNRestoresGuard;
    impl Drop for FailUndoAfterNRestoresGuard {
        fn drop(&mut self) {
            FAIL_UNDO_AFTER_N_RESTORES.with(|c| c.set(None));
        }
    }
    pub fn arm_fail_undo_after_n_restores(n: usize) -> FailUndoAfterNRestoresGuard {
        FAIL_UNDO_AFTER_N_RESTORES.with(|c| c.set(Some(n)));
        FailUndoAfterNRestoresGuard
    }
    pub(crate) fn check_fail_undo_after_n_restores(current_success_count: usize) -> bool {
        FAIL_UNDO_AFTER_N_RESTORES.with(|c| c.get() == Some(current_success_count))
    }
}

pub(crate) fn is_valid_operation_id(operation: &str) -> bool {
    (8..=64).contains(&operation.len())
        && operation
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn validate_events_and_find_hide_event<'a>(
    events: &'a [HistoryEvent],
) -> Result<Option<&'a HistoryEvent>, ()> {
    let mut latest_hide: Option<(i128, &'a HistoryEvent)> = None;
    for event in events {
        let seq = match event.sequence() {
            Ok(s) => s,
            Err(_) => return Err(()),
        };
        let seq_val = event.value().get("seq");
        if !seq_val.is_some_and(|s| s.is_i64() || s.is_u64()) {
            return Err(());
        }

        let before_is_hidden = event
            .value()
            .get("identity_before")
            .is_some_and(|b| !b.is_null() && solstone_core_entity::entity_is_hidden(b));
        let after_is_hidden = event
            .value()
            .get("identity_after")
            .is_some_and(solstone_core_entity::entity_is_hidden);

        if after_is_hidden
            && !before_is_hidden
            && latest_hide
                .as_ref()
                .is_none_or(|(prev_seq, _)| seq > *prev_seq)
        {
            latest_hide = Some((seq, event));
        }
    }
    Ok(latest_hide.map(|(_, ev)| ev))
}

fn is_hide_event_for_operation(event: &HistoryEvent, operation: &str) -> bool {
    let op = event.value().get("operation");
    op.and_then(|op| op.get("bulk_operation"))
        .or_else(|| {
            op.and_then(|op| op.get("metadata"))
                .and_then(|m| m.get("bulk_operation"))
        })
        .and_then(Value::as_str)
        == Some(operation)
}

fn relative_history_error_path(
    journal_root: &Path,
    entity_dir: &str,
    error: Option<&EntityStoreError>,
) -> String {
    if let Some(
        EntityStoreError::Read(solstone_core_journal_io::ReadError::Io { path, .. })
        | EntityStoreError::Read(solstone_core_journal_io::ReadError::Malformed(
            solstone_core_journal_io::errors::MalformedDataError { path, .. },
        ))
        | EntityStoreError::HistoryEventNotObject { path },
    ) = error
    {
        // Reads resolve through the canonical journal root, so on macOS a root
        // under a symlink (/var -> /private/var) only matches once canonical.
        let canonical_root = std::fs::canonicalize(journal_root).ok();
        let relative = path.strip_prefix(journal_root).ok().or_else(|| {
            canonical_root
                .as_deref()
                .and_then(|root| path.strip_prefix(root).ok())
        });
        return relative.unwrap_or(path).to_string_lossy().into_owned();
    }
    format!("entities/{entity_dir}/history/events")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableEntityRecord {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoppedReport {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkBlockReport {
    pub hidden: Vec<String>,
    pub already_hidden: Vec<String>,
    pub missing: Vec<String>,
    pub unreadable: Vec<UnreadableEntityRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<StoppedReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredEntity {
    pub id: String,
    pub facets_reattached: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedEntity {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkUnblockReport {
    pub restored: Vec<RestoredEntity>,
    pub skipped: Vec<String>,
    pub failed: Vec<FailedEntity>,
    pub remaining: usize,
    pub unreadable: Vec<UnreadableEntityRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<StoppedReport>,
}

#[derive(Debug)]
pub enum BulkJournalError {
    InvalidOperationId(String),
    InvalidLimit(i64),
    InvalidIdsCount(usize),
    OperationUndone(String),
    PrincipalEntityProtected { entity_id: String },
    RelationshipsUnreadable { path: String },
    FacetTrustLock(FacetTrustLockError),
    EntityTrustLock(EntityTrustLockError),
    PreWrite(String),
}

impl fmt::Display for BulkJournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOperationId(id) => write!(formatter, "invalid operation id: {id}"),
            Self::InvalidLimit(limit) => write!(formatter, "invalid limit: {limit}"),
            Self::InvalidIdsCount(count) => write!(formatter, "invalid ids count: {count}"),
            Self::OperationUndone(op) => {
                write!(formatter, "operation {op} has already been undone")
            }
            Self::PrincipalEntityProtected { entity_id } => {
                write!(
                    formatter,
                    "principal entity is protected from this operation: {entity_id}"
                )
            }
            Self::RelationshipsUnreadable { path } => formatter.write_str(path),
            Self::FacetTrustLock(err) => err.fmt(formatter),
            Self::EntityTrustLock(err) => err.fmt(formatter),
            Self::PreWrite(detail) => formatter.write_str(detail),
        }
    }
}

impl Error for BulkJournalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::FacetTrustLock(err) => Some(err),
            Self::EntityTrustLock(err) => Some(err),
            _ => None,
        }
    }
}

impl BulkJournalError {
    #[must_use]
    pub fn is_lock_timeout(&self) -> bool {
        matches!(
            self,
            Self::FacetTrustLock(FacetTrustLockError::Lock(
                solstone_core_journal_io::LockError::Timeout(_)
            )) | Self::EntityTrustLock(EntityTrustLockError::Lock(
                solstone_core_journal_io::LockError::Timeout(_)
            ))
        )
    }
}

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

/// One facet relationship discovered during a journal-wide entity plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedFacetRelationship {
    pub entity_id: String,
    pub facet_dir: String,
    pub relationship_dir: String,
    pub is_detached: bool,
    pub detached_by: Option<String>,
    pub value: Value,
}

/// Discovered facet relationships and any damaged paths encountered during traversal.
#[derive(Debug, Clone, PartialEq)]
pub struct FacetRelationshipPlan {
    pub relationships: Vec<PlannedFacetRelationship>,
    pub damaged_paths: Vec<String>,
}

/// Plan facet relationships across every facet in the journal.
///
/// Encounters unreadable directories or malformed links without halting,
/// collecting them into `damaged_paths` while preserving traversal order.
pub fn plan_facet_entity_relationships(
    journal_root: &Path,
) -> Result<FacetRelationshipPlan, FacetStoreError> {
    let mut relationships = Vec::new();
    let mut damaged_paths = Vec::new();
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
                damaged_paths.push(entities_rel);
                continue;
            }
        };
        let entities_exists = match path_lexists(&entities_path) {
            Ok(exists) => exists,
            Err(_) => {
                damaged_paths.push(entities_rel);
                continue;
            }
        };
        if !entities_exists {
            continue;
        }
        let rel_entries = match list_dir_entries(&entities_path) {
            Ok(entries) => entries,
            Err(_) => {
                damaged_paths.push(entities_rel);
                continue;
            }
        };
        for rel_entry in rel_entries
            .into_iter()
            .filter(|e| e.kind == DirEntryKind::Directory)
        {
            let relationship_dir = rel_entry.name.to_string_lossy().into_owned();
            let link_rel = crate::facet_links::LinkDirs::for_facet(journal_root, &facet_dir)
                .link_rel(&relationship_dir);
            let link_path = match contained_path(journal_root, &link_rel) {
                Ok(p) => p,
                Err(_) => {
                    damaged_paths.push(link_rel);
                    continue;
                }
            };
            let link_exists = match path_lexists(&link_path) {
                Ok(exists) => exists,
                Err(_) => {
                    damaged_paths.push(link_rel);
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
                    damaged_paths.push(link_rel);
                    continue;
                }
            };
            let is_detached = link.value().get("detached") == Some(&Value::Bool(true));
            let detached_by = link
                .value()
                .get("detached_by")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let entity_id = link.entity_id().to_owned();
            let value = link.into_value();
            relationships.push(PlannedFacetRelationship {
                entity_id,
                facet_dir: facet_dir.clone(),
                relationship_dir,
                is_detached,
                detached_by,
                value,
            });
        }
    }
    Ok(FacetRelationshipPlan {
        relationships,
        damaged_paths,
    })
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
    if solstone_core_entity::entity_is_hide_protected(&identity) {
        return Err(FacetEntityLifecycleError::PrincipalEntityProtected {
            entity_id: entity_id.to_owned(),
        });
    }

    let plan = plan_facet_entity_relationships(journal_root)?;
    if !plan.damaged_paths.is_empty() {
        return Err(FacetEntityLifecycleError::RelationshipsUnreadable {
            path: plan.damaged_paths[0].clone(),
        });
    }
    let planned: Vec<PlannedFacetRelationship> = plan
        .relationships
        .into_iter()
        .filter(|rel| rel.entity_id == entity_id)
        .collect();

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
            let link_rel = crate::facet_links::LinkDirs::for_facet(journal_root, &facet_dir)
                .link_rel(&relationship_dir);
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

/// Hide multiple journal entities and detach their facet relationships under one bulk operation.
pub fn block_journal_entities(
    journal_root: &Path,
    operation: &str,
    ids: &[String],
) -> Result<BulkBlockReport, BulkJournalError> {
    if !is_valid_operation_id(operation) {
        return Err(BulkJournalError::InvalidOperationId(operation.to_owned()));
    }
    if ids.is_empty() || ids.len() > 50 {
        return Err(BulkJournalError::InvalidIdsCount(ids.len()));
    }
    if is_operation_undone(operation) {
        return Err(BulkJournalError::OperationUndone(operation.to_owned()));
    }

    let mut seen = HashSet::new();
    let mut deduped_ids = Vec::new();
    for id in ids {
        if seen.insert(id.as_str()) {
            deduped_ids.push(id.clone());
        }
    }

    let _facet_trust =
        hold_facet_trust_lock(journal_root).map_err(BulkJournalError::FacetTrustLock)?;
    let _entity_trust = solstone_core_entity::hold_entity_trust_lock(journal_root)
        .map_err(BulkJournalError::EntityTrustLock)?;

    let identity_map =
        read_identity_map(journal_root).map_err(|e| BulkJournalError::PreWrite(e.to_string()))?;
    let plan = plan_facet_entity_relationships(journal_root)
        .map_err(|e| BulkJournalError::PreWrite(e.to_string()))?;

    // One read_entity_identity per resolved requested id; do not read the same identity twice.
    let mut resolved_identities: HashMap<String, Option<IdentitySnapshot>> = HashMap::new();
    for id in &deduped_ids {
        if let Some(entity_dir) = identity_map.resolved.get(id)
            && !resolved_identities.contains_key(entity_dir)
        {
            let identity = read_entity_identity(journal_root, entity_dir)
                .map_err(|e| BulkJournalError::PreWrite(e.to_string()))?;
            resolved_identities.insert(entity_dir.clone(), identity);
        }
    }

    // Pre-check hide-protected: principal wins over damaged paths
    for id in &deduped_ids {
        if let Some(entity_dir) = identity_map.resolved.get(id)
            && let Some(Some(identity)) = resolved_identities.get(entity_dir)
            && solstone_core_entity::entity_is_hide_protected(identity)
        {
            return Err(BulkJournalError::PrincipalEntityProtected {
                entity_id: id.clone(),
            });
        }
    }

    if !plan.damaged_paths.is_empty() {
        return Err(BulkJournalError::RelationshipsUnreadable {
            path: plan.damaged_paths[0].clone(),
        });
    }

    let mut durable_written = false;
    let mut hidden = Vec::new();
    let mut already_hidden = Vec::new();
    let mut missing = Vec::new();
    let mut unreadable = Vec::new();
    let mut stopped: Option<StoppedReport> = None;

    for id in &deduped_ids {
        let Some(entity_dir) = identity_map.resolved.get(id) else {
            missing.push(id.clone());
            continue;
        };
        let Some(Some(identity)) = resolved_identities.get(entity_dir) else {
            missing.push(id.clone());
            continue;
        };

        let is_hidden = solstone_core_entity::entity_is_hidden(identity.value());
        if !is_hidden {
            let mut blocked_identity = identity.value().clone();
            let object = blocked_identity
                .as_object_mut()
                .expect("identity reader returns an object");
            object.insert("blocked".to_owned(), Value::Bool(true));
            object.insert("updated_at".to_owned(), Value::String(now_iso()));
            let operation_ctx = EntityOperationContext {
                kind: EntityOperationKind::Update,
                caller: Value::Null,
                actor: Value::Null,
                metadata: json!({ "bulk_operation": operation }),
            };

            match save_entity_identity_with_map(
                journal_root,
                id,
                &blocked_identity,
                Some(&operation_ctx),
                &identity_map,
                None,
            ) {
                Ok(_) => {
                    durable_written = true;
                }
                Err(save_err) => {
                    if save_err.durable || durable_written {
                        durable_written = true;
                        stopped = Some(StoppedReport {
                            reason: "entity_save_failed".to_owned(),
                            id: Some(id.clone()),
                        });
                        break;
                    } else {
                        return Err(BulkJournalError::PreWrite(save_err.error.to_string()));
                    }
                }
            }

            // Strip detached_by on already-detached planned links for this id
            let mut rel_err = false;
            for rel in &plan.relationships {
                if rel.entity_id == *id && rel.is_detached {
                    let mut rel_val = rel.value.clone();
                    if let Some(obj) = rel_val.as_object_mut()
                        && mutate_strip_detached_by(obj)
                    {
                        if write_facet_entity_link_raw(
                            journal_root,
                            &rel.facet_dir,
                            &rel.relationship_dir,
                            &rel_val,
                        )
                        .is_err()
                        {
                            stopped = Some(StoppedReport {
                                reason: "relationship_write_failed".to_owned(),
                                id: Some(id.clone()),
                            });
                            rel_err = true;
                            break;
                        }
                        durable_written = true;
                    }
                }
            }
            if rel_err {
                break;
            }

            // Relationship failure test hook
            #[cfg(any(test, feature = "test-hooks"))]
            if test_hooks::check_fail_batch_relationship(id) {
                stopped = Some(StoppedReport {
                    reason: "relationship_write_failed".to_owned(),
                    id: Some(id.clone()),
                });
                break;
            }

            // Mark links that are not detached from planned value
            for rel in &plan.relationships {
                if rel.entity_id == *id && !rel.is_detached {
                    let mut rel_val = rel.value.clone();
                    if let Some(obj) = rel_val.as_object_mut()
                        && mutate_detach_link(obj, Some(HIDE_DETACHED_BY))
                    {
                        if write_facet_entity_link_raw(
                            journal_root,
                            &rel.facet_dir,
                            &rel.relationship_dir,
                            &rel_val,
                        )
                        .is_err()
                        {
                            stopped = Some(StoppedReport {
                                reason: "relationship_write_failed".to_owned(),
                                id: Some(id.clone()),
                            });
                            rel_err = true;
                            break;
                        }
                        durable_written = true;
                    }
                }
            }
            if rel_err {
                break;
            }

            hidden.push(id.clone());
        } else {
            // Already hidden
            match reconcile_prepared_history_reporting(journal_root, entity_dir) {
                Ok(published_or_discarded) => {
                    if published_or_discarded {
                        durable_written = true;
                    }
                }
                Err(failure) => {
                    if failure.durable {
                        durable_written = true;
                    }
                    if durable_written {
                        stopped = Some(StoppedReport {
                            reason: "reconcile_failed".to_owned(),
                            id: Some(id.clone()),
                        });
                        break;
                    } else {
                        return Err(BulkJournalError::PreWrite(failure.error.to_string()));
                    }
                }
            }

            let history_events = match read_visible_history(journal_root, entity_dir) {
                Ok(events) => events,
                Err(e) => {
                    unreadable.push(UnreadableEntityRecord {
                        id: id.clone(),
                        path: relative_history_error_path(journal_root, entity_dir, Some(&e)),
                    });
                    continue;
                }
            };

            let hide_event = match validate_events_and_find_hide_event(&history_events) {
                Ok(ev) => ev,
                Err(_) => {
                    unreadable.push(UnreadableEntityRecord {
                        id: id.clone(),
                        path: relative_history_error_path(journal_root, entity_dir, None),
                    });
                    continue;
                }
            };

            if let Some(ev) = hide_event {
                if is_hide_event_for_operation(ev, operation) {
                    // If any planned link is still attached, mark those from planned value
                    let mut rel_err = false;
                    for rel in &plan.relationships {
                        if rel.entity_id == *id && !rel.is_detached {
                            let mut rel_val = rel.value.clone();
                            if let Some(obj) = rel_val.as_object_mut()
                                && mutate_detach_link(obj, Some(HIDE_DETACHED_BY))
                            {
                                if write_facet_entity_link_raw(
                                    journal_root,
                                    &rel.facet_dir,
                                    &rel.relationship_dir,
                                    &rel_val,
                                )
                                .is_err()
                                {
                                    stopped = Some(StoppedReport {
                                        reason: "relationship_write_failed".to_owned(),
                                        id: Some(id.clone()),
                                    });
                                    rel_err = true;
                                    break;
                                }
                                durable_written = true;
                            }
                        }
                    }
                    if rel_err {
                        break;
                    }
                    hidden.push(id.clone());
                } else {
                    already_hidden.push(id.clone());
                }
            } else {
                already_hidden.push(id.clone());
            }
        }
    }

    if durable_written
        && write_resolved_identity_map_cache(journal_root, &identity_map.resolved).is_err()
        && stopped.is_none()
    {
        stopped = Some(StoppedReport {
            reason: "cache_update_failed".to_owned(),
            id: None,
        });
    }

    Ok(BulkBlockReport {
        hidden,
        already_hidden,
        missing,
        unreadable,
        stopped,
    })
}

/// Undo a previously executed bulk block operation, restoring up to `limit` entities.
pub fn unblock_journal_operation(
    journal_root: &Path,
    operation: &str,
    limit: i64,
) -> Result<BulkUnblockReport, BulkJournalError> {
    if !is_valid_operation_id(operation) {
        return Err(BulkJournalError::InvalidOperationId(operation.to_owned()));
    }
    if !(1..=50).contains(&limit) {
        return Err(BulkJournalError::InvalidLimit(limit));
    }

    mark_operation_undone(operation);

    let _facet_trust =
        hold_facet_trust_lock(journal_root).map_err(BulkJournalError::FacetTrustLock)?;
    let _entity_trust = solstone_core_entity::hold_entity_trust_lock(journal_root)
        .map_err(BulkJournalError::EntityTrustLock)?;

    let identity_map =
        read_identity_map(journal_root).map_err(|e| BulkJournalError::PreWrite(e.to_string()))?;
    let plan = plan_facet_entity_relationships(journal_root)
        .map_err(|e| BulkJournalError::PreWrite(e.to_string()))?;

    let skipped = plan.damaged_paths.clone();

    let mut durable_written = false;
    let mut unreadable = Vec::new();
    let mut failed = Vec::new();
    let mut candidate_members = Vec::new();

    // Iterate resolved identities: one identity read per resolved entity
    for (effective_id, entity_dir) in &identity_map.resolved {
        let identity = match read_entity_identity(journal_root, entity_dir) {
            Ok(Some(i)) => i,
            _ => continue,
        };

        if !solstone_core_entity::entity_is_hidden(identity.value()) {
            continue;
        }

        let reconcile_result = reconcile_prepared_history_reporting(journal_root, entity_dir);
        if reconcile_result
            .as_ref()
            .is_ok_and(|&reconciled| reconciled)
            || reconcile_result
                .as_ref()
                .is_err_and(|failure| failure.durable)
        {
            durable_written = true;
        }

        let history_events = match read_visible_history(journal_root, entity_dir) {
            Ok(events) => events,
            Err(e) => {
                unreadable.push(UnreadableEntityRecord {
                    id: effective_id.clone(),
                    path: relative_history_error_path(journal_root, entity_dir, Some(&e)),
                });
                continue;
            }
        };

        let hide_event = match validate_events_and_find_hide_event(&history_events) {
            Ok(ev) => ev,
            Err(_) => {
                unreadable.push(UnreadableEntityRecord {
                    id: effective_id.clone(),
                    path: relative_history_error_path(journal_root, entity_dir, None),
                });
                continue;
            }
        };

        let is_member = hide_event.is_some_and(|ev| is_hide_event_for_operation(ev, operation));
        if !is_member {
            continue;
        }

        match reconcile_result {
            Ok(_) => {
                candidate_members.push((
                    effective_id.clone(),
                    entity_dir.clone(),
                    identity,
                    history_events,
                ));
            }
            Err(failure) => match failure.error {
                EntityWriteError::ReconciliationRepairRequired { .. } => {
                    failed.push(FailedEntity {
                        id: effective_id.clone(),
                        reason: "reconciliation_repair_required".to_owned(),
                    });
                }
                _ => {
                    failed.push(FailedEntity {
                        id: effective_id.clone(),
                        reason: "reconciliation_failed".to_owned(),
                    });
                }
            },
        }
    }

    // Sort candidate members by effective id ascending
    candidate_members.sort_by(|a, b| a.0.cmp(&b.0));

    let mut restored = Vec::new();
    let mut stopped: Option<StoppedReport> = None;
    let limit_usize = limit as usize;

    for (id, _entity_dir, identity, events) in &candidate_members {
        if restored.len() >= limit_usize {
            break;
        }

        #[cfg(any(test, feature = "test-hooks"))]
        if test_hooks::check_fail_undo_after_n_restores(restored.len()) {
            stopped = Some(StoppedReport {
                reason: "undo_interrupted".to_owned(),
                id: None,
            });
            break;
        }

        #[cfg(any(test, feature = "test-hooks"))]
        let force_unhide_fail = test_hooks::check_fail_unhide_save(id);
        #[cfg(not(any(test, feature = "test-hooks")))]
        let force_unhide_fail = false;

        if force_unhide_fail {
            failed.push(FailedEntity {
                id: id.clone(),
                reason: "unhide_save_failed".to_owned(),
            });
            continue;
        }

        // Reattach only planned links whose detached_by == "hide"
        let mut facets_reattached = Vec::new();
        let mut rel_write_failed = false;

        for rel in &plan.relationships {
            if rel.entity_id == *id && rel.detached_by.as_deref() == Some(HIDE_DETACHED_BY) {
                let mut rel_val = rel.value.clone();
                if let Some(obj) = rel_val.as_object_mut()
                    && mutate_reattach_link(obj)
                {
                    if write_facet_entity_link_raw(
                        journal_root,
                        &rel.facet_dir,
                        &rel.relationship_dir,
                        &rel_val,
                    )
                    .is_err()
                    {
                        failed.push(FailedEntity {
                            id: id.clone(),
                            reason: "relationship_write_failed".to_owned(),
                        });
                        rel_write_failed = true;
                        break;
                    }
                    durable_written = true;
                    if !facets_reattached.contains(&rel.facet_dir) {
                        facets_reattached.push(rel.facet_dir.clone());
                    }
                }
            }
        }

        if rel_write_failed {
            continue;
        }

        let mut unblocked_identity = identity.value().clone();
        let object = unblocked_identity
            .as_object_mut()
            .expect("identity reader returns an object");
        object.remove("blocked");
        object.insert("updated_at".to_owned(), Value::String(now_iso()));
        let operation_ctx = EntityOperationContext {
            kind: EntityOperationKind::Update,
            caller: Value::Null,
            actor: Value::Null,
            metadata: json!({}),
        };

        match save_entity_identity_with_map(
            journal_root,
            id,
            &unblocked_identity,
            Some(&operation_ctx),
            &identity_map,
            Some(events),
        ) {
            Ok(_) => {
                durable_written = true;
                restored.push(RestoredEntity {
                    id: id.clone(),
                    facets_reattached,
                });
            }
            Err(save_err) => {
                if save_err.durable {
                    durable_written = true;
                }
                failed.push(FailedEntity {
                    id: id.clone(),
                    reason: "unhide_save_failed".to_owned(),
                });
            }
        }
    }

    let restored_or_failed: HashSet<&str> = restored
        .iter()
        .map(|r| r.id.as_str())
        .chain(failed.iter().map(|f| f.id.as_str()))
        .collect();
    let remaining = candidate_members
        .iter()
        .filter(|(m_id, _, _, _)| !restored_or_failed.contains(m_id.as_str()))
        .count();

    if durable_written
        && write_resolved_identity_map_cache(journal_root, &identity_map.resolved).is_err()
        && stopped.is_none()
    {
        stopped = Some(StoppedReport {
            reason: "cache_update_failed".to_owned(),
            id: None,
        });
    }

    Ok(BulkUnblockReport {
        restored,
        skipped,
        failed,
        remaining,
        unreadable,
        stopped,
    })
}

#[cfg(all(test, unix))]
mod relative_history_error_path_tests {
    use super::relative_history_error_path;
    use solstone_core_entity::EntityStoreError;

    #[test]
    fn a_root_reached_through_a_symlink_still_reports_a_journal_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let real_root = dir.path().join("real");
        std::fs::create_dir(&real_root).unwrap();
        let linked_root = dir.path().join("linked");
        std::os::unix::fs::symlink(&real_root, &linked_root).unwrap();
        let canonical_root = std::fs::canonicalize(&linked_root).unwrap();
        let error = EntityStoreError::HistoryEventNotObject {
            path: canonical_root
                .join("entities/someone/history/events/00000000000000000001-a.json"),
        };

        assert_eq!(
            relative_history_error_path(&linked_root, "someone", Some(&error)),
            "entities/someone/history/events/00000000000000000001-a.json"
        );
    }
}
