// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's journal-entity delete, held for its cancel window.
//!
//! The hold itself (the record, its window, the resume at start and the
//! outcome) is [`solstone_core_serving::held_delete`], shared with the segment
//! delete. This module is the entity's half: which entity the owner confirmed,
//! how to recognise it again, and what removing it did.
//!
//! What the owner confirmed is the entity, not a snapshot of it. Observations,
//! links and edits that reach it before the delete runs go with it, as they
//! always have inside the window. What a delete never removes is a different
//! entity that has since taken the same id: the record keeps the directory the
//! id resolved to and the identity's `created_at`, which nothing but creation
//! writes, and a delete runs only while both still match.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_serving::held_delete::{Claim, DeleteState, Record, Registry, Store};

use crate::action_log;

/// Where the records live, relative to the journal root.
pub(crate) const STORE: Store = Store::new("config/entity-deletes");

/// Why a delete keeps an entity that is no longer the one the owner chose.
const CHANGED_REASON: &str =
    "a new entity has taken its place since you chose to delete it, so the new one was kept";
/// Why a resumed delete keeps an entity it has nothing to compare against.
const UNCONFIRMED_REASON: &str =
    "your journal couldn't confirm this was the entity you chose, so it was kept";
/// Why a delete finds nothing to remove.
const GONE_REASON: &str = "it was merged or removed before the delete ran";

/// The entity the owner confirmed. Its fields sit at the top level of the record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct EntityTarget {
    pub(crate) entity_id: String,
    /// The directory the id resolved to when the owner confirmed.
    pub(crate) entity_dir: String,
    /// The name the owner saw, for reporting the outcome later.
    #[serde(default)]
    pub(crate) name: String,
    /// The identity's `created_at` at confirmation. Some older entities have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) created_at: Option<Value>,
}

pub(crate) type DeleteRecord = Record<EntityTarget>;

/// What the id names now, compared with what the owner confirmed.
enum Found {
    Same,
    Gone,
    Changed,
    Unconfirmed,
}

/// Resolve the id the way the delete itself does and compare it with the
/// confirmed entity.
pub(crate) fn current_target(
    journal_root: &Path,
    entity_id: &str,
) -> Result<Option<(EntityTarget, Value)>, String> {
    let map = solstone_core_entity::read_identity_map(journal_root).map_err(|e| e.to_string())?;
    let Some(entity_dir) = map.resolved.get(entity_id).cloned() else {
        return Ok(None);
    };
    let Some(identity) = solstone_core_entity::read_entity_identity(journal_root, &entity_dir)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let value = identity.value().clone();
    Ok(Some((
        EntityTarget {
            entity_id: entity_id.to_owned(),
            entity_dir,
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            // A `null` would not survive the record's round trip; it is no
            // stamp at all.
            created_at: value.get("created_at").filter(|v| !v.is_null()).cloned(),
        },
        value,
    )))
}

fn find(journal_root: &Path, confirmed: &EntityTarget, resumed: bool) -> Result<Found, String> {
    let Some((now, _)) = current_target(journal_root, &confirmed.entity_id)? else {
        return Ok(Found::Gone);
    };
    if now.entity_dir != confirmed.entity_dir {
        return Ok(Found::Changed);
    }
    Ok(match (&confirmed.created_at, &now.created_at) {
        (Some(confirmed), Some(now)) if confirmed == now => Found::Same,
        (None, None) if resumed => Found::Unconfirmed,
        (None, None) => Found::Same,
        _ => Found::Changed,
    })
}

/// Facets holding a link to the entity. An unreadable listing or link is an
/// error, never an absence.
fn linked_facets(journal_root: &Path, entity_id: &str) -> Result<Vec<String>, String> {
    let mut linked = Vec::new();
    for facet in
        solstone_core_facets::list_facet_directories(journal_root).map_err(|e| e.to_string())?
    {
        for dir in solstone_core_facets::list_facet_entity_directories(journal_root, &facet)
            .map_err(|e| e.to_string())?
        {
            let link = solstone_core_facets::read_facet_entity_link(journal_root, &facet, &dir)
                .map_err(|e| e.to_string())?;
            if link.is_some_and(|link| link.entity_id() == entity_id) {
                linked.push(facet);
                break;
            }
        }
    }
    Ok(linked)
}

/// Whether anything is at the entity's directory. An error reading it is an
/// error, so an unreadable entity is never taken for a removed one.
fn entity_dir_present(journal_root: &Path, entity_dir: &str) -> Result<bool, String> {
    match std::fs::symlink_metadata(journal_root.join("entities").join(entity_dir)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether a record names an entity the delete route itself would accept.
pub(crate) fn resumable(record: &DeleteRecord) -> bool {
    !record.target.entity_id.is_empty() && plain_name(&record.target.entity_dir)
}

/// A plain directory name, as the identity map produces. The records live in
/// `config/`, which travels with backups.
fn plain_name(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\'])
}

pub(crate) fn schedule(
    registry: &Registry,
    journal_root: &Path,
    pending_id: String,
    delay: Duration,
) {
    let root = journal_root.to_path_buf();
    let id = pending_id.clone();
    registry.schedule(pending_id, delay, move || commit(&root, &id, false));
}

/// Resume every delete a previous run confirmed and never finished; see
/// [`solstone_core_serving::held_delete::Store::resume`].
pub(crate) fn resume_pending(journal_root: &Path, registry: &Registry) {
    STORE.resume(
        journal_root,
        registry,
        "entity-delete-resume",
        resumable,
        |root, pending_id| commit(root, pending_id, true),
    );
}

/// Run one delete to its outcome under the record's lock.
pub(crate) fn commit(journal_root: &Path, pending_id: &str, resumed: bool) {
    STORE.commit(
        journal_root,
        pending_id,
        resumed,
        |record: &DeleteRecord, claim| {
            // `None` from the removal: what it did cannot be read back now. The
            // record stays pending, with its claim, and the next start settles
            // it from what is then on disk.
            let (phase, detail, state, reason) = match run_delete(journal_root, record, claim) {
                Ok(outcome) => outcome,
                Err(detail) => {
                    let _ = action_log::outcome(
                        journal_root,
                        &record.target.entity_id,
                        pending_id,
                        "failed",
                        merge(json!({"unsettled":true}), detail),
                    );
                    return None;
                }
            };
            let marker = if claim.resumed {
                json!({"resumed":true})
            } else {
                json!({})
            };
            let _ = action_log::outcome(
                journal_root,
                &record.target.entity_id,
                pending_id,
                phase,
                merge(marker, detail),
            );
            Some((state, reason.map(str::to_owned)))
        },
    );
}

type Outcome = (&'static str, Value, DeleteState, Option<&'static str>);

/// How long a commit keeps asking for the trust locks before leaving the
/// record pending. A new delete of the entity re-arms it; otherwise the next
/// start does. Short, so a blocking thread never holds a stopping journal.
const TRUST_LOCK_PATIENCE: Duration = Duration::from_secs(60);

fn refused(error: String) -> Outcome {
    (
        "refused",
        json!({"error":error}),
        DeleteState::NotDeleted,
        None,
    )
}

/// The outcome, or `Err` with log detail when it cannot be settled now; the
/// record then stays pending and the next start settles it.
///
/// Two cases, split on whether this delete's removal had already started in
/// a run that stopped:
/// - it had not: nothing of this delete has touched the journal, so the
///   entity is judged against what the owner confirmed;
/// - it had: some of it may already be gone, so the removal is finished, and
///   the outcome is read from what is left, never from a guess.
fn run_delete(journal_root: &Path, record: &DeleteRecord, claim: Claim) -> Result<Outcome, Value> {
    let target = &record.target;
    // Hold both trust locks, in the delete's own order, from the check through
    // the removal, so nothing can take the id in between. Both are reentrant
    // on this thread.
    let deadline = std::time::Instant::now() + TRUST_LOCK_PATIENCE;
    let (_facet, _entity) = loop {
        let held = solstone_core_entity::hold_facet_trust_lock(journal_root)
            .map_err(|error| error.to_string())
            .and_then(|facet| {
                solstone_core_entity::hold_entity_trust_lock(journal_root)
                    .map(|entity| (facet, entity))
                    .map_err(|error| error.to_string())
            });
        match held {
            Ok(locks) => break locks,
            Err(error) if std::time::Instant::now() >= deadline => {
                return Err(json!({"error":error}));
            }
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    };
    if claim.removal_started {
        return finish_started_removal(journal_root, record);
    }
    // Nothing has changed yet: a journal that cannot be read here is a
    // refusal, not a guess, and not a retry of a fault that may never clear.
    let found = match find(journal_root, target, claim.resumed) {
        Ok(found) => found,
        Err(error) => return Ok(refused(error)),
    };
    Ok(match found {
        Found::Gone => (
            "refused",
            json!({"reason":GONE_REASON}),
            DeleteState::NotDeleted,
            Some(GONE_REASON),
        ),
        Found::Changed => (
            "refused",
            json!({"reason":CHANGED_REASON}),
            DeleteState::NotDeleted,
            Some(CHANGED_REASON),
        ),
        Found::Unconfirmed => (
            "refused",
            json!({"reason":UNCONFIRMED_REASON}),
            DeleteState::NotDeleted,
            Some(UNCONFIRMED_REASON),
        ),
        Found::Same => {
            let linked_before = match linked_facets(journal_root, &target.entity_id) {
                Ok(linked) => linked,
                Err(error) => return Ok(refused(error)),
            };
            STORE
                .mark_removal_started(journal_root, record)
                .map_err(|error| json!({"error":error}))?;
            let error = match delete(journal_root, target) {
                Ok(done) => return Ok(done),
                Err(error) => error,
            };
            let unsettled = |read: String| json!({"error":error, "evidence_error":read});
            if !entity_dir_present(journal_root, &target.entity_dir).map_err(unsettled)? {
                return Ok(removed_despite(error));
            }
            let still_resolves = matches!(
                current_target(journal_root, &target.entity_id).map_err(unsettled)?,
                Some((now, _)) if now.entity_dir == target.entity_dir
            );
            let linked_after = linked_facets(journal_root, &target.entity_id).map_err(unsettled)?;
            let facets_deleted = linked_before
                .into_iter()
                .filter(|facet| !linked_after.contains(facet))
                .collect::<Vec<_>>();
            if still_resolves && facets_deleted.is_empty() {
                refused(error)
            } else {
                incomplete(error, json!(facets_deleted))
            }
        }
    })
}

/// Finish a removal a stopped run had started. The directory goes last, so its
/// absence means the entity is gone; while it is there the entity is removed
/// again if it is still the confirmed one, and anything else left is partial.
fn finish_started_removal(journal_root: &Path, record: &DeleteRecord) -> Result<Outcome, Value> {
    let target = &record.target;
    let unsettled = |read: String| json!({"evidence_error":read});
    if !entity_dir_present(journal_root, &target.entity_dir).map_err(unsettled)? {
        return Ok((
            "committed",
            json!({"already_removed":true}),
            DeleteState::Deleted,
            None,
        ));
    }
    match find(journal_root, target, false).map_err(unsettled)? {
        Found::Same => match delete(journal_root, target) {
            Ok(done) => Ok(done),
            Err(error) => {
                let unsettled = |read: String| json!({"error":error, "evidence_error":read});
                if entity_dir_present(journal_root, &target.entity_dir).map_err(unsettled)? {
                    Ok(incomplete(error, Value::Null))
                } else {
                    Ok(removed_despite(error))
                }
            }
        },
        // The directory is left without the identity the owner confirmed.
        _ => Ok(incomplete(
            "the entity directory remains without its confirmed identity".to_owned(),
            Value::Null,
        )),
    }
}

/// The store's delete. Its error text can name paths, so it goes to the action
/// log and never to the owner.
fn delete(journal_root: &Path, target: &EntityTarget) -> Result<Outcome, String> {
    solstone_core_facets::delete_journal_entity(journal_root, &target.entity_id)
        .map(|report| {
            (
                "committed",
                json!({"facets_deleted":report.facets_deleted}),
                DeleteState::Deleted,
                None,
            )
        })
        .map_err(|error| error.to_string())
}

/// The entity itself is gone; a later bookkeeping step failed.
fn removed_despite(error: String) -> Outcome {
    (
        "failed",
        json!({"error":error,"entity_removed":true}),
        DeleteState::Deleted,
        None,
    )
}

fn incomplete(error: String, facets_deleted: Value) -> Outcome {
    (
        "failed",
        json!({"error":error,"facets_deleted":facets_deleted,"entity_removed":false}),
        DeleteState::Incomplete,
        None,
    )
}

fn merge(base: Value, extra: Value) -> Value {
    match (base, extra) {
        (Value::Object(mut base), Value::Object(extra)) => {
            base.extend(extra);
            Value::Object(base)
        }
        (_, extra) => extra,
    }
}

/// Finish a waiting delete of an earlier entity under the id, now that the
/// owner is deleting the entity that has taken its place, so it never later
/// reports the new one as kept.
///
/// Only a record no commit has claimed is settled; the timer is dropped only
/// once the record says so. Returns whether it was settled.
pub(crate) fn settle_superseded(
    journal_root: &Path,
    registry: &Registry,
    stale: &DeleteRecord,
) -> bool {
    let settled = STORE.settle_unclaimed::<EntityTarget>(
        journal_root,
        &stale.pending_id,
        DeleteState::NotDeleted,
        Some(CHANGED_REASON.to_owned()),
        || {
            let _ = action_log::outcome(
                journal_root,
                &stale.target.entity_id,
                &stale.pending_id,
                "refused",
                json!({"reason":CHANGED_REASON,"superseded":true}),
            );
        },
    );
    let done = matches!(
        settled,
        solstone_core_serving::held_delete::Settled::Record(record)
            if record.state == DeleteState::NotDeleted
    );
    if done {
        registry.cancel(&stale.pending_id);
    }
    done
}

/// What became of one delete, for the page.
pub(crate) fn status_body(record: &DeleteRecord) -> Value {
    let mut body = json!({
        "pending": record.pending_id,
        "state": record.state.as_str(),
        "entity_id": record.target.entity_id,
        "name": record.target.name,
        "commit_at_ms": record.commit_at_ms,
    });
    if let Some(reason) = &record.reason {
        body["reason"] = json!(reason);
    }
    body
}
