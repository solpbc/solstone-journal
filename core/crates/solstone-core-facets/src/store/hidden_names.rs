// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The one writer of a facet's hidden names, `facets/<facet>/hidden-names.json`.
//!
//! Hiding a name keeps every detection of it in the journal and marks it
//! hidden wherever detections show. The path, the record and its strict read
//! belong to `solstone_core_entity::hidden_names`; this module changes the
//! record under the facet trust lock with an atomic replace, and after every
//! call, changed or not, queues the facet's detections so search sees the
//! change. A retry after search was not queued therefore heals.

use std::fmt;
use std::path::{Path, PathBuf};

use solstone_core_entity::hidden_names::{
    HiddenName, HiddenNames, encode_hidden_names, hidden_name_key, hidden_names_rel,
    read_hidden_names,
};
use solstone_core_journal_io::{AtomicWriteError, AtomicWriteOptions, PathError, contained_path};

use super::declaration::require_declared_facet;
use super::error::FacetWriteError;
use crate::{FacetTrustLockError, hold_facet_trust_lock};

/// Why a hide or unhide changed nothing.
#[derive(Debug)]
pub enum HiddenNamesError {
    /// No name was given.
    EmptyName,
    /// The facet's hidden names can't be read; nothing was written over them.
    Damaged {
        path: PathBuf,
        detail: String,
    },
    /// The facet isn't declared.
    Facet(FacetWriteError),
    TrustLock(FacetTrustLockError),
    Path(PathError),
    Write(AtomicWriteError),
}

impl HiddenNamesError {
    /// Whether another writer held the facet.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::TrustLock(FacetTrustLockError::Lock(
                solstone_core_entity::LockError::Timeout(_)
            ))
        )
    }
}

impl fmt::Display for HiddenNamesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => formatter.write_str("no name was given"),
            Self::Damaged { path, detail } => write!(
                formatter,
                "the hidden names at {} can't be read, so nothing was changed: {detail}",
                path.display()
            ),
            Self::Facet(error) => error.fmt(formatter),
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for HiddenNamesError {}

/// What a hide or unhide did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiddenNameWrite {
    /// Whether the record changed.
    pub changed: bool,
    /// Whether the facet's detections were queued for search.
    pub search_queued: bool,
}

/// Hide `name` in `facet`. A name whose key is already hidden keeps its first
/// entry.
pub fn hide_detected_name(
    journal: &Path,
    facet: &str,
    name: &str,
) -> Result<HiddenNameWrite, HiddenNamesError> {
    let hidden_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    hide_detected_name_at(journal, facet, name, &hidden_at)
}

/// [`hide_detected_name`] with the moment of the hide given.
pub fn hide_detected_name_at(
    journal: &Path,
    facet: &str,
    name: &str,
    hidden_at: &str,
) -> Result<HiddenNameWrite, HiddenNamesError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(HiddenNamesError::EmptyName);
    }
    let key = hidden_name_key(name);
    let changed = change_hidden_names(journal, facet, |names| {
        if names.iter().any(|entry| entry.key() == key) {
            return false;
        }
        names.push(HiddenName {
            name: name.to_owned(),
            hidden_at: hidden_at.to_owned(),
        });
        true
    })?;
    Ok(HiddenNameWrite {
        changed,
        search_queued: queue_facet_detections(journal, facet),
    })
}

/// Unhide `name` in `facet`: every entry with its key goes. Unhiding a name
/// that isn't hidden changes nothing and succeeds.
pub fn unhide_detected_name(
    journal: &Path,
    facet: &str,
    name: &str,
) -> Result<HiddenNameWrite, HiddenNamesError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(HiddenNamesError::EmptyName);
    }
    let key = hidden_name_key(name);
    let changed = change_hidden_names(journal, facet, |names| {
        let before = names.len();
        names.retain(|entry| entry.key() != key);
        names.len() != before
    })?;
    Ok(HiddenNameWrite {
        changed,
        search_queued: queue_facet_detections(journal, facet),
    })
}

/// Add every name the facet folder `source` hides to the facet folder
/// `destination`, keeping the destination's own entry for a key both hide.
/// Refuses, changing nothing, when either record can't be read. For a facet
/// merge, which holds the facet trust lock, writes into its staged copy, and
/// queues the destination's detections once it commits.
pub fn merge_hidden_names_locked(
    journal: &Path,
    source: &str,
    destination: &str,
) -> Result<bool, HiddenNamesError> {
    let incoming = match read_hidden_names(journal, source) {
        HiddenNames::Absent => return Ok(false),
        HiddenNames::Present(names) => names,
        HiddenNames::Damaged { path, detail } => {
            return Err(HiddenNamesError::Damaged { path, detail });
        }
    };
    write_hidden_names_locked(journal, destination, |names| {
        let mut changed = false;
        for entry in incoming {
            let key = entry.key();
            if !names.iter().any(|existing| existing.key() == key) {
                names.push(entry);
                changed = true;
            }
        }
        changed
    })
}

/// Whether `facet`'s hidden names can be read, for a caller that must refuse
/// before it changes anything.
pub fn check_hidden_names_readable(journal: &Path, facet: &str) -> Result<(), HiddenNamesError> {
    match read_hidden_names(journal, facet) {
        HiddenNames::Damaged { path, detail } => Err(HiddenNamesError::Damaged { path, detail }),
        HiddenNames::Absent | HiddenNames::Present(_) => Ok(()),
    }
}

fn change_hidden_names(
    journal: &Path,
    facet: &str,
    change: impl FnOnce(&mut Vec<HiddenName>) -> bool,
) -> Result<bool, HiddenNamesError> {
    let _trust = hold_facet_trust_lock(journal).map_err(HiddenNamesError::TrustLock)?;
    require_declared_facet(journal, facet).map_err(HiddenNamesError::Facet)?;
    write_hidden_names_locked(journal, facet, change)
}

fn write_hidden_names_locked(
    journal: &Path,
    facet: &str,
    change: impl FnOnce(&mut Vec<HiddenName>) -> bool,
) -> Result<bool, HiddenNamesError> {
    let mut names = match read_hidden_names(journal, facet) {
        HiddenNames::Absent => Vec::new(),
        HiddenNames::Present(names) => names,
        HiddenNames::Damaged { path, detail } => {
            return Err(HiddenNamesError::Damaged { path, detail });
        }
    };
    if !change(&mut names) {
        return Ok(false);
    }
    let path = contained_path(journal, &hidden_names_rel(facet)).map_err(HiddenNamesError::Path)?;
    solstone_core_journal_io::atomic_replace(
        &path,
        &encode_hidden_names(&names),
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(HiddenNamesError::Write)?;
    Ok(true)
}

/// Queue `facets/<facet>/entities` for search, so every detected day and
/// every link folder's notes there are rendered again under the current
/// hides. Logged, and reported as `false`, when it can't be queued.
pub fn queue_facet_detections(journal: &Path, facet: &str) -> bool {
    let rel = format!("facets/{facet}/entities");
    match solstone_core_indexer_store::todo::enqueue_path(journal, Path::new(&rel)) {
        Ok(()) => true,
        Err(error) => {
            log::warn!(
                target: "solstone::indexer",
                "facet detections not queued for search facet={facet} cause={error}"
            );
            false
        }
    }
}

/// Queue the detections of every facet holding a link to one of
/// `entity_ids`, so a hide, unhide or undo of those entities reaches search.
/// A facet whose links can't all be read is queued too. Reports whether
/// every queue succeeded.
pub fn queue_linked_facet_detections(journal: &Path, entity_ids: &[String]) -> bool {
    let mut queued = true;
    for facet in facet_folders(journal) {
        let dirs = solstone_core_entity::facet_links::LinkDirs::for_facet(journal, &facet);
        let links = match dirs.scan() {
            Ok((links, unreadable)) => unreadable.is_empty().then_some(links),
            Err(_) => None,
        };
        let linked = links.is_none_or(|links| {
            links
                .iter()
                .any(|link| entity_ids.contains(&link.entity_id))
        });
        if linked {
            queued &= queue_facet_detections(journal, &facet);
        }
    }
    queued
}

/// Queue the detections of every facet, for a change whose entities aren't
/// known up front (an undo of a set of hides).
pub fn queue_every_facet_detections(journal: &Path) -> bool {
    let mut queued = true;
    for facet in facet_folders(journal) {
        queued &= queue_facet_detections(journal, &facet);
    }
    queued
}

fn facet_folders(journal: &Path) -> Vec<String> {
    let Ok(facets) = contained_path(journal, "facets") else {
        return Vec::new();
    };
    match solstone_core_journal_io::list_dir_entries(&facets) {
        Ok(entries) => entries
            .into_iter()
            .filter(|entry| entry.kind == solstone_core_journal_io::DirEntryKind::Directory)
            .filter_map(|entry| entry.name.to_str().map(str::to_owned))
            // A dot-named folder is a merge's scratch, never a facet.
            .filter(|name| !name.starts_with('.'))
            .collect(),
        Err(error) => {
            log::warn!(
                target: "solstone::indexer",
                "facet detections not queued for search: facets can't be listed: {error}"
            );
            Vec::new()
        }
    }
}
