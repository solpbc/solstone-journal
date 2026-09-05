// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Safe movement of facet-scoped entity directories.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_entity_matching::{entity_slug, normalize_resolution_query};
use solstone_core_journal_io::{AtomicWriteOptions, write_text};

use crate::hold_facet_trust_lock;

use super::error::{FacetEntityWriteError, FacetWriteError};
use super::facet_entities::list_scoped_facet_entities;
use super::identity::read_facet_entity_link;
use super::write::save_facet_entity_link;

/// Outcome of moving one facet-scoped entity directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetEntityMoveResult {
    pub entity_dir: String,
    pub moved_from: String,
    pub moved_to: String,
    pub merged: bool,
}

/// Resolve a facet-scoped entity directory from a written name.
///
/// The relationship directory is a label, not an identity: a name can change
/// after the link is written, and two names can compare equal while slugifying
/// apart. Deriving the directory from the name alone therefore fails to find
/// entities that exist, so the stored link identity is consulted first.
///
/// The derived form remains as a fallback because a facet directory can
/// legitimately have no resolvable identity yet -- a restored backup arrives
/// before the linking pass runs -- and that read compatibility is permanent.
fn resolve_entity_dir(
    journal_root: &Path,
    facet_dir: &str,
    entity_name: &str,
) -> Result<String, FacetEntityWriteError> {
    let wanted = normalize_resolution_query(entity_name);
    for entity in list_scoped_facet_entities(journal_root, facet_dir, true, true)? {
        let name = entity
            .identity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if normalize_resolution_query(name) == wanted {
            return Ok(entity.relationship_dir);
        }
    }
    Ok(entity_slug(entity_name))
}

/// Move or merge a facet-memory directory without dropping files.
pub fn move_facet_entity(
    journal_root: &Path,
    entity_name: &str,
    from_facet: &str,
    to_facet: &str,
    merge: bool,
) -> Result<FacetEntityMoveResult, FacetEntityWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let entity_dir = resolve_entity_dir(journal_root, from_facet, entity_name)?;
    let source = entity_root(journal_root, from_facet, &entity_dir)?;
    let destination = entity_root(journal_root, to_facet, &entity_dir)?;
    if !source.exists() {
        return Err(FacetEntityWriteError::EntityNotFound {
            entity_id: entity_name.to_owned(),
        });
    }
    if !destination.exists() {
        fs::create_dir_all(destination.parent().expect("entity directory parent"))?;
        fs::rename(&source, &destination)?;
        return Ok(FacetEntityMoveResult {
            entity_dir,
            moved_from: from_facet.to_owned(),
            moved_to: to_facet.to_owned(),
            merged: false,
        });
    }
    if !merge {
        return Err(FacetEntityWriteError::EntityExists {
            name: entity_name.to_owned(),
        });
    }

    let manifest = collect_files(&source)?;
    let source_link = read_facet_entity_link(journal_root, from_facet, &entity_dir)?;
    let destination_link = read_facet_entity_link(journal_root, to_facet, &entity_dir)?;
    let source_link_handled = source_link.is_some();
    preflight_move_conflicts(&manifest, &source, &destination, source_link_handled)?;
    if let (Some(source_link), Some(destination_link)) = (&source_link, &destination_link) {
        let relationship = reconcile_relationship(source_link.value(), destination_link.value())?;
        save_facet_entity_link(
            journal_root,
            to_facet,
            &entity_dir,
            destination_link.entity_id(),
            &relationship,
        )?;
    } else if let Some(source_link) = &source_link {
        let relationship = object_clone(source_link.value())?;
        save_facet_entity_link(
            journal_root,
            to_facet,
            &entity_dir,
            source_link.entity_id(),
            &relationship,
        )?;
    }
    for relative in manifest {
        if relative == Path::new("entity.json") && source_link_handled {
            continue;
        }
        let source_file = source.join(&relative);
        let destination_file = destination.join(&relative);
        if relative == Path::new("observations.jsonl") && destination_file.exists() {
            merge_observations(&source_file, &destination_file)?;
            continue;
        }
        if destination_file.exists() {
            if fs::read(&source_file)? != fs::read(&destination_file)? {
                return Err(FacetEntityWriteError::MoveConflict { path: relative });
            }
        } else {
            fs::create_dir_all(destination_file.parent().expect("file parent"))?;
            fs::copy(&source_file, &destination_file)?;
        }
    }
    // Every source file was either reconciled, copied, or byte-identically deduplicated.
    fs::remove_dir_all(&source)?;
    Ok(FacetEntityMoveResult {
        entity_dir,
        moved_from: from_facet.to_owned(),
        moved_to: to_facet.to_owned(),
        merged: true,
    })
}

fn preflight_move_conflicts(
    manifest: &[PathBuf],
    source: &Path,
    destination: &Path,
    source_link_handled: bool,
) -> Result<(), FacetEntityWriteError> {
    for relative in manifest {
        if relative == Path::new("entity.json") && source_link_handled {
            continue;
        }
        if relative == Path::new("observations.jsonl") {
            continue;
        }
        let destination_file = destination.join(relative);
        if destination_file.exists()
            && fs::read(source.join(relative))? != fs::read(&destination_file)?
        {
            return Err(FacetEntityWriteError::MoveConflict {
                path: relative.clone(),
            });
        }
    }
    Ok(())
}

fn merge_observations(source: &Path, destination: &Path) -> Result<(), FacetEntityWriteError> {
    let mut lines = fs::read_to_string(destination)?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut keys = lines
        .iter()
        .map(|line| observation_key(line))
        .collect::<BTreeSet<_>>();
    for line in fs::read_to_string(source)?.lines() {
        if keys.insert(observation_key(line)) {
            lines.push(line.to_owned());
        }
    }
    write_text(
        destination,
        &(lines.join("\n") + if lines.is_empty() { "" } else { "\n" }),
        AtomicWriteOptions::default(),
    )
    .map_err(FacetWriteError::ContentWrite)?;
    Ok(())
}

fn observation_key(line: &str) -> String {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| {
            Some(format!(
                "{}\u{1f}{}",
                value.get("content")?.as_str()?,
                value
                    .get("observed_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ))
        })
        .unwrap_or_else(|| line.to_owned())
}

fn reconcile_relationship(
    source: &Value,
    destination: &Value,
) -> Result<Map<String, Value>, FacetEntityWriteError> {
    let source = object_clone(source)?;
    let mut destination = object_clone(destination)?;
    for (key, value) in source {
        if key == "entity_id" || key == "detached" {
            continue;
        }
        match key.as_str() {
            "attached_at" => choose_earliest(&mut destination, &key, value),
            "updated_at" | "last_seen" => choose_latest(&mut destination, &key, value),
            _ if missing_value(destination.get(&key)) && !missing_value(Some(&value)) => {
                destination.insert(key, value);
            }
            _ => {}
        }
    }
    Ok(destination)
}

fn choose_earliest(destination: &mut Map<String, Value>, key: &str, value: Value) {
    if missing_value(Some(&value)) {
        return;
    }
    if destination.get(key).is_none_or(|current| {
        missing_value(Some(current))
            || timestamp_order(&value, current).is_some_and(|order| order.is_lt())
    }) {
        destination.insert(key.to_owned(), value);
    }
}
fn choose_latest(destination: &mut Map<String, Value>, key: &str, value: Value) {
    if missing_value(Some(&value)) {
        return;
    }
    if destination.get(key).is_none_or(|current| {
        missing_value(Some(current))
            || timestamp_order(&value, current).is_some_and(|order| order.is_gt())
    }) {
        destination.insert(key.to_owned(), value);
    }
}
fn timestamp_order(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    Some(left.as_str()?.cmp(right.as_str()?))
}
fn missing_value(value: Option<&Value>) -> bool {
    value.is_none_or(|value| {
        value.is_null()
            || value == ""
            || value == &Value::Array(Vec::new())
            || value == &Value::Object(Map::new())
    })
}
fn object_clone(value: &Value) -> Result<Map<String, Value>, FacetEntityWriteError> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| FacetEntityWriteError::EntityNotFound {
            entity_id: "relationship".to_owned(),
        })
}
fn entity_root(
    journal_root: &Path,
    facet: &str,
    entity_dir: &str,
) -> Result<PathBuf, FacetEntityWriteError> {
    solstone_core_journal_io::contained_path(
        journal_root,
        &format!("facets/{facet}/entities/{entity_dir}"),
    )
    .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))
}
fn collect_files(root: &Path) -> Result<Vec<PathBuf>, FacetEntityWriteError> {
    let mut files = Vec::new();
    collect_files_inner(root, root, &mut files)?;
    files.sort();
    Ok(files)
}
fn collect_files_inner(
    root: &Path,
    current: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), FacetEntityWriteError> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files_inner(root, &path, files)?;
        } else if entry.file_type()?.is_file() {
            files.push(path.strip_prefix(root).expect("descendant").to_owned());
        } else {
            return Err(FacetEntityWriteError::MoveConflict {
                path: path.strip_prefix(root).expect("descendant").to_owned(),
            });
        }
    }
    Ok(())
}
