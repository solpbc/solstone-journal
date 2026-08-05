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

use super::merge_payload::{
    MergePayloadError, list_entity_merge_payload_ids, load_entity_merge_payload,
    move_entity_merge_payload, record_entity_merge_payload, remove_entity_merge_payload,
    snapshot_from_payload,
};
use super::merge_rollback::MergeRollback;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    injector: Option<&dyn Fn(&str) -> bool>,
) -> Result<EntityUndoReport, EntityUndoError> {
    let target_id = find_payload_holder(journal, merge_id)?;
    let payload = load_entity_merge_payload(journal, &target_id, merge_id)?;
    let active_payloads = load_active_sibling_payloads(journal, &target_id, merge_id)?;
    let source_id = payload
        .get("source_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            EntityUndoError::Refused("merge payload missing source entity id".to_owned())
        })?
        .to_owned();
    let source_snapshots = source_state_snapshots(&payload)?;
    let report = EntityUndoReport {
        merge_id: merge_id.to_owned(),
        source_id: source_id.clone(),
        target_id: target_id.clone(),
    };
    let _trust = hold_entity_trust_lock(journal).map_err(|error| EntityUndoError::Failed {
        failed_phase: "trust_lock".to_owned(),
        report: Box::new(report.clone()),
        rollback_error: Some(error.to_string()),
    })?;
    let mut rollback = MergeRollback::default();
    let mut phase = "snapshot";
    let result: Result<(), EntityUndoError> = (|| {
        let mut paths = HashSet::from([
            format!("entities/{target_id}"),
            format!("entities/{source_id}"),
            "indexer".to_owned(),
        ]);
        paths.extend(source_snapshots.iter().map(snapshot_path));
        for path in paths {
            rollback.capture(journal, &path)?;
        }
        phase = "source_state";
        for snapshot in &source_snapshots {
            restore_snapshot(journal, snapshot)?;
        }
        phase = "voiceprints";
        undo_voiceprints(journal, &target_id, &payload, &mut rollback)?;
        phase = "segments";
        undo_segments(journal, &payload, &mut rollback)?;
        inject_failure(injector, phase)?;
        phase = "activities";
        undo_activities(journal, &payload, &mut rollback)?;
        inject_failure(injector, phase)?;
        phase = "observations";
        undo_observation_relations(journal, &payload, &mut rollback)?;
        inject_failure(injector, phase)?;
        phase = "facets";
        undo_facets(journal, &target_id, &payload, &mut rollback)?;
        inject_failure(injector, phase)?;
        phase = "lineage";
        undo_rebased_payloads(journal, &source_id, &target_id, &payload, &mut rollback)?;
        phase = "identity";
        let target_after_undo =
            replay_target_identity(journal, &target_id, &payload, &active_payloads)?;
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
        phase = "private_payload";
        remove_entity_merge_payload(journal, &target_id, merge_id)?;
        phase = "edges";
        solstone_core_indexer_store::merge::rebuild_edges_for_recorded_merge_undo(journal)
            .map_err(EntityUndoError::Index)?;
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
    Ok(report)
}

fn load_active_sibling_payloads(
    journal: &Path,
    target_id: &str,
    merge_id: &str,
) -> Result<Vec<Value>, EntityUndoError> {
    let mut payloads = Vec::new();
    for sibling_id in list_entity_merge_payload_ids(journal, target_id)? {
        if sibling_id == merge_id {
            continue;
        }
        let payload = load_entity_merge_payload(journal, target_id, &sibling_id).map_err(|error| {
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
    target_id: &str,
    payload: &Value,
    active_payloads: &[Value],
) -> Result<Value, EntityUndoError> {
    let mut current = read_entity_identity(journal, target_id)
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?
        .ok_or_else(|| EntityUndoError::Refused(format!("target entity not found: {target_id}")))?
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
        target_id,
        &mut current,
        "aka",
        identity.get("aka_support"),
        active_payloads,
        merge_seq,
    )?;
    remove_supported_set_values(
        journal,
        target_id,
        &mut current,
        "emails",
        identity.get("email_support"),
        active_payloads,
        merge_seq,
    )?;
    replay_supported_scalars(
        journal,
        target_id,
        &mut current,
        identity.get("scalar_support"),
        active_payloads,
        merge_seq,
    )?;
    Ok(current)
}

fn remove_supported_set_values(
    journal: &Path,
    target_id: &str,
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
            || later_owner_event_introduced(journal, target_id, field, key, merge_seq)?
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
    target_id: &str,
    current: &mut Value,
    support: Option<&Value>,
    active_payloads: &[Value],
    merge_seq: i128,
) -> Result<(), EntityUndoError> {
    let entries = support.and_then(Value::as_array).ok_or_else(|| {
        EntityUndoError::Refused("merge payload identity scalar_support is missing".to_owned())
    })?;
    let object = current
        .as_object_mut()
        .ok_or_else(|| EntityUndoError::Refused(format!("target entity not found: {target_id}")))?;
    for entry in entries {
        let key = entry.get("key").and_then(Value::as_str).ok_or_else(|| {
            EntityUndoError::Refused("merge payload scalar support entry is missing key".to_owned())
        })?;
        if later_owner_scalar_changed(journal, target_id, key, merge_seq)? {
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
    target_id: &str,
    field: &str,
    key: &str,
    merge_seq: i128,
) -> Result<bool, EntityUndoError> {
    for event in read_visible_history(journal, target_id)
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
    target_id: &str,
    key: &str,
    merge_seq: i128,
) -> Result<bool, EntityUndoError> {
    for event in read_visible_history(journal, target_id)
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
    injector: Option<&dyn Fn(&str) -> bool>,
    phase: &str,
) -> Result<(), EntityUndoError> {
    if injector.is_some_and(|injector| injector(phase)) {
        return Err(EntityUndoError::Refused(format!(
            "injected failure after phase {phase}"
        )));
    }
    Ok(())
}

fn undo_facets(
    journal: &Path,
    target_id: &str,
    payload: &Value,
    rollback: &mut MergeRollback,
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
        let directory = format!("facets/{facet}/entities/{target_id}");
        match kind {
            "move" => {
                rollback.capture(journal, &directory)?;
                restore_snapshot(journal, &JournalSnapshot::Missing { path: directory })?;
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
    for entry in entries {
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
    }
    Ok(())
}

fn undo_voiceprints(
    journal: &Path,
    target_id: &str,
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
    let path = format!("entities/{target_id}/voiceprints.npz");
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
    source_id: &str,
    target_id: &str,
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
            &format!("entities/{target_id}/history/private/{merge_id}.json"),
        )?;
        let (mut descendant, _) =
            move_entity_merge_payload(journal, target_id, source_id, merge_id, None)?;
        descendant
            .as_object_mut()
            .expect("validated merge payload")
            .remove("rebased_from_entity_id");
        record_entity_merge_payload(journal, source_id, merge_id, &descendant)?;
    }
    Ok(())
}

fn undo_segments(
    journal: &Path,
    payload: &Value,
    rollback: &mut MergeRollback,
) -> Result<(), EntityUndoError> {
    let entries = manifest_entries(payload, "segments")?;
    for entry in entries {
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
    }
    Ok(())
}

fn undo_activities(
    journal: &Path,
    payload: &Value,
    rollback: &mut MergeRollback,
) -> Result<(), EntityUndoError> {
    let entries = manifest_entries(payload, "activities")?;
    for entry in entries {
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
        let entity_id = entry.name.to_string_lossy().into_owned();
        let path = contained_path(
            journal,
            &format!("entities/{entity_id}/history/private/{merge_id}.json"),
        )
        .map_err(|error| EntityUndoError::Refused(error.to_string()))?;
        if path_lexists(&path).map_err(|error| EntityUndoError::Refused(error.to_string()))? {
            return Ok(entity_id);
        }
    }
    Err(EntityUndoError::Refused(format!(
        "private merge payload not found: {merge_id}"
    )))
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
