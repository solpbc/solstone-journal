// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only reference survey used before an irreversible entity deletion.

use std::path::Path;

use serde_json::Value;
use solstone_core_entity::{read_entity_identity, read_identity_group_map};
use solstone_core_journal_io::{
    DirEntryKind, MalformedPolicy, contained_path, list_dir_entries, read_json, read_jsonl,
};

use super::error::FacetStoreError;
use super::identity::read_facet_entity_link;
use super::map::list_facet_entity_directories;
use super::paths::{facet_entity_observations_path, facets_dir};

/// Fixed per-surface counts observed before an entity deletion starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityReferenceBreakdown {
    pub unrecognized_file: usize,
    pub facet_relationship: usize,
    pub observation: usize,
    pub activity: usize,
    pub segment_label: usize,
    pub segment_correction: usize,
    pub aka_crossref: usize,
    pub speaker_candidate: usize,
    pub keep_separate: usize,
    pub identify_operation: usize,
    pub ambiguity: usize,
    pub entity_review_candidate: usize,
    pub speaker_review_candidate: usize,
    pub candidate_pair: usize,
    pub dismissal: usize,
    pub unreadable: usize,
}

/// Survey every supported durable reference surface before mutating an entity.
pub(crate) fn scan_entity_references(
    journal_root: &Path,
    entity_id: &str,
    entity_dir: &str,
    operation_id: Option<&str>,
) -> Result<EntityReferenceBreakdown, FacetStoreError> {
    let mut breakdown = EntityReferenceBreakdown::default();
    count_unrecognized_files(journal_root, entity_dir, &mut breakdown)?;
    count_facet_relationships_and_observations(journal_root, entity_id, &mut breakdown)?;
    count_activities(journal_root, entity_id, &mut breakdown)?;
    count_segment_speakers(journal_root, entity_id, operation_id, &mut breakdown)?;
    count_aka_crossrefs(journal_root, entity_id, &mut breakdown)?;
    count_speaker_candidates(journal_root, entity_id, &mut breakdown)?;
    // Fully-restored identify operations are not folded here: faithfully proving
    // that state needs the speaker undo/checkpoint model, which has no Rust owner.
    // Count every matching row instead; this advisory, proceed-and-report survey
    // may overstate references but cannot hide one.
    count_jsonl_surface(
        journal_root,
        "speakers/keep-separate.jsonl",
        entity_id,
        &mut breakdown,
        |row, id| {
            row.get("entity_id_a") == Some(&Value::String(id.to_owned()))
                || row.get("entity_id_b") == Some(&Value::String(id.to_owned()))
        },
        |counts| &mut counts.keep_separate,
    )?;
    count_jsonl_surface(
        journal_root,
        "speakers/identify-operations.jsonl",
        entity_id,
        &mut breakdown,
        |row, id| {
            !operation_id.is_some_and(|operation_id| {
                row.get("operation_id").and_then(Value::as_str) == Some(operation_id)
            }) && identify_operation_references(row, id)
        },
        |counts| &mut counts.identify_operation,
    )?;
    count_jsonl_surface(
        journal_root,
        "entities/ambiguities.jsonl",
        entity_id,
        &mut breakdown,
        ambiguity_references,
        |counts| &mut counts.ambiguity,
    )?;
    count_jsonl_surface(
        journal_root,
        "entities/review-candidates.jsonl",
        entity_id,
        &mut breakdown,
        |row, id| {
            row.get("source_slug") == Some(&Value::String(id.to_owned()))
                || row.get("target_slug") == Some(&Value::String(id.to_owned()))
        },
        |counts| &mut counts.entity_review_candidate,
    )?;
    count_jsonl_surface(
        journal_root,
        "speakers/review-candidates.jsonl",
        entity_id,
        &mut breakdown,
        |row, id| {
            row.get("source_id") == Some(&Value::String(id.to_owned()))
                || row.get("target_id") == Some(&Value::String(id.to_owned()))
        },
        |counts| &mut counts.speaker_review_candidate,
    )?;
    // These stores normally retain pre-resolution capture-cluster coordinates,
    // not journal entity ids. Keep the generic scan for exceptional rows, with
    // dedicated synthetic coverage, but production writer output is rarely nonzero.
    count_jsonl_surface(
        journal_root,
        "speakers/candidate-pair-review-candidates.jsonl",
        entity_id,
        &mut breakdown,
        value_contains,
        |counts| &mut counts.candidate_pair,
    )?;
    count_jsonl_surface(
        journal_root,
        "speakers/cluster-dismissals.jsonl",
        entity_id,
        &mut breakdown,
        value_contains,
        |counts| &mut counts.dismissal,
    )?;
    if operation_id.is_some() && !solstone_core_indexer_store::db::is_index_readable(journal_root) {
        breakdown.unreadable += 1;
    }
    Ok(breakdown)
}

fn count_unrecognized_files(
    root: &Path,
    entity_dir: &str,
    counts: &mut EntityReferenceBreakdown,
) -> Result<(), FacetStoreError> {
    let directory = contained_path(root, &format!("entities/{entity_dir}"))?;
    for file in descendant_files(&directory)? {
        let allowed = file == directory.join("entity.json")
            || file
                .strip_prefix(directory.join("history/events"))
                .is_ok_and(|relative| relative.extension() == Some("json".as_ref()));
        if !allowed
            && !file
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".lock"))
        {
            counts.unrecognized_file += 1;
        }
    }
    Ok(())
}

fn count_facet_relationships_and_observations(
    root: &Path,
    entity_id: &str,
    counts: &mut EntityReferenceBreakdown,
) -> Result<(), FacetStoreError> {
    for entry in list_dir_entries(&facets_dir(root)?)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let facet = entry.name.to_string_lossy().into_owned();
        for entity_dir in list_facet_entity_directories(root, &facet)? {
            let Some(link) = read_facet_entity_link(root, &facet, &entity_dir)? else {
                continue;
            };
            if link.entity_id() == entity_id {
                counts.facet_relationship += 1;
            }
            let path = facet_entity_observations_path(root, &facet, &entity_dir)?;
            for row in jsonl_rows(&path, counts) {
                if key_value_contains(
                    &row,
                    entity_id,
                    &["entity_id", "target_entity_id", "source_entity_id"],
                ) {
                    counts.observation += 1;
                }
            }
            // The reference counts a directory whose NAME equals the entity id as
            // an observation reference, additively with the field scan above.
            // Under written identity that name is a label and the comparison is
            // coincidental -- but the ownership signal it approximates is already
            // carried correctly by the facet_relationship counter, which keys on
            // the link's stored id. Reproduced as-is: diverging here changes a
            // blocker count against the recorded corpus for no gain in safety.
            if entity_dir == entity_id {
                counts.observation += 1;
            }
        }
    }
    Ok(())
}

fn count_activities(
    root: &Path,
    entity_id: &str,
    counts: &mut EntityReferenceBreakdown,
) -> Result<(), FacetStoreError> {
    for entry in list_dir_entries(&facets_dir(root)?)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let activity_dir = entry.path.join("activities");
        for activity in list_dir_entries(&activity_dir)? {
            if activity.kind != DirEntryKind::File
                || activity.path.extension() != Some("jsonl".as_ref())
            {
                continue;
            }
            for row in jsonl_rows(&activity.path, counts) {
                if key_value_contains(
                    &row,
                    entity_id,
                    &[
                        "entity_id",
                        "active_entities",
                        "owner_entity_id",
                        "counterparty_entity_id",
                        "from_entity_id",
                        "to_entity_id",
                    ],
                ) {
                    counts.activity += 1;
                }
            }
        }
    }
    Ok(())
}

// Expected correction artifacts from fully-restored identify operations are counted
// conservatively. Reproducing Python's exclusion would require its speaker undo
// state machine; an inflated informational count is safer than an under-count.
fn count_segment_speakers(
    root: &Path,
    entity_id: &str,
    operation_id: Option<&str>,
    counts: &mut EntityReferenceBreakdown,
) -> Result<(), FacetStoreError> {
    let chronicle = contained_path(root, "chronicle")?;
    for day in list_dir_entries(&chronicle)? {
        if day.kind != DirEntryKind::Directory {
            continue;
        }
        for stream in list_dir_entries(&day.path)? {
            if stream.kind != DirEntryKind::Directory {
                continue;
            }
            for segment in list_dir_entries(&stream.path)? {
                if segment.kind != DirEntryKind::Directory {
                    continue;
                }
                let talents = segment.path.join("talents");
                count_segment_labels(&talents.join("speaker_labels.json"), entity_id, counts);
                count_segment_corrections(
                    &talents.join("speaker_corrections.json"),
                    entity_id,
                    operation_id,
                    counts,
                );
            }
        }
    }
    Ok(())
}

fn count_segment_labels(path: &Path, entity_id: &str, counts: &mut EntityReferenceBreakdown) {
    if let Some(value) = json_object(path, counts) {
        for label in value
            .get("labels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if label.get("speaker").and_then(Value::as_str) == Some(entity_id) {
                counts.segment_label += 1;
            }
        }
    }
}

fn count_segment_corrections(
    path: &Path,
    entity_id: &str,
    operation_id: Option<&str>,
    counts: &mut EntityReferenceBreakdown,
) {
    if let Some(value) = json_object(path, counts) {
        for correction in value
            .get("corrections")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if operation_id.is_some_and(|operation_id| {
                correction.get("operation_id").and_then(Value::as_str) == Some(operation_id)
            }) {
                continue;
            }
            if correction.get("original_speaker").and_then(Value::as_str) == Some(entity_id)
                || correction.get("corrected_speaker").and_then(Value::as_str) == Some(entity_id)
            {
                counts.segment_correction += 1;
            }
        }
    }
}

fn count_aka_crossrefs(
    root: &Path,
    entity_id: &str,
    counts: &mut EntityReferenceBreakdown,
) -> Result<(), FacetStoreError> {
    for entity_dir in read_identity_group_map(root)
        .map_err(entity_store_to_facet)?
        .groups
        .into_values()
        .flatten()
    {
        let Some(identity) =
            read_entity_identity(root, &entity_dir).map_err(entity_store_to_facet)?
        else {
            continue;
        };
        if identity.entity_id() != entity_id
            && identity
                .value()
                .get("aka")
                .and_then(Value::as_array)
                .is_some_and(|aka| aka.iter().any(|value| value.as_str() == Some(entity_id)))
        {
            counts.aka_crossref += 1;
        }
    }
    Ok(())
}

fn count_speaker_candidates(
    root: &Path,
    entity_id: &str,
    counts: &mut EntityReferenceBreakdown,
) -> Result<(), FacetStoreError> {
    let path = contained_path(root, "awareness/speaker_candidates.json")?;
    if let Some(value) = json_object(&path, counts) {
        for candidate in value
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if candidate.get("confirmed_entity").and_then(Value::as_str) == Some(entity_id) {
                counts.speaker_candidate += 1;
            }
        }
    }
    Ok(())
}

fn count_jsonl_surface(
    root: &Path,
    relative: &str,
    entity_id: &str,
    counts: &mut EntityReferenceBreakdown,
    predicate: impl Fn(&Value, &str) -> bool,
    category: impl Fn(&mut EntityReferenceBreakdown) -> &mut usize,
) -> Result<(), FacetStoreError> {
    let path = contained_path(root, relative)?;
    for row in jsonl_rows(&path, counts) {
        if predicate(&row, entity_id) {
            *category(counts) += 1;
        }
    }
    Ok(())
}

fn jsonl_rows(path: &Path, counts: &mut EntityReferenceBreakdown) -> Vec<Value> {
    match read_jsonl(path, Vec::new(), MalformedPolicy::Raise) {
        Ok(rows) => rows,
        Err(_) => {
            // The shared reader exposes only fail-fast and silent-skip policies, not
            // the number of skipped records. Keep parsing centralized there and
            // report one unreadable file rather than duplicate its JSONL parser here.
            counts.unreadable += 1;
            read_jsonl(path, Vec::new(), MalformedPolicy::Skip).unwrap_or_default()
        }
    }
}

fn json_object(path: &Path, counts: &mut EntityReferenceBreakdown) -> Option<Value> {
    match read_json(path, Value::Null, MalformedPolicy::Raise) {
        Ok(value) if value.is_object() => Some(value),
        Ok(_) => None,
        Err(_) => {
            counts.unreadable += 1;
            None
        }
    }
}

fn descendant_files(directory: &Path) -> Result<Vec<std::path::PathBuf>, FacetStoreError> {
    let mut files = Vec::new();
    for entry in list_dir_entries(directory)? {
        match entry.kind {
            DirEntryKind::File => files.push(entry.path),
            DirEntryKind::Directory => files.extend(descendant_files(&entry.path)?),
            DirEntryKind::Other => {}
        }
    }
    Ok(files)
}

fn key_value_contains(value: &Value, entity_id: &str, keys: &[&str]) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            (keys.contains(&key.as_str()) && direct_value_contains(child, entity_id))
                || key_value_contains(child, entity_id, keys)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| key_value_contains(value, entity_id, keys)),
        _ => false,
    }
}

fn direct_value_contains(value: &Value, entity_id: &str) -> bool {
    value.as_str() == Some(entity_id)
        || value
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(entity_id)))
}

fn value_contains(value: &Value, entity_id: &str) -> bool {
    value.as_str() == Some(entity_id)
        || value
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value_contains(value, entity_id)))
        || value.as_object().is_some_and(|values| {
            values
                .values()
                .any(|value| value_contains(value, entity_id))
        })
}

fn ambiguity_references(row: &Value, entity_id: &str) -> bool {
    row.get("resolved_entity_id").and_then(Value::as_str) == Some(entity_id)
        || row
            .get("ranked_candidates")
            .and_then(Value::as_array)
            .is_some_and(|candidates| {
                candidates
                    .iter()
                    .any(|candidate| candidate.get("id").and_then(Value::as_str) == Some(entity_id))
            })
}

fn identify_operation_references(row: &Value, entity_id: &str) -> bool {
    row.get("target_entity_id").and_then(Value::as_str) == Some(entity_id)
        || row
            .get("reviewed_near_match_entity_ids")
            .is_some_and(|value| direct_value_contains(value, entity_id))
        || row
            .get("prepared_plan")
            .and_then(|plan| plan.get("keep_separate_assertions"))
            .and_then(Value::as_array)
            .is_some_and(|assertions| {
                assertions.iter().any(|assertion| {
                    [
                        "entity_id_a",
                        "entity_id_b",
                        "planned_target_entity_id",
                        "reviewed_id",
                    ]
                    .iter()
                    .any(|key| assertion.get(*key).and_then(Value::as_str) == Some(entity_id))
                })
            })
}

fn entity_store_to_facet(error: solstone_core_entity::EntityStoreError) -> FacetStoreError {
    match error {
        solstone_core_entity::EntityStoreError::Read(error) => FacetStoreError::Read(error),
        solstone_core_entity::EntityStoreError::Path(error) => FacetStoreError::Path(error),
        _ => FacetStoreError::Path(solstone_core_journal_io::PathError::InvalidRelativePath {
            rel: "entities".to_owned(),
            message: "entity identity scan failed",
        }),
    }
}
