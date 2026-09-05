// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::DirEntryKind;
use solstone_core_journal_io::JournalSnapshot;
use solstone_core_journal_io::JsonWriteOptions;
use solstone_core_journal_io::MalformedPolicy;
use solstone_core_journal_io::SnapshotDirectory;
use solstone_core_journal_io::SnapshotError;
use solstone_core_journal_io::SnapshotFile;
use solstone_core_journal_io::contained_path;
use solstone_core_journal_io::list_dir_entries;
use solstone_core_journal_io::path_lexists;
use solstone_core_journal_io::read_json;
use solstone_core_journal_io::read_jsonl;
use solstone_core_journal_io::restore_snapshot;
use solstone_core_journal_io::write_json;
use solstone_core_journal_io::write_jsonl;

use crate::{
    EntityOperationContext, EntityOperationKind, EntityWriteError, hold_entity_trust_lock,
    read_entity_identity, read_visible_history, save_entity_identity,
};

use super::lifecycle::resolve_entity_dir;
use super::merge_payload::{
    MergePayloadError, list_entity_merge_payload_ids, load_entity_merge_payload,
    move_entity_merge_payload, record_entity_merge_payload, remove_entity_merge_payload,
    snapshot_from_payload,
};
use super::merge_rollback::MergeRollback;

type FailureInjector = dyn Fn(&str, usize) -> bool;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EntityUndoReport {
    pub merge_id: String,
    pub source_id: String,
    pub target_id: String,
}

#[derive(Debug)]
pub enum EntityUndoError {
    Refused(String),
    Payload(MergePayloadError),
    Write(EntityWriteError),
    Snapshot(SnapshotError),
    Index(solstone_core_indexer_store::StoreError),
    Failed {
        failed_phase: String,
        report: Box<EntityUndoReport>,
        rollback_error: Option<String>,
    },
}

impl fmt::Display for EntityUndoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(message) => formatter.write_str(message),
            Self::Payload(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Snapshot(error) => error.fmt(formatter),
            Self::Index(error) => error.fmt(formatter),
            Self::Failed {
                failed_phase,
                rollback_error: Some(error),
                ..
            } => {
                write!(
                    formatter,
                    "entity merge undo failed during {failed_phase}: {error}"
                )
            }
            Self::Failed { failed_phase, .. } => {
                write!(formatter, "entity merge undo failed during {failed_phase}")
            }
        }
    }
}

impl Error for EntityUndoError {}

impl From<MergePayloadError> for EntityUndoError {
    fn from(error: MergePayloadError) -> Self {
        Self::Payload(error)
    }
}

impl From<EntityWriteError> for EntityUndoError {
    fn from(error: EntityWriteError) -> Self {
        Self::Write(error)
    }
}

impl From<SnapshotError> for EntityUndoError {
    fn from(error: SnapshotError) -> Self {
        Self::Snapshot(error)
    }
}

pub fn undo_entity_merge(
    journal: &Path,
    merge_id: &str,
    caller: Value,
) -> Result<EntityUndoReport, EntityUndoError> {
    undo_entity_merge_with_injector(journal, merge_id, caller, None)
}

pub(crate) fn undo_entity_merge_with_injector(
    journal: &Path,
    merge_id: &str,
    caller: Value,
    injector: Option<&FailureInjector>,
) -> Result<EntityUndoReport, EntityUndoError> {
    let canonical_journal =
        solstone_core_journal_io::realpath_non_strict(journal).map_err(SnapshotError::Path)?;
    let journal = canonical_journal.as_path();
    let _trust = hold_entity_trust_lock(journal)
        .map_err(|error| EntityUndoError::Write(EntityWriteError::TrustLock(error)))?;
    if let Some(recovered) = super::merge_rollback::recover(journal)?
        && recovered["operation"] == "undo"
        && recovered["report"]["merge_id"] == merge_id
    {
        return serde_json::from_value(recovered["report"].clone())
            .map_err(|error| EntityUndoError::Refused(error.to_string()));
    }
    let target_id = find_payload_holder(journal, merge_id)?;
    let target_dir = resolve_entity_dir(journal, &target_id)
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
    let payload = load_entity_merge_payload(journal, &target_dir, merge_id)?;
    let active_payloads = load_active_sibling_payloads(journal, &target_dir, merge_id)?;
    let source_id = payload
        .get("source_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            EntityUndoError::Refused("merge payload missing source entity id".to_owned())
        })?
        .to_owned();
    let mut rollback = MergeRollback::default();
    preflight_file_restoration(journal, &target_dir, &payload, &mut rollback)?;
    let source_snapshots = preflight_source_restoration(
        journal,
        &target_id,
        source_state_snapshots(&payload)?,
        &payload,
    )?;
    let report = EntityUndoReport {
        merge_id: merge_id.to_owned(),
        source_id: source_id.clone(),
        target_id: target_id.clone(),
    };
    rollback.start(journal)?;
    let mut phase = "snapshot";
    let result: Result<(), EntityUndoError> = (|| {
        let mut paths = HashSet::from([format!("entities/{target_dir}")]);
        paths.extend(source_snapshots.iter().map(snapshot_path));
        for path in paths {
            rollback.capture(journal, &path)?;
        }
        rollback.checkpoint(journal)?;
        phase = "source_state";
        for snapshot in &source_snapshots {
            restore_snapshot(journal, snapshot)?;
        }
        rollback.checkpoint(journal)?;
        phase = "voiceprints";
        undo_voiceprints(journal, &target_dir, &payload, &mut rollback)?;
        rollback.checkpoint(journal)?;
        phase = "segments";
        undo_segments(journal, &payload, &mut rollback, injector)?;
        rollback.checkpoint(journal)?;
        phase = "activities";
        undo_activities(journal, &payload, &mut rollback, injector)?;
        rollback.checkpoint(journal)?;
        phase = "observations";
        undo_observation_relations(journal, &payload, &mut rollback, injector)?;
        rollback.checkpoint(journal)?;
        phase = "facets";
        undo_facets(journal, &target_id, &payload, &mut rollback, injector)?;
        rollback.checkpoint(journal)?;
        phase = "lineage";
        let source_dir = resolve_entity_dir(journal, &source_id)
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        undo_rebased_payloads(
            journal,
            &target_dir,
            &source_dir,
            &source_id,
            &payload,
            &mut rollback,
        )?;
        rollback.checkpoint(journal)?;
        phase = "identity";
        let target_after_undo =
            replay_target_identity(journal, &target_dir, &payload, &active_payloads)?;
        save_entity_identity(
            journal,
            &target_id,
            &target_after_undo,
            Some(&EntityOperationContext {
                kind: EntityOperationKind::MergeUndo,
                caller,
                actor: Value::Null,
                metadata: serde_json::json!({
                    "undo_of": merge_id,
                    "source_id": source_id,
                    "target_id": target_id,
                }),
            }),
        )?;
        rollback.checkpoint(journal)?;
        phase = "private_payload";
        remove_entity_merge_payload(journal, &target_dir, merge_id)?;
        rollback.checkpoint(journal)?;
        Ok(())
    })();
    if let Err(error) = result {
        let rollback_error = rollback
            .restore(journal)
            .err()
            .map(|rollback_error| rollback_error.to_string());
        return Err(EntityUndoError::Failed {
            failed_phase: phase.to_owned(),
            report: Box::new(report),
            rollback_error: rollback_error.or_else(|| Some(error.to_string())),
        });
    }
    let mut committed = serde_json::json!({"operation":"undo", "report":report});
    rollback.commit_source(journal, "undo", &committed["report"])?;
    if injector.is_some_and(|injector| injector("edges", 0)) {
        return Err(EntityUndoError::Refused(
            "entity merge undo committed; index repair pending: injected failure".to_owned(),
        ));
    }
    super::merge_rollback::repair_index(journal, &mut committed).map_err(|error| {
        EntityUndoError::Refused(format!(
            "entity merge undo committed; index repair pending: {error}"
        ))
    })?;
    rollback.finish(journal).map_err(|error| {
        EntityUndoError::Refused(format!(
            "entity merge undo committed; recovery cleanup pending: {error}"
        ))
    })?;
    Ok(report)
}

fn load_active_sibling_payloads(
    journal: &Path,
    target_dir: &str,
    merge_id: &str,
) -> Result<Vec<Value>, EntityUndoError> {
    let mut payloads = Vec::new();
    for sibling_id in list_entity_merge_payload_ids(journal, target_dir)? {
        if sibling_id == merge_id {
            continue;
        }
        let payload = load_entity_merge_payload(journal, target_dir, &sibling_id).map_err(|error| {
            EntityUndoError::Refused(format!(
                "cannot undo recorded merge {merge_id}: active merge payload {sibling_id} is invalid: {error}"
            ))
        })?;
        payloads.push(payload);
    }
    Ok(payloads)
}

fn replay_target_identity(
    journal: &Path,
    target_dir: &str,
    payload: &Value,
    active_payloads: &[Value],
) -> Result<Value, EntityUndoError> {
    let mut current = read_entity_identity(journal, target_dir)
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?
        .ok_or_else(|| EntityUndoError::Refused(format!("target entity not found: {target_dir}")))?
        .value()
        .clone();
    let identity = payload
        .get("manifest")
        .and_then(|manifest| manifest.get("identity"))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            EntityUndoError::Refused("merge payload identity manifest is missing".to_owned())
        })?;
    let merge_seq = payload_sequence(payload);
    remove_supported_set_values(
        journal,
        target_dir,
        &mut current,
        "aka",
        identity.get("aka_support"),
        active_payloads,
        merge_seq,
    )?;
    remove_supported_set_values(
        journal,
        target_dir,
        &mut current,
        "emails",
        identity.get("email_support"),
        active_payloads,
        merge_seq,
    )?;
    replay_supported_scalars(
        journal,
        target_dir,
        &mut current,
        identity.get("scalar_support"),
        active_payloads,
        merge_seq,
    )?;
    Ok(current)
}

fn remove_supported_set_values(
    journal: &Path,
    target_dir: &str,
    current: &mut Value,
    field: &str,
    support: Option<&Value>,
    active_payloads: &[Value],
    merge_seq: i128,
) -> Result<(), EntityUndoError> {
    let entries = support.and_then(Value::as_array).ok_or_else(|| {
        EntityUndoError::Refused(format!("merge payload identity {field}_support is missing"))
    })?;
    let mut remove_keys = HashSet::new();
    for entry in entries {
        if entry
            .get("target_preexisting")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let key = entry.get("key").and_then(Value::as_str).ok_or_else(|| {
            EntityUndoError::Refused(format!(
                "merge payload {field} support entry is missing key"
            ))
        })?;
        if other_payload_supports(field, key, active_payloads)
            || later_owner_event_introduced(journal, target_dir, field, key, merge_seq)?
        {
            continue;
        }
        remove_keys.insert(key.to_owned());
    }
    if remove_keys.is_empty() {
        return Ok(());
    }
    if let Some(values) = current.get_mut(field).and_then(Value::as_array_mut) {
        values.retain(|value| {
            value
                .as_str()
                .is_none_or(|value| !remove_keys.contains(&value.to_lowercase()))
        });
    }
    Ok(())
}

fn replay_supported_scalars(
    journal: &Path,
    target_dir: &str,
    current: &mut Value,
    support: Option<&Value>,
    active_payloads: &[Value],
    merge_seq: i128,
) -> Result<(), EntityUndoError> {
    let entries = support.and_then(Value::as_array).ok_or_else(|| {
        EntityUndoError::Refused("merge payload identity scalar_support is missing".to_owned())
    })?;
    let object = current.as_object_mut().ok_or_else(|| {
        EntityUndoError::Refused(format!("target entity not found: {target_dir}"))
    })?;
    for entry in entries {
        let key = entry.get("key").and_then(Value::as_str).ok_or_else(|| {
            EntityUndoError::Refused("merge payload scalar support entry is missing key".to_owned())
        })?;
        if later_owner_scalar_changed(journal, target_dir, key, merge_seq)? {
            continue;
        }
        let mut missing = entry
            .get("target_prevalue_missing")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut value = entry.get("target_prevalue").cloned().unwrap_or(Value::Null);
        for sibling in active_payloads {
            if payload_sequence(sibling) <= merge_seq {
                continue;
            }
            let Some(support) = sibling
                .get("manifest")
                .and_then(|manifest| manifest.get("identity"))
                .and_then(|identity| identity.get("scalar_support"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for other in support {
                if other.get("key").and_then(Value::as_str) != Some(key) {
                    continue;
                }
                if missing && !is_missing_value(other.get("source_value")) {
                    value = other.get("source_value").cloned().unwrap_or(Value::Null);
                    missing = false;
                }
            }
        }
        if missing {
            object.remove(key);
        } else {
            object.insert(key.to_owned(), value);
        }
    }
    Ok(())
}

fn other_payload_supports(field: &str, key: &str, payloads: &[Value]) -> bool {
    let section = if field == "aka" {
        "aka_support"
    } else {
        "email_support"
    };
    payloads.iter().any(|payload| {
        payload
            .get("manifest")
            .and_then(|manifest| manifest.get("identity"))
            .and_then(|identity| identity.get(section))
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.get("key").and_then(Value::as_str) == Some(key))
            })
    })
}

fn later_owner_event_introduced(
    journal: &Path,
    target_dir: &str,
    field: &str,
    key: &str,
    merge_seq: i128,
) -> Result<bool, EntityUndoError> {
    for event in read_visible_history(journal, target_dir)
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?
    {
        let event = event.value();
        if event_sequence(event) <= merge_seq || !is_owner_event(event) {
            continue;
        }
        let before = event
            .get("identity_before")
            .and_then(|identity| identity.get(field))
            .and_then(Value::as_array);
        let after = event
            .get("identity_after")
            .and_then(|identity| identity.get(field))
            .and_then(Value::as_array);
        if !set_contains(before, key) && set_contains(after, key) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn later_owner_scalar_changed(
    journal: &Path,
    target_dir: &str,
    key: &str,
    merge_seq: i128,
) -> Result<bool, EntityUndoError> {
    for event in read_visible_history(journal, target_dir)
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?
    {
        let event = event.value();
        if event_sequence(event) > merge_seq
            && is_owner_event(event)
            && event
                .get("identity_before")
                .and_then(|identity| identity.get(key))
                != event
                    .get("identity_after")
                    .and_then(|identity| identity.get(key))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_owner_event(event: &Value) -> bool {
    matches!(
        event.get("kind").and_then(Value::as_str),
        Some("create" | "update" | "restore")
    )
}

fn event_sequence(event: &Value) -> i128 {
    event
        .get("seq")
        .and_then(Value::as_i64)
        .map(i128::from)
        .or_else(|| event.get("seq").and_then(Value::as_u64).map(i128::from))
        .unwrap_or_default()
}

fn payload_sequence(payload: &Value) -> i128 {
    payload
        .get("commit_seq")
        .and_then(Value::as_i64)
        .map(i128::from)
        .or_else(|| {
            payload
                .get("commit_seq")
                .and_then(Value::as_u64)
                .map(i128::from)
        })
        .unwrap_or_default()
}

fn set_contains(values: Option<&Vec<Value>>, key: &str) -> bool {
    values.is_some_and(|values| {
        values.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| value.to_lowercase() == key)
        })
    })
}

fn is_missing_value(value: Option<&Value>) -> bool {
    value.is_none_or(|value| value.is_null() || value.as_str() == Some(""))
}

fn inject_failure(
    injector: Option<&FailureInjector>,
    phase: &str,
    artifact_index: usize,
) -> Result<(), EntityUndoError> {
    if injector.is_some_and(|injector| injector(phase, artifact_index)) {
        return Err(EntityUndoError::Refused(format!(
            "injected failure after {phase} artifact {artifact_index}"
        )));
    }
    Ok(())
}

fn undo_facets(
    journal: &Path,
    target_id: &str,
    payload: &Value,
    rollback: &mut MergeRollback,
    injector: Option<&FailureInjector>,
) -> Result<(), EntityUndoError> {
    let entries = payload
        .get("manifest")
        .and_then(Value::as_object)
        .and_then(|manifest| manifest.get("facets"))
        .and_then(Value::as_object)
        .and_then(|facets| facets.get("entries"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            EntityUndoError::Refused("merge payload facets entries are missing".to_owned())
        })?;
    let mut artifact_index = 0;
    for entry in entries {
        let facet = entry
            .get("facet")
            .and_then(Value::as_str)
            .filter(|facet| !facet.is_empty())
            .ok_or_else(|| {
                EntityUndoError::Refused("merge payload facet entry is missing facet".to_owned())
            })?;
        let kind = entry.get("kind").and_then(Value::as_str).ok_or_else(|| {
            EntityUndoError::Refused("merge payload facet entry is missing kind".to_owned())
        })?;
        let recorded_dir = entry
            .get("target_dir")
            .and_then(Value::as_str)
            .filter(|dir| !dir.is_empty())
            .unwrap_or(target_id);
        let directory = format!("facets/{facet}/entities/{recorded_dir}");
        match kind {
            "relink" => {
                let source_entity_id = entry
                    .get("source_entity_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        EntityUndoError::Refused(
                            "merge payload facet entry is missing source_entity_id".to_owned(),
                        )
                    })?;
                let path = format!("{directory}/entity.json");
                let destination = contained_path(journal, &path)
                    .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
                rollback.capture(journal, &path)?;
                let mut link: Value = read_json(&destination, Value::Null, MalformedPolicy::Raise)
                    .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
                if let Some(object) = link.as_object_mut() {
                    object.insert(
                        "entity_id".to_owned(),
                        Value::String(source_entity_id.to_owned()),
                    );
                }
                write_json(
                    destination,
                    &link,
                    JsonWriteOptions {
                        indent: Some(2),
                        sort_keys: false,
                        mode: None,
                    },
                )
                .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
                inject_failure(injector, "facets", artifact_index)?;
                artifact_index += 1;
            }
            "move" => {
                rollback.capture(journal, &directory)?;
                restore_snapshot(journal, &JournalSnapshot::Missing { path: directory })?;
                inject_failure(injector, "facets", artifact_index)?;
                artifact_index += 1;
            }
            "merge" => {
                let target_before = entry
                    .get("target_before")
                    .filter(|value| value.is_object())
                    .ok_or_else(|| {
                        EntityUndoError::Refused(
                            "merge payload facet entry is missing target_before".to_owned(),
                        )
                    })?;
                let path = format!("{directory}/entity.json");
                let destination = contained_path(journal, &path)
                    .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
                rollback.capture(journal, &path)?;
                write_json(
                    destination,
                    target_before,
                    JsonWriteOptions {
                        indent: Some(2),
                        sort_keys: false,
                        mode: None,
                    },
                )
                .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
                inject_failure(injector, "facets", artifact_index)?;
                artifact_index += 1;
                let observations_before = entry
                    .get("target_observations_before")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        EntityUndoError::Refused(
                            "merge payload facet entry is missing target_observations_before"
                                .to_owned(),
                        )
                    })?;
                let observations_existed = entry
                    .get("target_observations_existed")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| {
                        EntityUndoError::Refused(
                            "merge payload facet entry is missing target_observations_existed"
                                .to_owned(),
                        )
                    })?;
                let observations_path = format!("{directory}/observations.jsonl");
                rollback.capture(journal, &observations_path)?;
                if observations_existed {
                    let destination = contained_path(journal, &observations_path)
                        .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
                    write_jsonl(
                        destination,
                        observations_before.to_vec(),
                        AtomicWriteOptions::default(),
                    )
                    .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
                } else {
                    restore_snapshot(
                        journal,
                        &JournalSnapshot::Missing {
                            path: observations_path,
                        },
                    )?;
                }
                inject_failure(injector, "facets", artifact_index)?;
                artifact_index += 1;
            }
            _ => {
                return Err(EntityUndoError::Refused(format!(
                    "merge payload facet entry has unknown kind: {kind}"
                )));
            }
        }
    }
    Ok(())
}

fn undo_observation_relations(
    journal: &Path,
    payload: &Value,
    rollback: &mut MergeRollback,
    injector: Option<&FailureInjector>,
) -> Result<(), EntityUndoError> {
    let entries = payload
        .get("manifest")
        .and_then(Value::as_object)
        .and_then(|manifest| manifest.get("observation_relations"))
        .and_then(Value::as_object)
        .and_then(|relations| relations.get("entries"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            EntityUndoError::Refused(
                "merge payload observation relation entries are missing".to_owned(),
            )
        })?;
    for (artifact_index, entry) in entries.iter().enumerate() {
        let path = entry.get("path").and_then(Value::as_str).ok_or_else(|| {
            EntityUndoError::Refused(
                "merge payload observation relation entry is missing path".to_owned(),
            )
        })?;
        let row_index = entry
            .get("row_index")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| {
                EntityUndoError::Refused(
                    "merge payload observation relation entry is missing row_index".to_owned(),
                )
            })?;
        let target_before = entry
            .get("target_before")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                EntityUndoError::Refused(
                    "merge payload observation relation entry is missing target_before".to_owned(),
                )
            })?;
        let destination = contained_path(journal, path)
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        let mut rows: Vec<Value> = solstone_core_journal_io::read_jsonl(
            &destination,
            Vec::new(),
            solstone_core_journal_io::MalformedPolicy::Raise,
        )
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        let relation = rows
            .get_mut(row_index)
            .and_then(|row| row.get_mut("relation"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                EntityUndoError::Refused(format!(
                    "merge payload observation relation row is missing: {path}:{row_index}"
                ))
            })?;
        relation.insert(
            "target_entity_id".to_owned(),
            Value::String(target_before.to_owned()),
        );
        rollback.capture(journal, path)?;
        write_jsonl(destination, rows, AtomicWriteOptions::default())
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        inject_failure(injector, "observations", artifact_index)?;
    }
    Ok(())
}

fn undo_voiceprints(
    journal: &Path,
    target_dir: &str,
    payload: &Value,
    rollback: &mut MergeRollback,
) -> Result<(), EntityUndoError> {
    let snapshot = payload
        .get("manifest")
        .and_then(|manifest| manifest.get("voiceprints"))
        .and_then(|voiceprints| voiceprints.get("target_before"))
        .ok_or_else(|| {
            EntityUndoError::Refused("merge payload voiceprints missing target_before".to_owned())
        })?;
    let snapshot = snapshot_from_payload(snapshot)?;
    let path = format!("entities/{target_dir}/voiceprints.npz");
    if snapshot_path(&snapshot) != path {
        return Err(EntityUndoError::Refused(
            "merge payload voiceprints snapshot path does not match target".to_owned(),
        ));
    }
    rollback.capture(journal, &path)?;
    restore_snapshot(journal, &snapshot)?;
    Ok(())
}

fn undo_rebased_payloads(
    journal: &Path,
    current_dir: &str,
    restore_dir: &str,
    restore_id: &str,
    payload: &Value,
    rollback: &mut MergeRollback,
) -> Result<(), EntityUndoError> {
    let rebased = payload
        .get("manifest")
        .and_then(|manifest| manifest.get("rebased_merge_ids"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            EntityUndoError::Refused("merge payload missing rebased_merge_ids".to_owned())
        })?;
    for merge_id in rebased {
        let merge_id = merge_id.as_str().ok_or_else(|| {
            EntityUndoError::Refused("merge payload rebased merge id is not a string".to_owned())
        })?;
        rollback.capture(
            journal,
            &format!("entities/{current_dir}/history/private/{merge_id}.json"),
        )?;
        let (mut descendant, _) = move_entity_merge_payload(
            journal,
            current_dir,
            restore_dir,
            restore_id,
            merge_id,
            None,
        )?;
        descendant
            .as_object_mut()
            .expect("validated merge payload")
            .remove("rebased_from_entity_id");
        record_entity_merge_payload(journal, restore_dir, merge_id, &descendant)?;
    }
    Ok(())
}

fn undo_segments(
    journal: &Path,
    payload: &Value,
    rollback: &mut MergeRollback,
    injector: Option<&FailureInjector>,
) -> Result<(), EntityUndoError> {
    let entries = manifest_entries(payload, "segments")?;
    for (artifact_index, entry) in entries.iter().enumerate() {
        let path = entry_path(entry, "segment")?;
        let section = entry
            .get("section")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                EntityUndoError::Refused(
                    "merge payload segment entry is missing section".to_owned(),
                )
            })?;
        let row_index = entry_index(entry, "row_index", "segment")?;
        let field = entry.get("field").and_then(Value::as_str).ok_or_else(|| {
            EntityUndoError::Refused("merge payload segment entry is missing field".to_owned())
        })?;
        let before = entry.get("before").cloned().ok_or_else(|| {
            EntityUndoError::Refused("merge payload segment entry is missing before".to_owned())
        })?;
        let destination = contained_path(journal, path)
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        rollback.lock_file(&destination)?;
        let mut value: Value = read_json(&destination, Value::Null, MalformedPolicy::Raise)
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        let object = value
            .get_mut(section)
            .and_then(Value::as_array_mut)
            .and_then(|rows| rows.get_mut(row_index))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                EntityUndoError::Refused(format!(
                    "merge payload segment row is missing: {path}:{section}:{row_index}"
                ))
            })?;
        object.insert(field.to_owned(), before);
        rollback.capture(journal, path)?;
        write_json(
            destination,
            &value,
            JsonWriteOptions {
                indent: Some(2),
                sort_keys: false,
                mode: None,
            },
        )
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        inject_failure(injector, "segments", artifact_index)?;
    }
    Ok(())
}

fn undo_activities(
    journal: &Path,
    payload: &Value,
    rollback: &mut MergeRollback,
    injector: Option<&FailureInjector>,
) -> Result<(), EntityUndoError> {
    let entries = manifest_entries(payload, "activities")?;
    for (artifact_index, entry) in entries.iter().enumerate() {
        let path = entry_path(entry, "activity")?;
        let row_index = entry_index(entry, "row_index", "activity")?;
        let container = entry
            .get("container")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                EntityUndoError::Refused(
                    "merge payload activity entry is missing container".to_owned(),
                )
            })?;
        let item_index = entry_index(entry, "item_index", "activity")?;
        let before = entry.get("before").cloned().ok_or_else(|| {
            EntityUndoError::Refused("merge payload activity entry is missing before".to_owned())
        })?;
        let destination = contained_path(journal, path)
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        rollback.lock_file(&destination)?;
        let mut rows: Vec<Value> = read_jsonl(&destination, Vec::new(), MalformedPolicy::Raise)
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        let row = rows
            .get_mut(row_index)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                EntityUndoError::Refused(format!(
                    "merge payload activity row is missing: {path}:{row_index}"
                ))
            })?;
        if let Some(field) = entry.get("field").and_then(Value::as_str) {
            let object = row
                .get_mut(container)
                .and_then(Value::as_array_mut)
                .and_then(|items| items.get_mut(item_index))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| {
                    EntityUndoError::Refused(format!(
                        "merge payload activity field is missing: {path}:{row_index}:{container}:{item_index}"
                    ))
                })?;
            object.insert(field.to_owned(), before);
        } else {
            let value = row
                .get_mut(container)
                .and_then(Value::as_array_mut)
                .and_then(|items| items.get_mut(item_index))
                .ok_or_else(|| {
                    EntityUndoError::Refused(format!(
                        "merge payload activity item is missing: {path}:{row_index}:{container}:{item_index}"
                    ))
                })?;
            *value = before;
        }
        rollback.capture(journal, path)?;
        write_jsonl(destination, rows, AtomicWriteOptions::default())
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        inject_failure(injector, "activities", artifact_index)?;
    }
    Ok(())
}

fn manifest_entries<'a>(payload: &'a Value, section: &str) -> Result<&'a [Value], EntityUndoError> {
    payload
        .get("manifest")
        .and_then(Value::as_object)
        .and_then(|manifest| manifest.get(section))
        .and_then(Value::as_object)
        .and_then(|section| section.get("entries"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            EntityUndoError::Refused(format!("merge payload {section} entries are missing"))
        })
}

fn entry_path<'a>(entry: &'a Value, section: &str) -> Result<&'a str, EntityUndoError> {
    entry.get("path").and_then(Value::as_str).ok_or_else(|| {
        EntityUndoError::Refused(format!("merge payload {section} entry is missing path"))
    })
}

fn entry_index(entry: &Value, field: &str, section: &str) -> Result<usize, EntityUndoError> {
    entry
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| {
            EntityUndoError::Refused(format!("merge payload {section} entry is missing {field}"))
        })
}

fn find_payload_holder(journal: &Path, merge_id: &str) -> Result<String, EntityUndoError> {
    let entities = contained_path(journal, "entities")
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
    for entry in
        list_dir_entries(&entities).map_err(|error| EntityUndoError::Refused(error.to_string()))?
    {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let entity_dir = entry.name.to_string_lossy().into_owned();
        let path = contained_path(
            journal,
            &format!("entities/{entity_dir}/history/private/{merge_id}.json"),
        )
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        if path_lexists(&path).map_err(|error| EntityUndoError::Refused(error.to_string()))? {
            let identity = read_entity_identity(journal, &entity_dir)
                .map_err(|error| EntityUndoError::Refused(error.to_string()))?
                .ok_or_else(|| {
                    EntityUndoError::Refused(format!(
                        "entity identity missing for merge payload holder: {entity_dir}"
                    ))
                })?;
            return Ok(identity.entity_id().to_owned());
        }
    }
    Err(EntityUndoError::Refused(format!(
        "private merge payload not found: {merge_id}"
    )))
}

fn preflight_file_restoration(
    journal: &Path,
    target_dir: &str,
    payload: &Value,
    rollback: &mut MergeRollback,
) -> Result<(), EntityUndoError> {
    let mut paths =
        std::collections::BTreeSet::from([format!("entities/{target_dir}/voiceprints.npz")]);
    for section in ["segments", "activities", "observation_relations"] {
        for entry in manifest_entries(payload, section)? {
            paths.insert(entry_path(entry, section)?.to_owned());
        }
    }
    // Segment writers use per-file locks rather than entity-trust. Acquire
    // their complete set before reading any proof and keep them through undo.
    for path in &paths {
        if path.starts_with("chronicle/") {
            let destination = contained_path(journal, path)
                .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
            rollback.lock_file(&destination)?;
        }
    }
    for path in paths {
        let current = solstone_core_journal_io::capture_snapshot(journal, &path)?;
        if payload["manifest"]["undo_expected"][&path].is_null()
            && path == format!("entities/{target_dir}/voiceprints.npz")
        {
            let before =
                snapshot_from_payload(&payload["manifest"]["voiceprints"]["target_before"])?;
            // Older payloads can prove this one restoration is a no-op without
            // inventing an after-state for positional or merged-facet edits.
            if snapshot_path(&before) == path && current == before {
                continue;
            }
        }
        let expected = payload["manifest"]["undo_expected"][&path]
            .as_str()
            .ok_or_else(|| {
                EntityUndoError::Refused(format!(
                    "cannot safely undo merge: recorded target state is missing at {path}"
                ))
            })?;
        if super::merge_rollback::fingerprint(&current) != expected {
            return Err(EntityUndoError::Refused(format!(
                "cannot undo merge: target artifact changed at {path}"
            )));
        }
    }
    Ok(())
}

fn preflight_source_restoration(
    journal: &Path,
    target_id: &str,
    snapshots: Vec<JournalSnapshot>,
    payload: &Value,
) -> Result<Vec<JournalSnapshot>, EntityUndoError> {
    for entry in payload["manifest"]["facets"]["entries"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if entry["kind"] == "move" {
            return Err(EntityUndoError::Refused(
                "cannot safely undo legacy moved facet without recorded target state".to_owned(),
            ));
        }
        if entry["kind"] == "merge" {
            let facet = entry["facet"].as_str().ok_or_else(|| {
                EntityUndoError::Refused("merge payload facet is missing".to_owned())
            })?;
            let directory = entry["target_dir"].as_str().ok_or_else(|| {
                EntityUndoError::Refused("merge payload target directory is missing".to_owned())
            })?;
            for name in ["entity.json", "observations.jsonl"] {
                let relative = format!("facets/{facet}/entities/{directory}/{name}");
                let expected = entry["undo_expected"][name].as_str().ok_or_else(|| {
                    EntityUndoError::Refused(format!(
                        "cannot safely undo merge: recorded target state is missing at {relative}"
                    ))
                })?;
                let current = solstone_core_journal_io::capture_snapshot(journal, &relative)?;
                if super::merge_rollback::fingerprint(&current) != expected {
                    return Err(EntityUndoError::Refused(format!(
                        "cannot undo merge: target facet changed at {relative}"
                    )));
                }
            }
        }
    }
    let relinks: HashSet<String> = payload["manifest"]["facets"]["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["kind"] == "relink")
        .filter_map(|entry| {
            Some(format!(
                "facets/{}/entities/{}",
                entry["facet"].as_str()?,
                entry["target_dir"].as_str()?
            ))
        })
        .collect();
    let mut restore = Vec::new();
    for snapshot in snapshots {
        super::merge_rollback::validate_before_image(journal, &snapshot, None)?;
        let relative = snapshot_path(&snapshot);
        if !(relative.starts_with("entities/") || relative.starts_with("facets/")) {
            return Err(EntityUndoError::Refused(
                "invalid undo source snapshot path".to_owned(),
            ));
        }
        let path = contained_path(journal, &relative)
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        if relinks.contains(&relative) {
            // This directory now belongs to the target. Keep its newer memory;
            // the facet phase reverses only the recorded entity link.
            let link: Value = read_json(
                path.join("entity.json"),
                Value::Null,
                MalformedPolicy::Raise,
            )
            .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
            if link["entity_id"] != target_id {
                return Err(EntityUndoError::Refused(format!(
                    "cannot undo merge: facet link changed at {relative}"
                )));
            }
        } else {
            if path_lexists(&path).map_err(|error| EntityUndoError::Refused(error.to_string()))? {
                return Err(EntityUndoError::Refused(format!(
                    "cannot undo merge: source destination is occupied at {relative}"
                )));
            }
            restore.push(snapshot);
        }
    }
    Ok(restore)
}

fn source_state_snapshots(payload: &Value) -> Result<Vec<JournalSnapshot>, EntityUndoError> {
    payload
        .get("source_state")
        .and_then(Value::as_object)
        .and_then(|source_state| source_state.get("snapshots"))
        .and_then(Value::as_array)
        .ok_or_else(|| EntityUndoError::Refused("merge payload missing snapshots".to_owned()))?
        .iter()
        .map(|snapshot| {
            let relative = snapshot.get("rel").and_then(Value::as_str).ok_or_else(|| {
                EntityUndoError::Refused("manifest snapshot missing relative path".to_owned())
            })?;
            let image = snapshot.get("snapshot").ok_or_else(|| {
                EntityUndoError::Refused("merge payload snapshot is missing image".to_owned())
            })?;
            let image = snapshot_from_payload(image)?;
            if snapshot_path(&image) != relative {
                return Err(EntityUndoError::Refused(
                    "merge payload snapshot image path does not match relative path".to_owned(),
                ));
            }
            Ok(image)
        })
        .collect()
}

fn snapshot_path(snapshot: &JournalSnapshot) -> String {
    match snapshot {
        JournalSnapshot::Missing { path } => path.clone(),
        JournalSnapshot::File(SnapshotFile { path, .. }) => path.clone(),
        JournalSnapshot::Directory(SnapshotDirectory { path, .. }) => path.clone(),
    }
}
