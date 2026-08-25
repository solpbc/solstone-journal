// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Day-scoped detected-entity records.

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_entity::is_valid_entity_type;
use solstone_core_entity_matching::{entity_slug, normalize_resolution_query};
use solstone_core_journal_io::{
    AtomicWriteOptions, contained_path, path_lexists, read_text, write_text,
};

use crate::hold_facet_trust_lock;

use super::error::FacetEntityWriteError;

/// One kept detection supplied by segment processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedEntityInput {
    pub entity_type: String,
    pub name: String,
    pub description: String,
}

/// Durable rows changed by one segment reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectionUpsertReport {
    pub wrote: usize,
}

/// Read detected records for one facet and day.
///
/// Missing files and directories at the day-file path are empty. Rows with an invalid type are
/// ignored, and missing or non-string IDs are filled from the row name.
pub fn read_detected_entities(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    read_detected_entities_with_mode(
        journal_root,
        facet_dir,
        day,
        DetectedEntityReadMode::Tolerant,
    )
}

pub(super) fn read_detected_entities_strict(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    read_detected_entities_with_mode(journal_root, facet_dir, day, DetectedEntityReadMode::Strict)
}

#[derive(Clone, Copy)]
enum DetectedEntityReadMode {
    Tolerant,
    Strict,
}

fn read_detected_entities_with_mode(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    mode: DetectedEntityReadMode,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    let path = detected_path(journal_root, facet_dir, day)?;
    if !path_lexists(&path).map_err(|error| FacetEntityWriteError::FacetStore(error.into()))? {
        return Ok(Vec::new());
    }
    if path.is_dir() {
        return Ok(Vec::new());
    }
    let contents = read_text(&path, String::new())
        .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))?;
    parse_detected_entity_rows(&contents, mode)
}

fn parse_detected_entity_rows(
    contents: &str,
    mode: DetectedEntityReadMode,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    let mut rows = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed = match serde_json::from_str::<Value>(line) {
            Ok(parsed) => parsed,
            Err(_) if matches!(mode, DetectedEntityReadMode::Tolerant) => continue,
            Err(error) => return Err(strict_row_error(index + 1, "invalid JSON", error)),
        };
        let Some(object) = parsed.as_object() else {
            if matches!(mode, DetectedEntityReadMode::Tolerant) {
                continue;
            }
            return Err(strict_row_error(
                index + 1,
                "record is not an object",
                std::io::Error::new(std::io::ErrorKind::InvalidData, "not an object"),
            ));
        };
        let mut row = Value::Object(object.clone());
        let object = row
            .as_object_mut()
            .expect("cloned object remains an object");
        let entity_type = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !is_valid_entity_type(entity_type) {
            if matches!(mode, DetectedEntityReadMode::Tolerant) {
                continue;
            }
            return Err(strict_row_error(
                index + 1,
                "record has an invalid entity type",
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid entity type"),
            ));
        }
        let name = object.get("name").and_then(Value::as_str);
        if matches!(mode, DetectedEntityReadMode::Strict)
            && !name.is_some_and(|value| !value.trim().is_empty())
        {
            return Err(strict_row_error(
                index + 1,
                "record has a missing or blank name",
                std::io::Error::new(std::io::ErrorKind::InvalidData, "missing or blank name"),
            ));
        }
        if !object.get("id").is_some_and(Value::is_string) {
            object.insert(
                "id".to_owned(),
                Value::String(entity_slug(name.unwrap_or_default())),
            );
        }
        rows.push(row);
    }
    Ok(rows)
}

fn strict_row_error(
    line_number: usize,
    reason: &str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> FacetEntityWriteError {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("detected entity row {line_number} {reason}: {source}"),
    )
    .into()
}

/// Reconcile one segment's kept detections into the facet/day detected store.
///
/// Existing rows match their stored ID first, falling back to a slug from their current name only
/// when the ID is absent.
pub fn upsert_detection_segment(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    segment: &str,
    detections: &[DetectedEntityInput],
) -> Result<DetectionUpsertReport, FacetEntityWriteError> {
    let mut kept = Vec::new();
    for detection in detections {
        let slug = entity_slug(&detection.name);
        if let Some(index) = kept.iter().position(|(key, _)| key == &slug) {
            kept[index] = (slug, detection);
        } else {
            kept.push((slug, detection));
        }
    }
    let mut wrote = 0;

    modify_detected_entities(journal_root, facet_dir, day, |rows| {
        for row in rows.iter_mut() {
            let slug = row
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    entity_slug(row.get("name").and_then(Value::as_str).unwrap_or_default())
                });
            let Some(index) = kept.iter().position(|(key, _)| key == &slug) else {
                continue;
            };
            let (_, detection) = kept.remove(index);
            let object = row
                .as_object_mut()
                .expect("detected reader returns objects");
            object.insert(
                "type".to_owned(),
                Value::String(detection.entity_type.clone()),
            );
            object.insert("name".to_owned(), Value::String(detection.name.clone()));
            object.insert(
                "description".to_owned(),
                Value::String(detection.description.clone()),
            );
            object.insert(
                "segments".to_owned(),
                Value::Array(
                    normalized_detection_segments(object.get("segments"), segment)
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            );
            object.insert(
                "updated_at".to_owned(),
                Value::Number(chrono::Utc::now().timestamp_millis().into()),
            );
            wrote += 1;
        }

        for (slug, detection) in kept.drain(..) {
            rows.push(json!({
                "id": slug,
                "type": detection.entity_type,
                "name": detection.name,
                "description": detection.description,
                "segments": [segment],
                "updated_at": chrono::Utc::now().timestamp_millis(),
            }));
            wrote += 1;
        }
        Ok(())
    })?;

    Ok(DetectionUpsertReport { wrote })
}

/// Save one detection, refusing a unified-normalized name duplicate.
pub fn save_detected_entity(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    entity_type: &str,
    name: &str,
    description: &str,
) -> Result<Value, FacetEntityWriteError> {
    let query = normalize_resolution_query(name);
    let entity = json!({"id": entity_slug(name), "type": entity_type, "name": name, "description": description});
    modify_detected_entities(journal_root, facet_dir, day, |rows| {
        if rows.iter().any(|row| {
            normalize_resolution_query(row.get("name").and_then(Value::as_str).unwrap_or_default())
                == query
        }) {
            return Err(FacetEntityWriteError::EntityExists {
                name: name.to_owned(),
            });
        }
        rows.push(entity.clone());
        Ok(())
    })?;
    Ok(entity)
}

/// Update exactly one detection selected by raw display-name equality.
pub fn update_detected_entity(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    name: &str,
    description: &str,
) -> Result<Value, FacetEntityWriteError> {
    let mut updated = None;
    modify_detected_entities(journal_root, facet_dir, day, |rows| {
        let row = rows
            .iter_mut()
            .find(|row| row.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| FacetEntityWriteError::EntityNotFound {
                entity_id: name.to_owned(),
            })?;
        row.as_object_mut().expect("object rows").insert(
            "description".to_owned(),
            Value::String(description.to_owned()),
        );
        updated = Some(row.clone());
        Ok(())
    })?;
    Ok(updated.expect("update set result"))
}

/// Delete all detections selected by raw display-name equality.
pub fn delete_detected_entity(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    name: &str,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    let mut removed = Vec::new();
    modify_detected_entities(journal_root, facet_dir, day, |rows| {
        let mut retained = Vec::with_capacity(rows.len());
        for row in rows.drain(..) {
            if row.get("name").and_then(Value::as_str) == Some(name) {
                removed.push(row);
            } else {
                retained.push(row);
            }
        }
        *rows = retained;
        Ok(())
    })?;
    Ok(removed)
}

fn modify_detected_entities<F>(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    modify: F,
) -> Result<(), FacetEntityWriteError>
where
    F: FnOnce(&mut Vec<Value>) -> Result<(), FacetEntityWriteError>,
{
    let _trust = hold_facet_trust_lock(journal_root)?;
    let mut rows = read_detected_entities(journal_root, facet_dir, day)?;
    modify(&mut rows)?;
    for row in &mut rows {
        let object = row
            .as_object_mut()
            .expect("detected reader returns objects");
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        object.insert("id".to_owned(), Value::String(entity_slug(name)));
    }
    rows.sort_by(|left, right| {
        left.get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .then_with(|| {
                left.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .cmp(
                        right
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
            })
    });
    let contents = rows
        .iter()
        .map(|row| serde_json::to_string(row).expect("value serializes") + "\n")
        .collect::<String>();
    let path = detected_path(journal_root, facet_dir, day)?;
    write_text(&path, &contents, AtomicWriteOptions { mode: Some(0o600) }).map_err(|error| {
        FacetEntityWriteError::FacetWrite(super::error::FacetWriteError::ContentWrite(error))
    })
}

fn detected_path(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
) -> Result<std::path::PathBuf, FacetEntityWriteError> {
    contained_path(
        journal_root,
        &format!("facets/{facet_dir}/entities/{day}.jsonl"),
    )
    .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))
}

fn normalized_detection_segments(existing: Option<&Value>, segment: &str) -> Vec<String> {
    let mut segments = std::collections::BTreeSet::from([segment.to_owned()]);
    if let Some(items) = existing.and_then(Value::as_array) {
        for item in items {
            let value = item.as_str().or_else(|| {
                item.as_object()
                    .and_then(|object| object.get("segment"))
                    .and_then(Value::as_str)
            });
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                segments.insert(value.to_owned());
            }
        }
    }
    segments.into_iter().collect()
}
