// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A facet in the trash, and the files a facet merge dropped.
//!
//! The trash (`solstone-core-trash`) moves the files; this is the facets' side
//! of it. Both kinds hold the facet trust lock while their files move.
//!
//! - **A facet** ([`Facets`]) is its whole folder, `facets/<name>/`, moved in
//!   one rename. Its own id comes back with its `facet.json`, so what names it
//!   by id heals by itself; search is told what left and came back, and a
//!   recover queues a full scan and a reclassify, as a merge does. A new facet
//!   of the same name sits at the same folder, so a recover replaces it.
//! - **What a merge dropped** ([`FacetDrops`]) is one hidden folder,
//!   `facets/.facet-merge-<tx>.dropped/`: the merged facet's copy of each file
//!   both facets had, under `files/`, and a `manifest.json` naming the facet
//!   they were merged into. Recovering it puts each file back over that
//!   facet's own copy, which goes with the item. A leftover folder with no
//!   manifest (one the doctor moved to the trash as it was) comes back where it
//!   was, and nothing else is touched.

use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_retention::notify::IndexNotify;
use solstone_core_retention::receipt::RemovedPath;
use solstone_core_trash::{Change, Hold, Item, Kind, State, TrashError};

use crate::{FacetTrustLock, hold_facet_trust_lock};

// A hold's refusal is a code the page turns into its own words: a compact
// JSON object, `{"code": ..}` with any fields the words need.

/// Refused: a journal keeps one enabled facet.
pub const LAST_FACET: &str = r#"{"code":"last_facet"}"#;

/// Refused: the facet lock, or another removal, is in the way.
pub const BUSY: &str = r#"{"code":"busy"}"#;

/// Refused: the facet's name now leads to another facet, merged or renamed
/// since under another id.
pub const NAME_REDIRECTED: &str = r#"{"code":"name_redirected"}"#;

/// Refused: the facet a merge's files were merged into is gone for good.
pub const MERGE_DEST_MISSING: &str = r#"{"code":"merge_dest_missing"}"#;

/// Refused: the record of past facet names can't be read.
pub const RECORD_DAMAGED: &str = r#"{"code":"record_damaged"}"#;

/// Refused: the facet a merge's files were merged into waits in the trash,
/// under the owner's name for it, `title`.
pub fn waits_dest(title: &str) -> String {
    serde_json::json!({"code": "waits_dest", "title": title}).to_string()
}

/// The code a refusal carries: one a hold gave, `busy` for a step moving now,
/// `gone` for a thing no longer here, or `other`.
pub fn failure_key(error: &TrashError) -> String {
    match error {
        TrashError::Held(reason) => serde_json::from_str::<Value>(reason)
            .ok()
            .and_then(|value| value.get("code")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| "busy".to_owned()),
        TrashError::InFlight | TrashError::Unfinished(_) => "busy".to_owned(),
        TrashError::Missing(_) | TrashError::NotInTrash => "gone".to_owned(),
        _ => "other".to_owned(),
    }
}

/// A merge's manifest, beside the files it dropped.
pub const MANIFEST: &str = "manifest.json";

/// Where a merge's dropped files wait, below its folder.
pub const DROPPED_FILES: &str = "files";

/// The prefix every facet merge's scratch folder carries.
pub const MERGE_PREFIX: &str = ".facet-merge-";

/// Whether `name` can name a facet's folder.
fn facet_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && name.chars().all(|c| !c.is_control())
}

/// The facet name a facet item's path is.
fn name_of(path: &str) -> Option<&str> {
    path.strip_prefix("facets/").filter(|name| facet_name(name))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// The id a `facet.json` declares.
fn declared_id(declaration: &Path) -> Option<String> {
    read_json(declaration)?
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| crate::is_well_formed_facet_id(id))
        .map(str::to_owned)
}

/// The facet the trust lock keeps still while a thing moves.
struct FacetHold {
    _trust: FacetTrustLock,
}

impl Hold for FacetHold {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

fn trust(journal: &Path) -> Result<FacetTrustLock, String> {
    hold_facet_trust_lock(journal).map_err(|error| {
        log::warn!("facet trash: the facet lock wasn't taken: {error}");
        BUSY.to_owned()
    })
}

/// The trash's kind for facets.
pub struct Facets<'a> {
    index: &'a dyn IndexNotify,
}

impl<'a> Facets<'a> {
    pub fn new(index: &'a dyn IndexNotify) -> Self {
        Self { index }
    }

    /// Move facet `name` to the trash, as the owner's `action` from `surface`.
    pub fn trash(
        &self,
        journal: &Path,
        name: &str,
        action: Action,
        surface: Surface,
    ) -> Result<Item, TrashError> {
        if !facet_name(name) {
            return Err(TrashError::Missing("facet".to_owned()));
        }
        let path = format!("facets/{name}");
        let title = read_json(&journal.join(&path).join("facet.json"))
            .and_then(|declaration| {
                declaration
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| name.to_owned());
        solstone_core_trash::detach_labelled(
            journal,
            self,
            Unit::Facet,
            action,
            surface,
            vec![path],
            Some(title.chars().take(200).collect()),
        )
    }
}

impl Kind for Facets<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Facet)
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let name = item
            .paths
            .first()
            .and_then(|path| name_of(path))
            .ok_or_else(|| "this isn't a facet your journal can place".to_owned())?
            .to_owned();
        let hold = FacetHold {
            _trust: trust(journal)?,
        };
        if item.state == State::Detaching {
            // A journal keeps one facet to route what it senses to.
            let inventory = crate::observe_declared_facet_inventory(journal)
                .map_err(|error| error.to_string())?;
            if inventory.enabled == [name.as_str()] {
                return Err(LAST_FACET.to_owned());
            }
            return Ok(Box::new(hold));
        }
        // Coming back: refused when the name now leads to another facet, a
        // newer one of this name that was merged or renamed away since.
        let held = solstone_core_trash::held_file(journal, &item.id, &format!("facets/{name}"))
            .map_err(|error| error.to_string())?;
        if held.is_dir() {
            let own = declared_id(&held.join("facet.json"));
            let entry = crate::read_retired_facets(journal)
                .entries_for_write()
                .map_err(|_| RECORD_DAMAGED.to_owned())?
                .remove(&name);
            // Two facets with no id can't be told apart, so one counts as another.
            if entry
                .is_some_and(|entry| entry.redirects() && (entry.id.is_none() || entry.id != own))
            {
                return Err(NAME_REDIRECTED.to_owned());
            }
        }
        Ok(Box::new(hold))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        name_of(path).is_some() && journal.join(path).join("facet.json").is_file()
    }

    fn references(
        &self,
        journal: &Path,
        _item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        match change {
            Change::Detached => {
                let removed: Vec<RemovedPath> =
                    files.iter().cloned().map(RemovedPath::confirmed).collect();
                self.index.paths_removed(&removed).map(|_| ())
            }
            Change::Attached => self.index.paths_returned(files),
        }
        .map_err(|error| format!("search was not updated: {}", error.reason))?;
        if change == Change::Attached {
            // What names the facet comes back with it: search reads every
            // file again and puts stored classifications right, as a merge
            // does.
            for token in [
                solstone_core_indexer_store::todo::Token::ScanFull,
                solstone_core_indexer_store::todo::Token::Reclassify,
            ] {
                solstone_core_indexer_store::todo::enqueue_token(journal, token)
                    .map_err(|error| format!("search was not queued: {error}"))?;
            }
        }
        Ok(())
    }
}

/// The facets in the trash, each by the id its held `facet.json` declares,
/// with the owner's name for it.
fn trashed_facet(journal: &Path, id: &str) -> Result<Option<String>, String> {
    let mut items = solstone_core_trash::list(journal).map_err(|error| error.to_string())?;
    items.extend(solstone_core_trash::moving(journal).map_err(|error| error.to_string())?);
    for item in items {
        if item.unit != Unit::Facet {
            continue;
        }
        let Some(path) = item.paths.first() else {
            continue;
        };
        let held = solstone_core_trash::held_file(journal, &item.id, &format!("{path}/facet.json"))
            .map_err(|error| error.to_string())?;
        if declared_id(&held).as_deref() == Some(id) {
            let name = name_of(path).unwrap_or_default().to_owned();
            return Ok(Some(item.label.unwrap_or(name)));
        }
    }
    Ok(None)
}

/// The live facet whose `facet.json` declares `id`, by its name now.
fn live_facet(journal: &Path, id: &str) -> Result<Option<String>, String> {
    for name in crate::list_declared_facet_names(journal).map_err(|error| error.to_string())? {
        if declared_id(&journal.join("facets").join(&name).join("facet.json")).as_deref()
            == Some(id)
        {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// What a merge wrote beside the files it dropped.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DropManifest {
    /// The id of the facet the files were merged into.
    pub dest_id: String,
    /// Its name when the merge ran.
    pub dest_name: String,
    /// Each dropped file, relative to the facet folder, `/`-separated.
    pub files: Vec<String>,
}

impl DropManifest {
    /// Read a manifest; `None` when there is none.
    pub fn read(path: &Path) -> Result<Option<Self>, String> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| format!("{}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }
}

/// Whether `rel` is a plain relative path below a facet folder.
fn plain_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.starts_with('/')
        && !rel.contains('\\')
        && rel
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Whether `name` is a facet merge's folder under `facets/`.
fn merge_folder(name: &str) -> bool {
    name.starts_with(MERGE_PREFIX) && !name.contains(['/', '\\']) && name.len() > MERGE_PREFIX.len()
}

/// The trash's kind for what a facet merge dropped, and for a merge's
/// leftover folders.
pub struct FacetDrops<'a> {
    index: &'a dyn IndexNotify,
}

impl<'a> FacetDrops<'a> {
    pub fn new(index: &'a dyn IndexNotify) -> Self {
        Self { index }
    }

    /// Move a merge's folder `facets/<folder>` to the trash as one item, as
    /// it is.
    pub fn trash(
        &self,
        journal: &Path,
        folder: &str,
        action: Action,
        surface: Surface,
    ) -> Result<Item, TrashError> {
        if !merge_folder(folder) {
            return Err(TrashError::Missing("thing".to_owned()));
        }
        solstone_core_trash::detach(
            journal,
            self,
            Unit::FacetFile,
            action,
            surface,
            vec![format!("facets/{folder}")],
        )
    }

    /// The item's manifest: from its folder in the journal if it came back,
    /// else from the trash. `None` when it has none, and when nothing of it
    /// is left anywhere.
    fn manifest(journal: &Path, item: &Item) -> Result<Option<DropManifest>, String> {
        let Some(folder) = item.paths.first() else {
            return Ok(None);
        };
        let live = journal.join(folder);
        if live.is_dir() {
            return DropManifest::read(&live.join(MANIFEST));
        }
        let held =
            solstone_core_trash::held_file(journal, &item.id, &format!("{folder}/{MANIFEST}"))
                .map_err(|error| error.to_string())?;
        DropManifest::read(&held)
    }

    /// Put each staged file into the facet it was merged into, over that
    /// facet's own copy, then remove the staging folder. Each step is skipped
    /// once done, so a resumed recover finishes.
    fn put_back(&self, journal: &Path, item: &Item) -> Result<(), String> {
        let Some(folder) = item.paths.first() else {
            return Ok(());
        };
        let staging = journal.join(folder);
        if !staging.is_dir() {
            // Already put back and removed.
            return Ok(());
        }
        let Some(manifest) = DropManifest::read(&staging.join(MANIFEST))? else {
            // A leftover the doctor moved as it was: it is back where it was.
            // Only an empty one is the last step of a put-back that stopped
            // after its manifest went, and it holds nothing.
            if fs::read_dir(&staging).is_ok_and(|mut listing| listing.next().is_none()) {
                remove_staging_folder(journal, &staging)?;
            }
            return Ok(());
        };
        let dest =
            live_facet(journal, &manifest.dest_id)?.ok_or_else(|| MERGE_DEST_MISSING.to_owned())?;
        let mut returned = Vec::new();
        for rel in &manifest.files {
            if !plain_rel(rel) {
                return Err(format!("the merge record names an unsafe path: {rel}"));
            }
            let staged = staging.join(DROPPED_FILES).join(rel);
            let target_rel = format!("facets/{dest}/{rel}");
            if solstone_core_journal_io::path_lexists(&staged).map_err(|e| e.to_string())? {
                solstone_core_trash::set_aside(journal, item, &target_rel)
                    .map_err(|error| error.to_string())?;
                let (parent, _) = target_rel
                    .rsplit_once('/')
                    .ok_or_else(|| format!("{target_rel} has no folder"))?;
                solstone_core_journal_io::paths::ensure_directory(&journal.join(parent))
                    .map_err(|error| error.to_string())?;
                let target = solstone_core_journal_io::contained_path(journal, parent)
                    .map_err(|error| error.to_string())?
                    .join(rel.rsplit('/').next().unwrap_or(rel));
                if solstone_core_journal_io::path_lexists(&target).map_err(|e| e.to_string())? {
                    return Err(format!("{target_rel} is already taken"));
                }
                fs::rename(&staged, &target).map_err(|error| format!("{target_rel}: {error}"))?;
                if let Err(error) = solstone_core_journal_io::entry::sync_dir(journal, parent) {
                    log::warn!("facet merge files: {parent} was not flushed: {error}");
                }
            }
            if journal.join(&target_rel).is_file() {
                returned.push(target_rel);
            }
        }
        // Every file the merge dropped that is in the facet now, whether this
        // run moved it or an earlier one did: the folder goes only once search
        // has been told, so a folder gone means search was told.
        self.index
            .paths_returned(&returned)
            .map_err(|error| format!("search was not updated: {}", error.reason))?;
        // Only the manifest and empty folders are left; a file still there is
        // never removed with them.
        let files = staging.join(DROPPED_FILES);
        if any_file(&files) {
            return Err(format!("{folder} still holds files"));
        }
        #[expect(
            clippy::disallowed_methods,
            reason = "system internal: the empty folders a merge's staging folder kept its files in, once every file is back in place"
        )]
        solstone_core_journal_io::removal::remove_contained_tree(journal, &files)
            .map_err(|error| error.to_string())?;
        // The manifest last, so a stop before it leaves a put-back that runs
        // again.
        #[expect(
            clippy::disallowed_methods,
            reason = "system internal: the merge's own record of the files it staged, once every one is back in place"
        )]
        solstone_core_journal_io::entry::remove_file(journal, &format!("{folder}/{MANIFEST}"))
            .map_err(|error| error.to_string())?;
        remove_staging_folder(journal, &staging)
    }
}

/// Remove a merge's staging folder that holds nothing now.
fn remove_staging_folder(journal: &Path, staging: &Path) -> Result<(), String> {
    #[expect(
        clippy::disallowed_methods,
        reason = "system internal: a merge's staging folder that holds nothing now"
    )]
    solstone_core_journal_io::removal::remove_contained_tree(journal, staging)
        .map_err(|error| error.to_string())
}

/// Whether any entry other than a folder is below `path`.
fn any_file(path: &Path) -> bool {
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(listing) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in listing.flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(entry.path()),
                _ => return true,
            }
        }
    }
    false
}

impl Kind for FacetDrops<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::FacetFile)
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        if !item
            .paths
            .first()
            .and_then(|path| path.strip_prefix("facets/"))
            .is_some_and(merge_folder)
        {
            return Err("this isn't a facet merge your journal can place".to_owned());
        }
        let hold = FacetHold {
            _trust: trust(journal)?,
        };
        if item.state == State::Detaching {
            return Ok(Box::new(hold));
        }
        // Coming back over the facet they were merged into: that facet must
        // be in the journal. One waiting in the trash comes back first; one
        // gone for good leaves the item waiting on its timer.
        if let Some(manifest) = Self::manifest(journal, item)?
            && live_facet(journal, &manifest.dest_id)?.is_none()
        {
            return Err(match trashed_facet(journal, &manifest.dest_id)? {
                Some(title) => waits_dest(&title),
                None => MERGE_DEST_MISSING.to_owned(),
            });
        }
        Ok(Box::new(hold))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        path.strip_prefix("facets/").is_some_and(merge_folder)
            && fs::symlink_metadata(journal.join(path)).is_ok_and(|meta| meta.is_dir())
    }

    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        match change {
            Change::Detached => {
                let removed: Vec<RemovedPath> =
                    files.iter().cloned().map(RemovedPath::confirmed).collect();
                self.index
                    .paths_removed(&removed)
                    .map(|_| ())
                    .map_err(|error| format!("search was not updated: {}", error.reason))
            }
            Change::Attached => self.put_back(journal, item),
        }
    }
}

#[cfg(test)]
#[path = "facet_trash_tests.rs"]
mod tests;
