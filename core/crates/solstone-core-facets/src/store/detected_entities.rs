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

pub fn read_detected_entities_strict(
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
        if matches!(mode, DetectedEntityReadMode::Strict) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("detected entity path is a directory: {}", path.display()),
            )
            .into());
        }
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
/// A row is one written name, up to case, compatibility forms and whitespace:
/// the same test `save_detected_entity` refuses a duplicate on. Two names that
/// only share a slug keep their own rows, so neither one's description replaces
/// the other's. The shared slug stays their `id`, which is how entity review
/// still weighs them together. A row whose stored id is not its name's slug
/// also matches a detection of that id, as a renamed row always has.
pub fn upsert_detection_segment(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    segment: &str,
    detections: &[DetectedEntityInput],
) -> Result<DetectionUpsertReport, FacetEntityWriteError> {
    let mut kept = Vec::new();
    for detection in detections {
        let key = normalize_resolution_query(&detection.name);
        if key.is_empty() {
            continue;
        }
        if let Some(index) = kept.iter().position(|(existing, _)| existing == &key) {
            kept[index] = (key, detection);
        } else {
            kept.push((key, detection));
        }
    }
    let mut wrote = 0;

    modify_detected_entities(journal_root, facet_dir, day, |rows| {
        // A recording that went to the trash while its detection ran writes
        // nothing: what it found left with it. Checked under the day's lock,
        // which the trash's narrowing takes after the recording has moved.
        if segment_removed(journal_root, segment) {
            return Ok(());
        }
        for row in rows.iter_mut() {
            let name = row.get("name").and_then(Value::as_str).unwrap_or_default();
            let key = normalize_resolution_query(name);
            let renamed_id = row
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| *id != entity_slug(name));
            let Some(index) = kept.iter().position(|(existing, detection)| {
                existing == &key || renamed_id == Some(entity_slug(&detection.name).as_str())
            }) else {
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
            object.insert(DESCRIBED_BY.to_owned(), Value::String(segment.to_owned()));
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

        for (_, detection) in kept.drain(..) {
            rows.push(json!({
                "id": entity_slug(&detection.name),
                "type": detection.entity_type,
                "name": detection.name,
                "description": detection.description,
                DESCRIBED_BY: segment,
                "segments": [segment],
                "updated_at": chrono::Utc::now().timestamp_millis(),
            }));
            wrote += 1;
        }
        Ok(())
    })?;

    Ok(DetectionUpsertReport { wrote })
}

/// The recording whose detection wrote a row's `description`, or
/// [`DESCRIBED_BY_OWNER`].
pub const DESCRIBED_BY: &str = "described_by";
/// A description the owner wrote, which no recording's delete takes.
pub const DESCRIBED_BY_OWNER: &str = "owner";

/// Whether the recording at chronicle-relative `origin` was removed by the
/// owner: its place is there and isn't a live recording. A place that isn't
/// there isn't a removal.
fn segment_removed(journal_root: &Path, origin: &str) -> bool {
    let Ok(dir) =
        solstone_core_journal_io::contained_path(journal_root, &format!("chronicle/{origin}"))
    else {
        return false;
    };
    dir.is_dir()
        && matches!(
            solstone_core_segment::owner_deletion_state(&dir),
            Ok(state) if state != solstone_core_segment::OwnerDeletionState::Live
        )
}

fn row_origins(row: &Value) -> Vec<String> {
    row.get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.as_str().or_else(|| {
                item.as_object()
                    .and_then(|object| object.get("segment"))
                    .and_then(Value::as_str)
            })
        })
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Take the recordings `gone` (chronicle-relative origins) out of one facet
/// and day's detections, for the trash: each row loses them, and a row left
/// with none goes. A row that keeps other recordings loses its description
/// too, unless a remaining one wrote it, since it may quote what left. Rows
/// the owner added (no recordings) are never touched. Returns each changed
/// row as it was, for a recover.
pub fn narrow_detections(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    gone: &std::collections::BTreeSet<String>,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    let path = detected_path(journal_root, facet_dir, day)?;
    if !path.is_file()
        || !read_detected_entities(journal_root, facet_dir, day)?
            .iter()
            .any(|row| row_origins(row).iter().any(|origin| gone.contains(origin)))
    {
        return Ok(Vec::new());
    }
    let mut before = Vec::new();
    let mut changed = false;
    let path = write_detected_entities(journal_root, facet_dir, day, |rows| {
        let mut next = Vec::with_capacity(rows.len());
        for row in rows.drain(..) {
            let origins = row_origins(&row);
            if !origins.iter().any(|origin| gone.contains(origin)) {
                next.push(row);
                continue;
            }
            before.push(row.clone());
            changed = true;
            let left: Vec<String> = origins
                .into_iter()
                .filter(|origin| !gone.contains(origin))
                .collect();
            if left.is_empty() {
                continue;
            }
            let mut row = row;
            let object = row
                .as_object_mut()
                .expect("detected reader returns objects");
            let described_by_left = object
                .get(DESCRIBED_BY)
                .and_then(Value::as_str)
                .is_some_and(|by| {
                    by == DESCRIBED_BY_OWNER || left.iter().any(|origin| origin == by)
                });
            if !described_by_left {
                object.insert("description".to_owned(), Value::String(String::new()));
                object.remove(DESCRIBED_BY);
            }
            object.insert(
                "segments".to_owned(),
                Value::Array(left.into_iter().map(Value::String).collect()),
            );
            next.push(row);
        }
        *rows = next;
        Ok(())
    })?;
    if changed {
        super::publication::publish_saved(journal_root, &path);
    }
    Ok(before)
}

/// Take the recordings the owner removed out of every day of `facet_dir`'s
/// detections, as [`narrow_detections`] does: for a facet coming back from
/// the trash, which a recording's delete couldn't reach while it waited.
pub fn prune_removed_detections(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<usize, FacetEntityWriteError> {
    let dir = solstone_core_journal_io::contained_path(
        journal_root,
        &format!("facets/{facet_dir}/entities"),
    )
    .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))?;
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut pruned = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(day) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".jsonl"))
            .filter(|day| day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()))
        else {
            continue;
        };
        let gone: std::collections::BTreeSet<String> =
            read_detected_entities(journal_root, facet_dir, day)?
                .iter()
                .flat_map(row_origins)
                .filter(|origin| segment_removed(journal_root, origin))
                .collect();
        if !gone.is_empty() {
            pruned += narrow_detections(journal_root, facet_dir, day, &gone)?.len();
        }
    }
    Ok(pruned)
}

/// One facet and day's detection rows naming any of `gone`, as they are.
pub fn detections_naming(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    gone: &std::collections::BTreeSet<String>,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    Ok(read_detected_entities(journal_root, facet_dir, day)?
        .into_iter()
        .filter(|row| row_origins(row).iter().any(|origin| gone.contains(origin)))
        .collect())
}

/// Put back what [`narrow_detections`] took, for a recover: each kept row's
/// recordings join the row of the same name on the day, or the row comes
/// back whole when there is none. What's on the day stands otherwise; an
/// empty description takes the kept one.
pub fn restore_detections(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    kept: &[Value],
) -> Result<(), FacetEntityWriteError> {
    if kept.is_empty() {
        return Ok(());
    }
    // Only the recordings in the journal now: a recover never brings back
    // what another delete took.
    let here = |origin: &String| !segment_removed(journal_root, origin);
    modify_detected_entities(journal_root, facet_dir, day, |rows| {
        for back in kept {
            let back_origins: Vec<String> = row_origins(back).into_iter().filter(here).collect();
            if back_origins.is_empty() {
                continue;
            }
            let key = normalize_resolution_query(
                back.get("name").and_then(Value::as_str).unwrap_or_default(),
            );
            let found = rows.iter_mut().find(|row| {
                normalize_resolution_query(
                    row.get("name").and_then(Value::as_str).unwrap_or_default(),
                ) == key
            });
            // A description comes back with the recording that wrote it; one
            // with no writer recorded, only when all its recordings do.
            let described_here = match back.get(DESCRIBED_BY).and_then(Value::as_str) {
                Some(by) => {
                    by == DESCRIBED_BY_OWNER || back_origins.iter().any(|origin| origin == by)
                }
                None => back_origins.len() == row_origins(back).len(),
            };
            let Some(row) = found else {
                let mut row = back.clone();
                let object = row.as_object_mut().expect("kept rows are objects");
                object.insert(
                    "segments".to_owned(),
                    Value::Array(back_origins.iter().cloned().map(Value::String).collect()),
                );
                if !described_here {
                    object.insert("description".to_owned(), Value::String(String::new()));
                    object.remove(DESCRIBED_BY);
                }
                rows.push(row);
                continue;
            };
            let mut origins: std::collections::BTreeSet<String> =
                row_origins(row).into_iter().collect();
            origins.extend(back_origins.iter().cloned());
            let object = row
                .as_object_mut()
                .expect("detected reader returns objects");
            object.insert(
                "segments".to_owned(),
                Value::Array(origins.into_iter().map(Value::String).collect()),
            );
            let blank = object
                .get("description")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty);
            if blank
                && described_here
                && let Some(description) = back.get("description")
            {
                object.insert("description".to_owned(), description.clone());
                match back.get(DESCRIBED_BY) {
                    Some(by) => object.insert(DESCRIBED_BY.to_owned(), by.clone()),
                    None => object.remove(DESCRIBED_BY),
                };
            }
        }
        Ok(())
    })
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
        let object = row.as_object_mut().expect("object rows");
        object.insert(
            "description".to_owned(),
            Value::String(description.to_owned()),
        );
        // The owner's words now: no recording's delete takes them.
        object.insert(
            DESCRIBED_BY.to_owned(),
            Value::String(DESCRIBED_BY_OWNER.to_owned()),
        );
        updated = Some(row.clone());
        Ok(())
    })?;
    Ok(updated.expect("update set result"))
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
    let path = write_detected_entities(journal_root, facet_dir, day, modify)?;
    super::publication::publish_saved(journal_root, &path);
    Ok(())
}

fn write_detected_entities<F>(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
    modify: F,
) -> Result<std::path::PathBuf, FacetEntityWriteError>
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
    })?;
    Ok(path)
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
