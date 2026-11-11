// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An owner correction holds for the whole stretch it was made on.
//!
//! One stream's segments can belong to an activity in each of several facets.
//! Those activities stay apart (none is merged or hidden), and the owner's
//! account or recording mark made on one stands on every other activity of
//! the same stream that shares a segment with it, as an owner edit of its own.
//! Every edit of one correction names the same stretch id, so removing it from
//! any of them removes it from all.

use std::path::Path;

use serde_json::{Map, Value};

use super::activity_records::{
    ActivityRecord, ActivityRecordStoreError, EDIT_STRETCH, STRETCH_ID, STRETCH_SEGMENTS,
    edit_removed, is_body_record, load_activity_records, remove_activity_edit,
    update_activity_record_noting,
};

/// The fields of an owner correction that hold for the whole stretch. A
/// title or description stays the activity's own.
pub const STRETCH_FIELDS: [&str; 2] = ["details", "playback"];

pub use super::activity_records::correction_segments;

const OWNER_UPDATE: &str = "owner:update";

/// One activity a correction reached, or could not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StretchCopy {
    pub facet: String,
    pub id: String,
}

/// What saving a correction over its stretch wrote.
#[derive(Debug)]
pub struct StretchCorrection {
    /// The activity the owner corrected, as saved. `None` when it is missing.
    pub record: Option<ActivityRecord>,
    /// Every other activity over the stretch that now carries it.
    pub copies: Vec<StretchCopy>,
    /// Activities over the stretch it could not be saved on.
    pub failed: Vec<StretchCopy>,
}

fn id(record: &ActivityRecord) -> &str {
    record.get("id").and_then(Value::as_str).unwrap_or_default()
}

fn stream(record: &ActivityRecord) -> Option<&str> {
    record.get("stream").and_then(Value::as_str)
}

fn segments(record: &ActivityRecord) -> Vec<&str> {
    record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// Every other activity on `day`, in any declared facet, of the same stream as
/// `record` and sharing a segment with it, with the shared segments in that
/// activity's order. Hidden activities count: a correction is the owner's
/// word about the stretch, and showing one again keeps it.
pub fn activities_over_stretch(
    root: &Path,
    day: &str,
    facet: &str,
    record: &ActivityRecord,
) -> Vec<(String, ActivityRecord, Vec<String>)> {
    let own = segments(record);
    if own.is_empty() || is_body_record(record) {
        return Vec::new();
    }
    let facets = match super::map::list_declared_facet_names(root) {
        Ok(facets) => facets,
        Err(error) => {
            log::warn!("stretch: facets not listed: {error}");
            return Vec::new();
        }
    };
    let mut found = Vec::new();
    for other_facet in facets {
        let rows = match load_activity_records(root, &other_facet, day, true) {
            Ok(rows) => rows,
            Err(ActivityRecordStoreError::MissingDayFile { .. }) => continue,
            Err(error) => {
                log::warn!("stretch: activities of {other_facet} on {day} not read: {error}");
                continue;
            }
        };
        for row in rows {
            if (other_facet == facet && id(&row) == id(record))
                || is_body_record(&row)
                || stream(&row) != stream(record)
            {
                continue;
            }
            let shared = segments(&row)
                .into_iter()
                .filter(|segment| own.contains(segment))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if !shared.is_empty() {
                found.push((other_facet.clone(), row, shared));
            }
        }
    }
    found
}

/// A new stretch id: the corrected activity and the moment, to the nanosecond.
fn new_stretch_id(facet: &str, record_id: &str, timestamp: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{facet}/{record_id}/{timestamp}/{nanos:x}")
}

/// Save the owner's correction `patch` on one activity, and its account and
/// recording mark on every other activity over the same stretch. Each copy
/// records which of its segments the corrected activity covers.
#[allow(clippy::too_many_arguments)] // The correction and where it was made.
pub fn correct_over_stretch(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    patch: &Map<String, Value>,
    note: &str,
    timestamp: &str,
) -> Result<StretchCorrection, ActivityRecordStoreError> {
    let shared = patch
        .iter()
        .filter(|(key, _)| STRETCH_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    let stretch_id = new_stretch_id(facet, record_id, timestamp);
    let mut mark = Map::new();
    if !shared.is_empty() {
        mark.insert(
            EDIT_STRETCH.to_owned(),
            Value::Object(Map::from_iter([(
                STRETCH_ID.to_owned(),
                Value::String(stretch_id.clone()),
            )])),
        );
    }
    let record = update_activity_record_noting(
        root,
        facet,
        day,
        record_id,
        patch,
        OWNER_UPDATE,
        note,
        timestamp,
        &mark,
    )?;
    let mut result = StretchCorrection {
        record,
        copies: Vec::new(),
        failed: Vec::new(),
    };
    let Some(corrected) = result.record.as_ref().filter(|_| !shared.is_empty()) else {
        return Ok(result);
    };
    for (other_facet, other, covered) in activities_over_stretch(root, day, facet, corrected) {
        let copy = StretchCopy {
            facet: other_facet.clone(),
            id: id(&other).to_owned(),
        };
        let mark = Map::from_iter([(
            EDIT_STRETCH.to_owned(),
            serde_json::json!({ STRETCH_ID: stretch_id, STRETCH_SEGMENTS: covered }),
        )]);
        match update_activity_record_noting(
            root,
            &other_facet,
            day,
            &copy.id,
            &shared,
            OWNER_UPDATE,
            note,
            timestamp,
            &mark,
        ) {
            Ok(Some(_)) => result.copies.push(copy),
            Ok(None) => {}
            Err(error) => {
                log::warn!(
                    "stretch: correction not saved on {}/{}: {error}",
                    copy.facet,
                    copy.id
                );
                result.failed.push(copy);
            }
        }
    }
    Ok(result)
}

/// The stretch id an edit names, when it is one copy of a stretch correction.
pub fn edit_stretch_id(edit: &Map<String, Value>) -> Option<&str> {
    edit.get(EDIT_STRETCH)?.get(STRETCH_ID)?.as_str()
}

/// Remove, from every activity on `day` other than `facet`/`record_id`, the
/// standing edit that names `stretch_id`. Returns the activities it was
/// removed from, and those it could not be removed from.
pub fn remove_over_stretch(
    root: &Path,
    day: &str,
    facet: &str,
    record_id: &str,
    stretch_id: &str,
    removed_at: &str,
) -> (Vec<StretchCopy>, Vec<StretchCopy>) {
    let (mut removed, mut failed) = (Vec::new(), Vec::new());
    let facets = match super::map::list_declared_facet_names(root) {
        Ok(facets) => facets,
        Err(error) => {
            log::warn!("stretch: facets not listed: {error}");
            return (removed, failed);
        }
    };
    for other_facet in facets {
        let Ok(rows) = load_activity_records(root, &other_facet, day, true) else {
            continue;
        };
        for row in rows {
            if other_facet == facet && id(&row) == record_id {
                continue;
            }
            let edits = row
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .enumerate()
                .filter(|(_, edit)| {
                    !edit_removed(edit) && edit_stretch_id(edit) == Some(stretch_id)
                })
                .map(|(position, edit)| {
                    (
                        position,
                        edit.get("timestamp")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        edit.get("actor")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    )
                })
                .collect::<Vec<_>>();
            let copy = StretchCopy {
                facet: other_facet.clone(),
                id: id(&row).to_owned(),
            };
            let mut ok = !edits.is_empty();
            for (position, timestamp, actor) in edits {
                if let Err(error) = remove_activity_edit(
                    root,
                    &other_facet,
                    day,
                    &copy.id,
                    position,
                    &timestamp,
                    &actor,
                    removed_at,
                ) {
                    log::warn!(
                        "stretch: correction not removed from {}/{}: {error}",
                        copy.facet,
                        copy.id
                    );
                    ok = false;
                    failed.push(copy.clone());
                }
            }
            if ok {
                removed.push(copy);
            }
        }
    }
    (removed, failed)
}
