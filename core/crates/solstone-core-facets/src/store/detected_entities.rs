// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Day-scoped detected-entity records.

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_entity_matching::{entity_slug, normalize_resolution_query};
use solstone_core_journal_io::{
    AtomicWriteOptions, contained_path, path_lexists, read_text, write_text,
};

use crate::hold_facet_trust_lock;

use super::error::FacetEntityWriteError;

/// Read detected records for one facet and day. Missing files are empty.
pub fn read_detected_entities(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    let path = detected_path(journal_root, facet_dir, day)?;
    if !path_lexists(&path).map_err(|error| FacetEntityWriteError::FacetStore(error.into()))? {
        return Ok(Vec::new());
    }
    let contents = read_text(&path, String::new())
        .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))?;
    Ok(contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(Value::is_object)
        .collect())
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
