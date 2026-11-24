// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A quiet, repeatable pass over facet and entity link state that a write
//! stopped part way can leave behind. Every step does only what the normal
//! path for that data would have done, under the same locks, and a second run
//! changes nothing:
//!
//! 1. A facet merge's leftover folder that only copies a live facet (a
//!    `.dest`, `.stage` or `.preview` carrying a live facet's id) goes to the
//!    trash, as the facet doctor's sort calls it safe. Every other leftover
//!    (`.source`, `.dropped`, a `.dest` whose facet is gone) may hold the only
//!    copy of something and is never touched.
//! 2. Links to a merged-away entity join its live successor, with the facet
//!    doctor's own repair, which also settles an entity's duplicate or
//!    misnamed folders into the one named by its id.
//! 3. A link whose entity the owner deleted, and whose trash item is gone, is
//!    removed when it holds nothing but the link itself. An entity still in
//!    the trash keeps its links, so recovering it finds them; a link holding
//!    notes or other files is left as it is.
//! 4. A hidden entity's still-attached links are detached as hiding does,
//!    marked `detached_by: "hide"`. Already-detached links keep their marker
//!    or lack of one, and nothing is ever re-attached: an unhide puts links
//!    back before it clears the hidden flag, so a visible entity with marked
//!    links is never what a stopped unhide leaves.
//!
//! Only declared facets are read: a folder under `facets/` with no readable
//! declaration is left entirely alone.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::path::Path;

use serde_json::Value;
use solstone_core_entity::facet_links::{LinkDirs, LinkIssueKind};
use solstone_core_entity::{IdentityMapLoserReason, RetiredState};
use solstone_core_journal_io::{DirEntryKind, list_dir_entries};
use solstone_core_removals::{Action, Surface};
use solstone_core_trash::{State, TrashError};

use crate::facet_trash::{self, FacetDrops};
use crate::{FacetTrustLockError, PlannedFacetRelationship, hold_facet_trust_lock};

/// What one pass did.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct FacetLinkHealReport {
    /// Facet merge leftovers moved to the trash: copies a live facet holds.
    pub merge_copies_trashed: usize,
    /// Facet merge leftovers left in place: they may hold the only copy.
    pub merge_leftovers_kept: usize,
    /// Facets where a merged-away entity's links joined its live successor.
    pub merged_links_folded: usize,
    /// Facets where an entity's duplicate or misnamed folders were settled.
    pub link_folders_settled: usize,
    /// Links to an entity deleted for good, removed: they held only the link.
    pub absent_links_removed: usize,
    /// Links to an entity deleted for good, left: they hold notes or files.
    pub absent_links_kept: usize,
    /// A hidden entity's still-attached links, detached as hiding does.
    pub hidden_links_detached: usize,
    /// What couldn't be done; the rest of the pass still ran.
    pub failures: Vec<String>,
}

impl FacetLinkHealReport {
    fn fail(&mut self, failure: String) {
        log::warn!("facet link self-heal: {failure}");
        self.failures.push(failure);
    }
}

/// Why a pass didn't run.
#[derive(Debug)]
pub enum FacetLinkHealError {
    FacetTrustLock(FacetTrustLockError),
    EntityTrustLock(solstone_core_entity::EntityTrustLockError),
}

impl fmt::Display for FacetLinkHealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FacetTrustLock(error) => error.fmt(formatter),
            Self::EntityTrustLock(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FacetLinkHealError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FacetTrustLock(error) => Some(error),
            Self::EntityTrustLock(error) => Some(error),
        }
    }
}

/// Run every step once over `journal`, holding the facet trust lock and then
/// the entity trust lock throughout, the order every facet-and-entity writer
/// takes them. A step that fails is reported and the others still run.
pub fn self_heal_facet_links(journal: &Path) -> Result<FacetLinkHealReport, FacetLinkHealError> {
    // Links and merge leftovers both live under `facets/`; without it there is
    // nothing to repair and no reason to take the journal's locks.
    if !journal.join("facets").is_dir() {
        return Ok(FacetLinkHealReport::default());
    }
    let _facet = hold_facet_trust_lock(journal).map_err(FacetLinkHealError::FacetTrustLock)?;
    let _entity = solstone_core_entity::hold_entity_trust_lock(journal)
        .map_err(FacetLinkHealError::EntityTrustLock)?;
    let mut report = FacetLinkHealReport::default();
    trash_merge_copies(journal, &mut report);
    // An entity merge that stopped part way can still put entities and links
    // back; links are judged only once it has been settled.
    if solstone_core_entity::entity_merge_recovery_pending(journal) {
        log::info!(
            "facet link self-heal: an entity merge that stopped part way isn't settled yet; links are left for a later pass"
        );
        return Ok(report);
    }
    match crate::list_declared_facet_names(journal) {
        Ok(facets) => {
            fold_merged_links(journal, &facets, &mut report);
            remove_deleted_entity_links(journal, &facets, &mut report);
            detach_hidden_entity_links(journal, &facets, &mut report);
        }
        Err(error) => report.fail(format!("the facets couldn't be listed: {error}")),
    }
    log::info!(
        "facet link self-heal: {} merge copies trashed, {} merge leftovers kept, {} merged links folded, {} link folders settled, {} deleted-entity links removed, {} kept, {} hidden-entity links detached, {} failures",
        report.merge_copies_trashed,
        report.merge_leftovers_kept,
        report.merged_links_folded,
        report.link_folders_settled,
        report.absent_links_removed,
        report.absent_links_kept,
        report.hidden_links_detached,
        report.failures.len(),
    );
    Ok(report)
}

/// Step 1: a merge leftover that only copies a live facet goes to the trash.
fn trash_merge_copies(journal: &Path, report: &mut FacetLinkHealReport) {
    let leftovers = facet_trash::merge_leftovers(journal);
    if leftovers.is_empty() {
        return;
    }
    let (copies, kept) = match facet_trash::sort_merge_leftovers(journal, leftovers) {
        Ok(sorted) => sorted,
        Err(error) => {
            report.fail(format!("facet merge leftovers couldn't be sorted: {error}"));
            return;
        }
    };
    if !kept.is_empty() {
        log::info!(
            "facet link self-heal: left facet merge leftovers that may hold the only copy of something: {}",
            kept.join(", ")
        );
    }
    report.merge_leftovers_kept = kept.len();
    let index = solstone_core_indexer_store::RetentionIndex::new(journal);
    let drops = FacetDrops::new(&index);
    for folder in copies {
        match drops.trash(journal, &folder, Action::FacetMerge, Surface::Cli) {
            // Unfinished: it has left the journal and the trash's sweep
            // finishes the rest.
            Ok(_) | Err(TrashError::Unfinished(_)) => {
                log::info!("facet link self-heal: moved facet merge copy {folder} to the trash");
                report.merge_copies_trashed += 1;
            }
            Err(error) => report.fail(format!("{folder} wasn't moved to the trash: {error}")),
        }
    }
}

/// Step 2: the facet doctor's link repair, over declared facets.
fn fold_merged_links(journal: &Path, facets: &[String], report: &mut FacetLinkHealReport) {
    match solstone_core_entity::facet_links::repair_facet_links(journal, facets) {
        Ok((repaired, left)) => {
            for issue in repaired {
                log::info!(
                    "facet link self-heal: repaired links to {} in {} ({}): {:?}",
                    issue.entity_id,
                    issue.facet,
                    issue.folders.join(", "),
                    issue.kind
                );
                match issue.kind {
                    LinkIssueKind::Merged { .. } => report.merged_links_folded += 1,
                    LinkIssueKind::Duplicate | LinkIssueKind::Misnamed => {
                        report.link_folders_settled += 1;
                    }
                    LinkIssueKind::Unreadable => {}
                }
            }
            for issue in left.into_iter().filter(|issue| issue.failed) {
                report.fail(format!(
                    "links to {} in {} weren't repaired: {}",
                    issue.entity_id,
                    issue.facet,
                    issue.note.unwrap_or_default()
                ));
            }
        }
        Err(error) => report.fail(format!("facet links couldn't be repaired: {error}")),
    }
}

/// Step 3: remove a link to an entity the owner deleted whose trash item is
/// gone, when the link holds nothing else. The delete was the owner's and the
/// trash has settled it; this link is one it would have taken had its facet
/// been in the journal then.
fn remove_deleted_entity_links(
    journal: &Path,
    facets: &[String],
    report: &mut FacetLinkHealReport,
) {
    let map = match crate::read_identity_map(journal) {
        Ok(map) => map,
        Err(error) => {
            report.fail(format!("entities couldn't be read: {error}"));
            return;
        }
    };
    // An entity that can't be read could be any id: nothing is judged gone.
    if map
        .losers
        .iter()
        .any(|loser| matches!(loser.reason, IdentityMapLoserReason::Malformed { .. }))
    {
        log::info!(
            "facet link self-heal: an entity can't be read, so no link is judged to name one deleted for good"
        );
        return;
    }
    let in_trash = match entities_in_trash(journal) {
        Ok(ids) => ids,
        Err(error) => {
            report.fail(format!(
                "the trash couldn't be read, so links to deleted entities were left: {error}"
            ));
            return;
        }
    };
    for facet in facets {
        let dirs = LinkDirs::for_facet(journal, facet);
        let links = match dirs.scan() {
            Ok((links, _unreadable)) => links,
            Err(error) => {
                report.fail(format!("links in {facet} couldn't be read: {error}"));
                continue;
            }
        };
        for link in links {
            let id = &link.entity_id;
            if map.resolved.contains_key(id) || in_trash.contains(id) {
                continue;
            }
            // Only an id the owner's delete recorded; anything under its name
            // in `entities/`, readable or not, is not gone.
            if !matches!(
                solstone_core_entity::retired_state(journal, id),
                Ok(Some(RetiredState::Deleted))
            ) || !solstone_core_journal_io::contained_path(journal, &format!("entities/{id}"))
                .is_ok_and(|path| {
                    matches!(solstone_core_journal_io::path_lexists(&path), Ok(false))
                })
            {
                continue;
            }
            if !holds_only_link(&dirs, &link.dir) {
                log::info!(
                    "facet link self-heal: left {facet}/entities/{} to deleted entity {id}: it holds notes or other files",
                    link.dir
                );
                report.absent_links_kept += 1;
                continue;
            }
            match crate::delete_facet_entity_link(journal, facet, &link.dir) {
                Ok(true) => {
                    log::info!(
                        "facet link self-heal: removed {facet}/entities/{} to deleted entity {id}",
                        link.dir
                    );
                    report.absent_links_removed += 1;
                }
                Ok(false) => {}
                Err(error) => report.fail(format!(
                    "{facet}/entities/{} wasn't removed: {error}",
                    link.dir
                )),
            }
        }
    }
}

/// Whether a link folder holds its `entity.json` and nothing else.
fn holds_only_link(dirs: &LinkDirs, dir: &str) -> bool {
    dirs.folder_path(dir)
        .ok()
        .and_then(|path| list_dir_entries(&path).ok())
        .is_some_and(|entries| {
            entries.iter().all(|entry| {
                entry.kind == DirEntryKind::File && entry.name.to_str() == Some("entity.json")
            })
        })
}

/// The id of every entity something in the trash could still bring back, and
/// the folder name it went from. An item that can't be read is an error.
fn entities_in_trash(journal: &Path) -> Result<BTreeSet<String>, String> {
    let mut held = BTreeSet::new();
    for id in solstone_core_trash::ids(journal).map_err(|error| error.to_string())? {
        let Some(item) =
            solstone_core_trash::item(journal, &id).map_err(|error| error.to_string())?
        else {
            continue;
        };
        if item.state == State::Erasing {
            continue;
        }
        for path in &item.paths {
            let Some(dir) = path
                .strip_prefix("entities/")
                .filter(|dir| !dir.is_empty() && !dir.contains('/'))
            else {
                continue;
            };
            held.insert(dir.to_owned());
            // The item keeps the entity at its journal name below its files.
            let file = solstone_core_trash::held_file(journal, &item.id, path)
                .map_err(|error| error.to_string())?;
            let Some(root) = file.parent().and_then(Path::parent) else {
                continue;
            };
            let identity = solstone_core_entity::entity_identity_path(root, dir)
                .map_err(|error| error.to_string())?;
            match std::fs::read(&identity) {
                Ok(bytes) => {
                    let value: Value = serde_json::from_slice(&bytes)
                        .map_err(|error| format!("trash item {}: {error}", item.id))?;
                    if let Some(entity_id) = value
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|entity_id| !entity_id.is_empty())
                    {
                        held.insert(entity_id.to_owned());
                    }
                }
                // Moved back already: the journal's copy answers for it.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("trash item {}: {error}", item.id)),
            }
        }
    }
    Ok(held)
}

/// Step 4: a hidden entity's still-attached links detach as hiding does.
fn detach_hidden_entity_links(journal: &Path, facets: &[String], report: &mut FacetLinkHealReport) {
    let map = match crate::read_identity_map(journal) {
        Ok(map) => map,
        Err(error) => {
            report.fail(format!("entities couldn't be read: {error}"));
            return;
        }
    };
    let plan = match crate::plan_facet_entity_relationships(journal) {
        Ok(plan) => plan,
        Err(error) => {
            report.fail(format!("facet links couldn't be read: {error}"));
            return;
        }
    };
    let declared: BTreeSet<&str> = facets.iter().map(String::as_str).collect();
    let mut attached: HashMap<&str, Vec<&PlannedFacetRelationship>> = HashMap::new();
    for rel in &plan.relationships {
        if !rel.is_detached && declared.contains(rel.facet_dir.as_str()) {
            attached
                .entry(rel.entity_id.as_str())
                .or_default()
                .push(rel);
        }
    }
    let mut entity_ids: Vec<&str> = attached.keys().copied().collect();
    entity_ids.sort_unstable();
    let mut hidden = Vec::new();
    for entity_id in entity_ids {
        let Some(entity_dir) = map.resolved.get(entity_id) else {
            continue;
        };
        let identity = match solstone_core_entity::read_entity_identity(journal, entity_dir) {
            Ok(Some(identity)) => identity,
            Ok(None) => continue,
            Err(error) => {
                report.fail(format!("entity {entity_id} couldn't be read: {error}"));
                continue;
            }
        };
        // Hiding refuses the owner's own entity, so it is never detached here.
        if !solstone_core_entity::entity_is_hidden(identity.value())
            || solstone_core_entity::entity_is_hide_protected(&identity)
        {
            continue;
        }
        let links = attached.remove(entity_id).unwrap_or_default();
        match crate::store::detach_attached_for_hide(journal, links) {
            Ok(detached) => {
                for rel in &detached {
                    log::info!(
                        "facet link self-heal: detached {}/entities/{} of hidden entity {entity_id}",
                        rel.facet_dir,
                        rel.relationship_dir
                    );
                }
                if !detached.is_empty() {
                    report.hidden_links_detached += detached.len();
                    hidden.push(entity_id.to_owned());
                }
            }
            Err(error) => report.fail(format!(
                "hidden entity {entity_id}'s links weren't detached: {error}"
            )),
        }
    }
    // As a hide does, so the entity's names in those facets reach search.
    if !hidden.is_empty() && !crate::queue_linked_facet_detections(journal, &hidden) {
        report.fail("search wasn't told about the detached links".to_owned());
    }
}

#[cfg(test)]
#[path = "link_heal_tests.rs"]
mod tests;
