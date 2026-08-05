// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{DirEntryKind, MalformedPolicy, list_dir_entries, read_json};

use super::error::EntityStoreError;
use super::paths::{events_dir, prepared_dir};
use super::reconcile::python_json_equal;

/// One durable history event retaining the complete event object.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEvent {
    value: Value,
}

impl HistoryEvent {
    pub(super) fn from_value(value: Value, path: &Path) -> Result<Self, EntityStoreError> {
        if value.is_object() {
            Ok(Self { value })
        } else {
            Err(EntityStoreError::HistoryEventNotObject {
                path: path.to_path_buf(),
            })
        }
    }

    /// Complete durable event object.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Sequence value when a caller needs an integer guard.
    pub fn sequence(&self) -> Result<i128, EntityStoreError> {
        match self.value.get("seq") {
            Some(Value::Bool(value)) => Ok(i128::from(u8::from(*value))),
            Some(Value::Number(number)) => number
                .as_i64()
                .map(i128::from)
                .or_else(|| number.as_u64().map(i128::from))
                .ok_or(EntityStoreError::HistorySequenceNotInteger),
            _ => Err(EntityStoreError::HistorySequenceNotInteger),
        }
    }

    /// Version id when a caller needs a visible-event filename guard.
    pub fn version_id(&self) -> Result<&str, EntityStoreError> {
        let version_id = self.value.get("version_id").and_then(Value::as_str);
        match version_id.filter(|version_id| version_id.starts_with("vh_")) {
            Some(version_id) => Ok(version_id),
            None => Err(EntityStoreError::InvalidHistoryVersionId),
        }
    }

    fn kind(&self) -> Option<&str> {
        self.value.get("kind").and_then(Value::as_str)
    }

    fn visible_filename(&self) -> Result<String, EntityStoreError> {
        Ok(format!(
            "{:020}-{}.json",
            self.sequence()?,
            self.version_id()?
        ))
    }
}

/// One prepared event and its staging directory name.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedHistoryEvent {
    pub staging_id: String,
    pub event: HistoryEvent,
}

/// Read visible events in Python-compatible lexical filename order.
pub fn read_visible_history(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<Vec<HistoryEvent>, EntityStoreError> {
    let directory = events_dir(journal_root, entity_dir)?;
    list_dir_entries(&directory)?
        .into_iter()
        .filter(|entry| {
            entry.kind == DirEntryKind::File && entry.path.extension() == Some("json".as_ref())
        })
        .map(|entry| read_history_event(&entry.path))
        .collect()
}

/// Read prepared staging events in lexical staging-directory order.
pub fn read_prepared_history(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<Vec<PreparedHistoryEvent>, EntityStoreError> {
    let directory = prepared_dir(journal_root, entity_dir)?;
    list_dir_entries(&directory)?
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::Directory)
        .map(|entry| {
            let event = read_history_event(&entry.path.join("event.json"))?;
            Ok(PreparedHistoryEvent {
                staging_id: entry.name.to_string_lossy().into_owned(),
                event,
            })
        })
        .collect()
}

/// Refuse an unequal existing visible event for an otherwise identical target.
pub fn guard_visible_event_collision(
    entity_id: &str,
    event: &HistoryEvent,
    existing: Option<&HistoryEvent>,
) -> Result<(), EntityStoreError> {
    let filename = event.visible_filename()?;
    let Some(existing) = existing else {
        return Ok(());
    };
    if python_json_equal(existing.value(), event.value()) {
        return Ok(());
    }
    Err(EntityStoreError::VisibleEventCollision {
        entity_id: entity_id.to_owned(),
        filename,
    })
}

/// Refuse generic restores that target or cross a recorded merge.
pub fn guard_restore_does_not_cross_merge(
    target_event: &HistoryEvent,
    events: &[HistoryEvent],
) -> Result<(), EntityStoreError> {
    let target_sequence = target_event.sequence()?;
    if is_recorded_merge(target_event) {
        return Err(EntityStoreError::RestoreTargetsRecordedMerge);
    }
    for event in events {
        if event
            .sequence()
            .is_ok_and(|sequence| sequence > target_sequence)
            && is_recorded_merge(event)
        {
            return Err(EntityStoreError::RestoreCrossesRecordedMerge);
        }
    }
    Ok(())
}

fn read_history_event(path: &Path) -> Result<HistoryEvent, EntityStoreError> {
    let value: Value = read_json(path, Value::Null, MalformedPolicy::Raise)?;
    HistoryEvent::from_value(value, path)
}

fn is_recorded_merge(event: &HistoryEvent) -> bool {
    matches!(event.kind(), Some("merge" | "merge_undo"))
}
