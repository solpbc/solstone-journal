// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The unfulfilled portion of a published import's scoped thinking request.
//! Fulfillment removes its priority/consent; it never certifies source coverage.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_format::content::strava_body::STRAVA_STREAM;
use solstone_core_journal_io::{JsonWriteOptions, LockOptions, hold_lock, write_json};

use crate::publish::{PUBLICATION_SCHEMA, PublicationRecord, SegmentBindingOutcome};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Coordinate {
    pub day: String,
    pub stream: String,
    pub segment: String,
}

#[derive(Debug, Clone)]
pub struct ImportThinking {
    pub id: String,
    pub publication: String,
    pub segments: BTreeSet<Coordinate>,
    pub days: BTreeSet<String>,
    day_scopes: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
pub struct ThinkingInventory {
    pub imports: Vec<ImportThinking>,
    pub unreadable: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Fulfilled {
    version: u32,
    publication: String,
    segments: BTreeSet<Coordinate>,
    days: BTreeMap<String, String>,
}

fn read_fulfilled(directory: &Path) -> Result<Fulfilled, String> {
    let bytes = match std::fs::read(directory.join("thinking.json")) {
        Ok(bytes) => bytes,
        Err(error) => return Err(error.to_string()),
    };
    let record: Fulfilled = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if record.version != 1 {
        return Err("unrecognized import thinking receipt".into());
    }
    Ok(record)
}

/// Called under the publication writer's shared lock, before imported.json exists.
/// Missing retirement on an existing publication must never manufacture consent.
pub(crate) fn prepare(directory: &Path) -> Result<(), String> {
    if directory
        .join("imported.json")
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        read_fulfilled(directory)?;
        return Ok(());
    }
    if directory
        .join("thinking.json")
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        read_fulfilled(directory)?;
        return Ok(());
    }
    write_json(
        directory.join("thinking.json"),
        &Fulfilled {
            version: 1,
            ..Fulfilled::default()
        },
        JsonWriteOptions::default(),
    )
    .map_err(|e| e.to_string())
}

fn inspect(journal: &Path, id: &str) -> Result<Option<ImportThinking>, String> {
    let directory = journal.join("imports").join(id);
    let bytes = match std::fs::read(directory.join("imported.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    // Other importers' records carry no publication schema and never asked for thinking.
    if value
        .as_object()
        .is_some_and(|record| !record.contains_key("schema"))
    {
        return Ok(None);
    }
    let record: PublicationRecord =
        serde_json::from_value(value).map_err(|error| error.to_string())?;
    if record.schema == crate::publish::LEGACY_PUBLICATION_SCHEMA {
        return Ok(None);
    }
    if record.schema != PUBLICATION_SCHEMA {
        return Err("unrecognized import publication".into());
    }
    let publication = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&record).map_err(|error| error.to_string())?)
    );
    let fulfilled = read_fulfilled(&directory)?;
    let mut segments = BTreeSet::new();
    let mut day_coordinates = BTreeMap::<String, BTreeSet<Coordinate>>::new();
    for segment in record.segments {
        if !matches!(segment.outcome, SegmentBindingOutcome::Bound { .. }) || segment.body_only {
            continue;
        }
        solstone_core_journal_io::segment_path(
            journal,
            &segment.day,
            &segment.segment,
            &segment.stream,
            false,
        )
        .map_err(|error| error.to_string())?;
        // A Strava tile never asks for thinking, whatever its directory holds,
        // as publication decides. One published before that rule was saved as
        // owing thinking; it owes none.
        if segment.stream == STRAVA_STREAM {
            continue;
        }
        if segment.day.len() != 8
            || chrono::NaiveDate::parse_from_str(&segment.day, "%Y%m%d").is_err()
        {
            return Err("invalid imported day".into());
        }
        let coordinate = Coordinate {
            day: segment.day,
            stream: segment.stream,
            segment: segment.segment,
        };
        day_coordinates
            .entry(coordinate.day.clone())
            .or_default()
            .insert(coordinate.clone());
        if !fulfilled.segments.contains(&coordinate) {
            segments.insert(coordinate);
        }
    }
    let day_scopes = day_coordinates
        .into_iter()
        .map(|(day, coordinates)| {
            let digest = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&coordinates).expect("coordinate serialization"))
            );
            (day, digest)
        })
        .collect::<BTreeMap<_, _>>();
    let days = day_scopes
        .iter()
        .filter(|(day, digest)| fulfilled.days.get(*day) != Some(*digest))
        .map(|(day, _)| day.clone())
        .collect();
    Ok(Some(ImportThinking {
        id: id.to_owned(),
        publication,
        segments,
        days,
        day_scopes,
    }))
}

pub fn read(journal: &Path) -> Result<ThinkingInventory, String> {
    let mut inventory = ThinkingInventory::default();
    let entries = match std::fs::read_dir(journal.join("imports")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(inventory),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            inventory
                .unreadable
                .push("non-UTF-8 import directory".into());
            continue;
        };
        match inspect(journal, &id) {
            Ok(Some(import)) if !import.segments.is_empty() || !import.days.is_empty() => {
                inventory.imports.push(import)
            }
            Ok(_) => {}
            Err(error) => inventory.unreadable.push(format!("{id}: {error}")),
        }
    }
    Ok(inventory)
}

pub fn fulfill(
    journal: &Path,
    import: &ImportThinking,
    segments: BTreeSet<Coordinate>,
    days: BTreeSet<String>,
) -> Result<(), String> {
    fulfill_checked(journal, import, segments, days, &|| Ok(()))
}

pub fn fulfill_checked(
    journal: &Path,
    import: &ImportThinking,
    segments: BTreeSet<Coordinate>,
    days: BTreeSet<String>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    if !segments.is_subset(&import.segments) || !days.is_subset(&import.days) {
        return Err("fulfillment outside import scope".into());
    }
    // Validate the id before constructing a write path, including public callers.
    if Path::new(&import.id).components().count() != 1
        || import.id == "."
        || import.id == ".."
        || import.id.contains(['/', '\\', ':'])
    {
        return Err("invalid import id".into());
    }
    let directory = journal.join("imports").join(&import.id);
    let _lock = hold_lock(directory.join("thinking.lock"), LockOptions::default())
        .map_err(|error| error.to_string())?;
    checkpoint()?;
    let Some(current) = inspect(journal, &import.id)? else {
        return Err("import publication disappeared".into());
    };
    if current.publication != import.publication {
        return Err("import publication changed".into());
    }
    let mut fulfilled = read_fulfilled(&directory)?;
    fulfilled.version = 1;
    fulfilled.publication = import.publication.clone();
    fulfilled.segments.extend(segments);
    for day in days {
        fulfilled
            .days
            .insert(day.clone(), import.day_scopes[&day].clone());
    }
    checkpoint()?;
    write_json(
        directory.join("thinking.json"),
        &fulfilled,
        JsonWriteOptions::default(),
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish::*;

    #[test]
    fn legacy_publications_remain_displayable_and_retry_without_fresh_consent() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("imports/legacy");
        std::fs::create_dir_all(&directory).unwrap();
        let mut record = PublicationRecord {
            schema: LEGACY_PUBLICATION_SCHEMA.into(),
            status: PublicationStatus::Success,
            segments: vec![],
            indexing: IndexPublicationOutcomes::default(),
            day_markers: vec![],
        };
        write_json(
            directory.join("imported.json"),
            &record,
            JsonWriteOptions::default(),
        )
        .unwrap();
        let inventory = read(root.path()).unwrap();
        assert!(inventory.unreadable.is_empty());
        assert!(inventory.imports.is_empty());
        assert!(crate::project_import_result(root.path(), "legacy").is_native());
        record.schema = PUBLICATION_SCHEMA.into();
        let published = write_publication_record(Some(&directory), &record).unwrap();
        assert_eq!(published.schema, LEGACY_PUBLICATION_SCHEMA);
        assert_eq!(
            read_publication_record(&directory).unwrap().unwrap().schema,
            LEGACY_PUBLICATION_SCHEMA
        );
        assert!(!directory.join("thinking.json").exists());
        assert!(read(root.path()).unwrap().unreadable.is_empty());
        assert!(read(root.path()).unwrap().imports.is_empty());
        record.schema = "unknown".into();
        write_json(
            directory.join("imported.json"),
            &record,
            JsonWriteOptions::default(),
        )
        .unwrap();
        assert_eq!(read(root.path()).unwrap().unreadable.len(), 1);
        // The shapes earlier importers wrote: no schema, segments as bare keys.
        for other in [
            serde_json::json!({"processed_timestamp":"20250922_084053","target_day":"20250922","outputs":[]}),
            serde_json::json!({"processed_timestamp":"20260429_112605","target_day":"20260429","source_type":"generic","segments":["112605_299","113104_300"]}),
        ] {
            std::fs::write(directory.join("imported.json"), other.to_string()).unwrap();
            let inventory = read(root.path()).unwrap();
            assert!(inventory.unreadable.is_empty());
            assert!(inventory.imports.is_empty());
        }
        std::fs::write(directory.join("imported.json"), "{").unwrap();
        assert_eq!(read(root.path()).unwrap().unreadable.len(), 1);
    }

    #[test]
    fn only_bound_scope_authorizes_work_and_fulfilled_scope_does_not_return() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("imports/test");
        std::fs::create_dir_all(&directory).unwrap();
        let mut record = PublicationRecord {
            schema: PUBLICATION_SCHEMA.into(),
            status: PublicationStatus::Failure,
            segments: vec![
                SegmentPublicationOutcome {
                    day: "20260101".into(),
                    stream: "import.notes".into(),
                    segment: "120000_60".into(),
                    outcome: SegmentBindingOutcome::Bound {
                        prev_day: None,
                        prev_segment: None,
                        seq: 1,
                    },
                    body_only: false,
                },
                SegmentPublicationOutcome {
                    day: "20260102".into(),
                    stream: "import.notes".into(),
                    segment: "120000_60".into(),
                    outcome: SegmentBindingOutcome::OwnerDeleted,
                    body_only: false,
                },
            ],
            indexing: IndexPublicationOutcomes::default(),
            day_markers: Vec::new(),
        };
        write_publication_record(Some(&directory), &record).unwrap();
        let import = read(root.path()).unwrap().imports.pop().unwrap();
        assert_eq!(import.segments.len(), 1);
        assert_eq!(import.days, BTreeSet::from(["20260101".into()]));
        fulfill(
            root.path(),
            &import,
            import.segments.clone(),
            import.days.clone(),
        )
        .unwrap();
        assert!(read(root.path()).unwrap().imports.is_empty());
        // Indexing/status-only changes cannot renew retired consent.
        record.status = PublicationStatus::Success;
        write_publication_record(Some(&directory), &record).unwrap();
        assert!(read(root.path()).unwrap().imports.is_empty());
        // A newly published coordinate grants only that coordinate, and its day.
        let mut added = record.segments[0].clone();
        added.segment = "130000_60".into();
        record.segments.push(added);
        write_publication_record(Some(&directory), &record).unwrap();
        assert!(
            fulfill(
                root.path(),
                &import,
                import.segments.clone(),
                import.days.clone()
            )
            .is_err()
        );
        assert_eq!(read(root.path()).unwrap().imports.len(), 1);
        let renewed = read(root.path()).unwrap().imports.pop().unwrap();
        assert_eq!(renewed.segments.len(), 1);
        assert_eq!(renewed.segments.first().unwrap().segment, "130000_60");
        std::fs::remove_file(directory.join("thinking.json")).unwrap();
        assert!(write_publication_record(Some(&directory), &record).is_err());
        std::fs::write(directory.join("thinking.json"), "{").unwrap();
        let inventory = read(root.path()).unwrap();
        assert!(inventory.imports.is_empty());
        assert_eq!(inventory.unreadable.len(), 1);
    }

    #[test]
    fn a_strava_tile_saved_as_owing_thinking_is_read_again_as_body_data() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("imports/strava-earlier");
        std::fs::create_dir_all(&directory).unwrap();
        let tile = |day: &str, files: &[&str]| {
            let dir = root
                .path()
                .join("chronicle")
                .join(day)
                .join("import.strava/070000_300");
            std::fs::create_dir_all(&dir).unwrap();
            for file in files {
                std::fs::write(dir.join(file), "{}").unwrap();
            }
            SegmentPublicationOutcome {
                day: day.into(),
                stream: "import.strava".into(),
                segment: "070000_300".into(),
                outcome: SegmentBindingOutcome::Bound {
                    prev_day: None,
                    prev_segment: None,
                    seq: 1,
                },
                // What a publication before this change saved for every tile.
                body_only: false,
            }
        };
        let mut record = PublicationRecord {
            schema: PUBLICATION_SCHEMA.into(),
            status: PublicationStatus::Success,
            segments: vec![tile("20261001", &["workout.json", "stream.json"])],
            indexing: IndexPublicationOutcomes::default(),
            day_markers: Vec::new(),
        };
        write_publication_record(Some(&directory), &record).unwrap();
        let inventory = read(root.path()).unwrap();
        assert!(inventory.unreadable.is_empty());
        assert!(inventory.imports.is_empty(), "neither held nor left to do");

        // A tile the body-only test rejects owes none either, as publication
        // asks none for it; any other stream still counts.
        record
            .segments
            .push(tile("20261002", &["workout.json", "notes.md"]));
        let mut notes = record.segments[0].clone();
        notes.stream = "import.notes".into();
        notes.day = "20261003".into();
        record.segments.push(notes);
        write_publication_record(Some(&directory), &record).unwrap();
        let import = read(root.path()).unwrap().imports.pop().unwrap();
        assert_eq!(
            import
                .segments
                .iter()
                .map(|c| (c.day.as_str(), c.stream.as_str()))
                .collect::<Vec<_>>(),
            vec![("20261003", "import.notes")]
        );
        assert_eq!(import.days, BTreeSet::from(["20261003".into()]));
    }
}
