// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Safe movement of facet-scoped entity directories.

use std::path::Path;

use serde_json::Value;
use solstone_core_entity::facet_links::{FolderState, LinkDirs, LinkFieldPolicy};
use solstone_core_entity_matching::{entity_slug, normalize_resolution_query};

use crate::hold_facet_trust_lock;

use super::error::FacetEntityWriteError;
use super::facet_entities::list_scoped_facet_entities;

/// Outcome of moving one facet-scoped entity directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetEntityMoveResult {
    pub entity_dir: String,
    pub moved_from: String,
    pub moved_to: String,
    pub merged: bool,
}

/// The entity a written name links in a facet, if any.
///
/// The link folder is a label, not an identity: a name can change after the
/// link is written, so the linked identities are matched by name first.
fn linked_entity_for_name(
    journal_root: &Path,
    facet_dir: &str,
    entity_name: &str,
) -> Result<Option<String>, FacetEntityWriteError> {
    let wanted = normalize_resolution_query(entity_name);
    Ok(
        list_scoped_facet_entities(journal_root, facet_dir, true, true)?
            .into_iter()
            .find(|entity| {
                let name = entity
                    .identity
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                normalize_resolution_query(name) == wanted
            })
            .map(|entity| entity.entity_id),
    )
}

/// Move an entity's facet memory to another facet without dropping a note.
///
/// A linked entity brings every folder that links it, and lands in the folder
/// named by its id, folding into the link the destination already has. Notes
/// under a name no link claims move by that name's folder, as they always
/// have. Without `merge`, a destination that already holds the entity refuses.
pub fn move_facet_entity(
    journal_root: &Path,
    entity_name: &str,
    from_facet: &str,
    to_facet: &str,
    merge: bool,
) -> Result<FacetEntityMoveResult, FacetEntityWriteError> {
    // A move into the facet it is already in would take the folder in and
    // then remove it as the source, however the name is spelled.
    let facet_dir = |facet: &str| {
        solstone_core_journal_io::contained_path(journal_root, &format!("facets/{facet}"))
            .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))
    };
    let same = from_facet == to_facet
        || solstone_core_journal_io::same_directory(&facet_dir(from_facet)?, &facet_dir(to_facet)?)
            .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))?;
    if same {
        return Err(FacetEntityWriteError::SameFacet {
            facet: to_facet.to_owned(),
        });
    }
    let _trust = hold_facet_trust_lock(journal_root)?;
    let from = LinkDirs::for_facet(journal_root, from_facet);
    let to = LinkDirs::for_facet(journal_root, to_facet);
    let result = |entity_dir: String, merged: bool| FacetEntityMoveResult {
        entity_dir,
        moved_from: from_facet.to_owned(),
        moved_to: to_facet.to_owned(),
        merged,
    };
    let Some(entity_id) = linked_entity_for_name(journal_root, from_facet, entity_name)? else {
        let entity_dir = entity_slug(entity_name);
        if from.state(&entity_dir)? == FolderState::Absent {
            return Err(FacetEntityWriteError::EntityNotFound {
                entity_id: entity_name.to_owned(),
            });
        }
        if to.state(&entity_dir)? == FolderState::Absent {
            let store_error = |error: solstone_core_journal_io::PathError| {
                FacetEntityWriteError::FacetStore(error.into())
            };
            solstone_core_journal_io::ensure_directory(
                &solstone_core_journal_io::contained_path(journal_root, to.entities_rel())
                    .map_err(store_error)?,
            )
            .map_err(store_error)?;
            solstone_core_journal_io::rename_within(
                journal_root,
                &from.folder_rel(&entity_dir),
                &to.folder_rel(&entity_dir),
            )
            .map_err(store_error)?;
            return Ok(result(entity_dir, false));
        }
        if !merge {
            return Err(FacetEntityWriteError::EntityExists {
                name: entity_name.to_owned(),
            });
        }
        to.fold_into(
            &from,
            &entity_dir,
            &entity_dir,
            LinkFieldPolicy::Merge,
            &mut |_| Ok(()),
        )?;
        remove_folder(&from, &entity_dir)?;
        return Ok(result(entity_dir, true));
    };
    let mut folders: Vec<String> = from
        .folders_for(&entity_id)?
        .into_iter()
        .map(|link| link.dir)
        .collect();
    if folders.is_empty() {
        return Err(FacetEntityWriteError::EntityNotFound {
            entity_id: entity_name.to_owned(),
        });
    }
    // Notes under the entity's id that no link claimed go with it too.
    if from.state(&entity_id)? == FolderState::Orphan {
        folders.push(entity_id.clone());
    }
    let incoming: Vec<(&LinkDirs, &str)> =
        folders.iter().map(|dir| (&from, dir.as_str())).collect();
    // Checked before anything is written, so a refusal changes nothing.
    to.check_take_in_all(&entity_id, &incoming)?;
    let occupied = to.find(&entity_id)?.is_some() || to.state(&entity_id)? != FolderState::Absent;
    if occupied && !merge {
        return Err(FacetEntityWriteError::EntityExists {
            name: entity_name.to_owned(),
        });
    }
    // Every folder that links the entity leaves together, so it never ends up
    // split between two facets.
    let (entity_dir, _) =
        to.take_in_all(&entity_id, &incoming, LinkFieldPolicy::Merge, &mut |_| {
            Ok(())
        })?;
    for dir in &folders {
        remove_folder(&from, dir)?;
    }
    Ok(result(entity_dir, occupied))
}

fn remove_folder(dirs: &LinkDirs, dir: &str) -> Result<(), FacetEntityWriteError> {
    solstone_core_journal_io::remove_dir_all(dirs.root(), &dirs.folder_rel(dir))
        .map_err(|error| FacetEntityWriteError::FacetStore(error.into()))
}
