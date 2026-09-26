// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;

use super::declaration::{facet_write_identity, read_facet_declaration};
use super::error::{FacetIdError, FacetIdResolveError};
use super::map::list_declared_facet_names;
use super::write::save_facet_declaration;
use crate::hold_facet_trust_lock;

/// Adopt the existing facet identity schema at daily write admission only.
/// Existing valid identities and all other declaration fields are preserved.
pub fn ensure_daily_facet_id(root: &Path, facet: &str) -> Result<String, String> {
    let _guard = hold_facet_trust_lock(root).map_err(|e| e.to_string())?;
    let declaration = read_facet_declaration(root, facet)
        .map_err(|e| e.to_string())?
        .ok_or("conflict: owning facet no longer exists")?;
    if declaration.value().get("id").is_some() {
        return facet_write_identity(root, facet).map_err(|e| e.to_string());
    }
    let mut value = declaration.into_value();
    let id = assign_new_facet_id_locked(&mut value, root).map_err(|e| e.to_string())?;
    save_facet_declaration(root, facet, &value).map_err(|e| e.to_string())?;
    Ok(id)
}

const MAX_COLLISION_RETRIES: usize = 32;

/// Outcome of backfilling facet identifiers across declared facets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillReport {
    pub total_scanned: usize,
    pub backfilled_count: usize,
    pub unchanged_count: usize,
    pub committed: bool,
}

/// Check if an identifier is a canonical lowercase RFC 4122 UUIDv4 string.
pub fn is_well_formed_facet_id(id: &str) -> bool {
    solstone_core_journal_io::is_uuid_v4(id)
}

/// Mint one random RFC 4122 UUIDv4 identifier string.
fn generate_uuid_v4() -> Result<String, FacetIdError> {
    solstone_core_journal_io::mint_uuid_v4().map_err(FacetIdError::from)
}

/// Scan all declared facets and collect their existing well-formed `id` values.
fn collect_existing_ids(journal_root: &Path) -> Result<HashSet<String>, FacetIdError> {
    let mut existing = HashSet::new();
    let declared_names = list_declared_facet_names(journal_root).map_err(FacetIdError::Store)?;
    for name in declared_names {
        let Some(snapshot) =
            read_facet_declaration(journal_root, &name).map_err(FacetIdError::Store)?
        else {
            continue;
        };
        if let Some(id) = snapshot
            .value()
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_well_formed_facet_id(id))
        {
            existing.insert(id.to_owned());
        }
    }
    // A retired facet's id is never given to a new facet either.
    for entry in super::retired::read_retired_facets(journal_root)
        .entries()
        .into_values()
    {
        existing.extend(entry.id);
    }
    Ok(existing)
}

/// Allocate a new journal-local facet identifier, acquiring the facet trust lock.
pub fn allocate_facet_id(journal_root: &Path) -> Result<String, FacetIdError> {
    let _guard = hold_facet_trust_lock(journal_root)?;
    allocate_facet_id_locked(journal_root)
}

/// Allocate a new journal-local facet identifier while the facet trust lock is already held.
pub fn allocate_facet_id_locked(journal_root: &Path) -> Result<String, FacetIdError> {
    let existing_ids = collect_existing_ids(journal_root)?;
    for _ in 0..MAX_COLLISION_RETRIES {
        let candidate = generate_uuid_v4()?;
        if !existing_ids.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(FacetIdError::CollisionExhausted)
}

/// Strip any incoming `id` field from a facet declaration Value.
pub fn strip_incoming_facet_id(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        map.remove("id");
    }
}

/// Strip any foreign `id` and assign a new journal-local identifier under facet trust lock.
pub fn assign_new_facet_id(value: &mut Value, journal_root: &Path) -> Result<String, FacetIdError> {
    let _guard = hold_facet_trust_lock(journal_root)?;
    assign_new_facet_id_locked(value, journal_root)
}

/// Strip any foreign `id` and assign a new journal-local identifier while trust lock is held.
pub fn assign_new_facet_id_locked(
    value: &mut Value,
    journal_root: &Path,
) -> Result<String, FacetIdError> {
    strip_incoming_facet_id(value);
    let id = allocate_facet_id_locked(journal_root)?;
    if let Some(map) = value.as_object_mut() {
        map.insert("id".to_owned(), Value::String(id.clone()));
    }
    Ok(id)
}

/// Resolve a facet identifier to its current directory name using exact byte comparison.
/// This is display resolution, not query-boundary enforcement: boundaries carry
/// identifiers and are never constructed by translating them to names first.
pub fn resolve_facet_id(journal_root: &Path, id: &str) -> Result<String, FacetIdResolveError> {
    if !is_well_formed_facet_id(id) {
        return Err(FacetIdResolveError::Malformed);
    }
    let declared_names =
        list_declared_facet_names(journal_root).map_err(|_| FacetIdResolveError::Store)?;
    let mut matched_directory: Option<String> = None;
    for name in declared_names {
        let snapshot = read_facet_declaration(journal_root, &name)
            .map_err(|_| FacetIdResolveError::Store)?
            .ok_or(FacetIdResolveError::Store)?;
        if snapshot.value().get("id").and_then(Value::as_str) == Some(id) {
            if matched_directory.is_some() {
                return Err(FacetIdResolveError::Duplicate);
            }
            matched_directory = Some(name);
        }
    }
    matched_directory.ok_or(FacetIdResolveError::Missing)
}

/// Backfill identifiers for declared facets that lack a well-formed identifier.
pub fn backfill_facet_ids(
    journal_root: &Path,
    commit: bool,
) -> Result<BackfillReport, FacetIdError> {
    let _guard = hold_facet_trust_lock(journal_root)?;
    let declared_names = list_declared_facet_names(journal_root).map_err(FacetIdError::Store)?;
    let mut existing_ids = HashSet::new();
    let mut pending_backfill = Vec::new();

    // Pass 1: scan all declarations, validate existing IDs and detect duplicate IDs on disk
    for name in &declared_names {
        let snapshot = read_facet_declaration(journal_root, name)
            .map_err(FacetIdError::Store)?
            .ok_or_else(|| {
                FacetIdError::Store(super::error::FacetStoreError::DeclarationNotObject {
                    path: journal_root.join("facets").join(name).join("facet.json"),
                })
            })?;
        if let Some(id) = snapshot
            .value()
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_well_formed_facet_id(id))
        {
            if existing_ids.contains(id) {
                return Err(FacetIdError::DuplicateIdOnDisk { id: id.to_owned() });
            }
            existing_ids.insert(id.to_owned());
            continue;
        }
        pending_backfill.push((name.clone(), snapshot.into_value()));
    }

    let backfilled_count = pending_backfill.len();
    let unchanged_count = declared_names.len() - backfilled_count;

    // Pass 2: generate IDs for declarations lacking a well-formed ID
    for (name, mut value) in pending_backfill {
        let mut new_id = None;
        for _ in 0..MAX_COLLISION_RETRIES {
            let candidate = generate_uuid_v4()?;
            if !existing_ids.contains(&candidate) {
                existing_ids.insert(candidate.clone());
                new_id = Some(candidate);
                break;
            }
        }
        let id = new_id.ok_or(FacetIdError::CollisionExhausted)?;
        if commit {
            if let Some(map) = value.as_object_mut() {
                map.insert("id".to_owned(), Value::String(id));
            }
            save_facet_declaration(journal_root, &name, &value)
                .map_err(|e| FacetIdError::Write(Box::new(e)))?;
        }
    }

    Ok(BackfillReport {
        total_scanned: declared_names.len(),
        backfilled_count,
        unchanged_count,
        committed: commit,
    })
}
