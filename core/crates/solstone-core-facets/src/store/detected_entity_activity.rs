// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read models derived from day-scoped detected-entity records.

use std::collections::HashMap;
use std::path::Path;

use chrono::{Local, NaiveDate, TimeDelta};
use serde_json::{Map, Value};
#[cfg(all(test, feature = "full-tests"))]
use solstone_core_entity_matching::MatchTier;
use solstone_core_entity_matching::{
    EntityNameCandidate, EntityNameMatchOutcome, find_matching_entity_detailed,
};
use solstone_core_journal_io::{DirEntryKind, contained_path, list_dir_entries};

use super::detected_entities::{read_detected_entities, read_detected_entities_strict};
use super::error::{FacetEntityWriteError, FacetStoreError};
use super::facet_entities::list_scoped_facet_entities;
use super::relationship_scans::enrich_relationship_with_journal;

const FUZZY_THRESHOLD: f64 = 90.0;

/// Load recent detections, excluding names matched by attached facet entities.
pub fn load_detected_entities_recent(
    journal_root: &Path,
    facet_dir: &str,
    days: i64,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    let candidates = exclusion_candidates(journal_root, facet_dir)?;
    let cutoff = cutoff_day(Local::now().date_naive(), days);
    let mut exclusion_cache = HashMap::new();
    let mut detected = Vec::new();

    for day in detected_days(journal_root, facet_dir)?.into_iter().rev() {
        if day < cutoff {
            continue;
        }
        for entity in read_detected_entities(journal_root, facet_dir, &day)? {
            let name = entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let excluded = *exclusion_cache.entry(name.clone()).or_insert_with(|| {
                matches!(
                    find_matching_entity_detailed(&name, &candidates, FUZZY_THRESHOLD),
                    EntityNameMatchOutcome::Matched { .. }
                        | EntityNameMatchOutcome::Ambiguous { .. }
                )
            });
            if excluded {
                continue;
            }
            let entity_type = entity
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(existing) = detected.iter_mut().find(|existing: &&mut Value| {
                existing.get("type").and_then(Value::as_str) == Some(entity_type)
                    && existing.get("name").and_then(Value::as_str) == Some(name.as_str())
            }) {
                let count = existing
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                existing["count"] = Value::from(count + 1);
                continue;
            }
            detected.push(Value::Object(Map::from_iter([
                ("type".to_owned(), Value::String(entity_type.to_owned())),
                ("name".to_owned(), Value::String(name)),
                (
                    "description".to_owned(),
                    entity
                        .get("description")
                        .cloned()
                        .unwrap_or_else(|| Value::String(String::new())),
                ),
                ("count".to_owned(), Value::from(1)),
                ("last_seen".to_owned(), Value::String(day.clone())),
            ])));
        }
    }
    Ok(detected)
}

/// Return every detected name at or after `since`, preserving duplicate rows.
pub fn iter_detected_entity_names_since(
    journal_root: &Path,
    since: &str,
) -> Result<Vec<(String, String, String)>, FacetEntityWriteError> {
    iter_detected_entity_names_since_with_reader(journal_root, since, None, read_detected_entities)
}

/// Read one detected-day file strictly, returning names in source-row order.
pub fn read_detected_entity_names_strict(
    journal_root: &Path,
    facet_dir: &str,
    day: &str,
) -> Result<Vec<String>, FacetEntityWriteError> {
    read_detected_entities_strict(journal_root, facet_dir, day).map(|entities| {
        entities
            .into_iter()
            .map(|entity| {
                entity
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("strict detected reader requires a name")
                    .trim()
                    .to_owned()
            })
            .collect()
    })
}

/// Return every strictly-valid detected name at or after `since`, optionally within one facet.
pub fn iter_detected_entity_names_since_strict(
    journal_root: &Path,
    since: &str,
    facet: Option<&str>,
) -> Result<Vec<(String, String, String)>, FacetEntityWriteError> {
    iter_detected_entity_names_since_with_reader(
        journal_root,
        since,
        facet,
        read_detected_entities_strict,
    )
}

fn iter_detected_entity_names_since_with_reader(
    journal_root: &Path,
    since: &str,
    requested_facet: Option<&str>,
    reader: fn(&Path, &str, &str) -> Result<Vec<Value>, FacetEntityWriteError>,
) -> Result<Vec<(String, String, String)>, FacetEntityWriteError> {
    let mut files = Vec::new();
    let facet_dirs = if let Some(facet) = requested_facet {
        vec![facet.to_owned()]
    } else {
        let facets = contained_path(journal_root, "facets")
            .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))?;
        list_dir_entries(&facets)
            .map_err(FacetStoreError::from)?
            .into_iter()
            .filter(|facet| facet.kind == DirEntryKind::Directory)
            .map(|facet| facet.name.to_string_lossy().into_owned())
            .collect()
    };
    for facet_dir in facet_dirs {
        for day in detected_days(journal_root, &facet_dir)? {
            if day.as_str() >= since {
                files.push((facet_dir.clone(), day));
            }
        }
    }
    files.sort();

    let mut names = Vec::new();
    for (facet, day) in files {
        for entity in reader(journal_root, &facet, &day)? {
            let name = entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if !name.is_empty() {
                names.push((name.to_owned(), facet.clone(), day.clone()));
            }
        }
    }
    Ok(names)
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn exclusion_tier(
    journal_root: &Path,
    facet_dir: &str,
    detected_name: &str,
) -> Result<Option<MatchTier>, FacetEntityWriteError> {
    Ok(
        match find_matching_entity_detailed(
            detected_name,
            &exclusion_candidates(journal_root, facet_dir)?,
            FUZZY_THRESHOLD,
        ) {
            EntityNameMatchOutcome::Matched { tier, .. }
            | EntityNameMatchOutcome::Ambiguous { tier, .. } => Some(tier),
            EntityNameMatchOutcome::NoMatch => None,
        },
    )
}

pub(crate) fn cutoff_day(local_today: NaiveDate, days: i64) -> String {
    (local_today - TimeDelta::days(days))
        .format("%Y%m%d")
        .to_string()
}

fn exclusion_candidates(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<Vec<EntityNameCandidate>, FacetEntityWriteError> {
    let mut candidates = Vec::new();
    for entity in list_scoped_facet_entities(journal_root, facet_dir, false, false)? {
        let enriched =
            enrich_relationship_with_journal(&entity.relationship, Some(&entity.identity));
        let Some(name) = enriched
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        candidates.push(EntityNameCandidate {
            id: enriched
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or(Some(entity.entity_id)),
            name: name.to_owned(),
            aka: string_values(enriched.get("aka")),
            emails: string_values(enriched.get("emails")),
        });
    }
    Ok(candidates)
}

pub(crate) fn detected_days(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<Vec<String>, FacetEntityWriteError> {
    let directory = contained_path(journal_root, &format!("facets/{facet_dir}/entities"))
        .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))?;
    let mut days: Vec<_> = list_dir_entries(&directory)
        .map_err(FacetStoreError::from)?
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .filter_map(|entry| {
            let stem = entry.path.file_stem()?.to_str()?;
            (entry
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("jsonl")
                && stem.len() == 8
                && stem.chars().all(|character| character.is_ascii_digit()))
            .then(|| stem.to_owned())
        })
        .collect();
    days.sort();
    Ok(days)
}

fn string_values(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
