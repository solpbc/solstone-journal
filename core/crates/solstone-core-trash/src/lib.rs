// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The trash: where what an owner removes waits before it is erased.
//!
//! A thing in the journal is its files plus some metadata. Each kind of thing
//! has one **detach** and one **attach**, and the trash runs both the same way
//! for every kind:
//!
//! - **Detach** moves the thing's files into one folder at the journal root,
//!   [`TRASH_DIR`], and tells the kind to point the journal's references away
//!   from it ([`Kind::references`]). The item keeps what attach needs.
//! - **Attach** moves the files back to where they were, overwriting anything
//!   that has arrived there since, and tells the kind to heal its references.
//! - **Erase** unlinks the files, after the days the owner set
//!   ([`erase_after_days`]) or at once.
//!
//! Every trashed, recovered and erased unit is recorded in the deletion record
//! through `solstone-core-removals`.
//!
//! # One item
//!
//! `TRASH_DIR/<id>/item.json` is the item: its unit, when it was trashed, the
//! journal-relative paths it moved, and its [`State`]. `TRASH_DIR/<id>/files/`
//! holds those paths at the same relative names. Each step publishes the
//! item's state before it moves anything, and every step can run again from
//! what is on disk, so a step a stop interrupts is finished by [`sweep`].
//! A recording is one directory moved by one rename, so it is never partly in
//! the trash and partly in place.
//!
//! # Hidden
//!
//! Nothing that reads or walks the journal looks inside [`TRASH_DIR`], and
//! nothing writes into it but this crate and a whole-journal restore. A root
//! walk skips it through [`is_trash_root_name`].

use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::atomic::{atomic_replace, write_bytes_exclusive};
use solstone_core_journal_io::entry::sync_dir;
use solstone_core_journal_io::locking::{FileLock, LockOptions, hold_lock};
use solstone_core_journal_io::paths::{contained_path, path_lexists};
use solstone_core_journal_io::{AtomicWriteOptions, PathError};
use solstone_core_removals::{Action, Event, OperationId, Removal, Surface, Unit};

/// The trash folder, at the journal root.
pub use solstone_core_journal::TRASH_DIR;

/// The item's own record inside its folder.
const ITEM_FILE: &str = "item.json";

/// The item's files, at their journal-relative names.
const FILES_DIR: &str = "files";

/// What attach moved out of the way before it put the item back.
const DISPLACED_DIR: &str = "displaced";

/// What a kind kept with an item for its own attach ([`keep`]).
const KEPT_DIR: &str = "kept";

/// The lock every change to the trash takes: `TRASH_DIR/trash.lock`.
const LOCK_STEM: &str = "trash";

/// The item format this build writes.
pub const SCHEMA: u32 = 1;

/// Days an item waits before the trash erases it, unless the owner set others.
pub const DEFAULT_ERASE_AFTER_DAYS: u32 = 7;

/// The fewest days the owner can set.
pub const MIN_ERASE_AFTER_DAYS: u32 = 1;

/// The most days the owner can set.
pub const MAX_ERASE_AFTER_DAYS: u32 = 30;

/// Where the owner's setting lives in `config/journal.json`: this section,
/// then [`ERASE_AFTER_DAYS_KEY`].
pub const SETTINGS_SECTION: &str = "trash";

/// The setting's key in [`SETTINGS_SECTION`].
pub const ERASE_AFTER_DAYS_KEY: &str = "erase_after_days";

pub use solstone_core_journal::is_trash_root_name;

mod linked;
pub use linked::{Linked, detach_linked, finish_linked};

/// Where an item stands.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Its files are moving into the trash.
    Detaching,
    /// It is in the trash.
    Trashed,
    /// Its files are moving back.
    Attaching,
    /// Its files are being erased.
    Erasing,
}

/// What an owner act asked of an item that is now attaching or erasing, so a
/// resumed step records the act the owner made.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Pending {
    pub action: Action,
    #[serde(flatten)]
    pub surface: Surface,
    /// When the owner asked.
    pub at: String,
    /// The owner act's operation, when it spans several items (a selection
    /// recovered or erased together); otherwise the item's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
}

/// One thing in the trash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Item {
    pub schema: u32,
    pub id: String,
    pub state: State,
    /// What it is, in the owner's terms and without content.
    pub unit: Unit,
    /// What the owner did to trash it.
    pub action: Action,
    #[serde(flatten)]
    pub surface: Surface,
    /// When it moved to the trash.
    pub trashed_at: String,
    /// When the trash erases it: fixed when it was trashed, so a later change
    /// to the setting never moves an item already waiting.
    pub erase_at: String,
    /// Its journal-relative paths, where attach puts them back.
    pub paths: Vec<String>,
    /// The size of its files.
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    /// The owner's own name for it, where nothing else tells two items apart
    /// (an import's file name). Kept with the item and erased with it; never
    /// in the removal history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Paths it names but didn't take, because another thing still in the
    /// journal held them too ([`Kind::refs`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    /// The owner act's operation, when it moved several things together (a
    /// set); otherwise the item's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// Its files were linked here before a mechanism of the kind's own removed
    /// them from the journal ([`detach_linked`]): settled by what left, never
    /// moved.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linked: bool,
}

impl Item {
    /// When it moved to the trash.
    pub fn trashed_at(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.trashed_at)
            .ok()
            .map(|at| at.with_timezone(&Utc))
    }

    /// When the trash erases it.
    pub fn erase_at(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.erase_at)
            .ok()
            .map(|at| at.with_timezone(&Utc))
    }
}

/// Which way a thing's files just moved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    /// They left the journal for the trash.
    Detached,
    /// They came back from the trash.
    Attached,
}

/// What keeps the journal's other writers off a thing's places while its files
/// move. Checked before each move; released when dropped.
pub trait Hold {
    /// Whether moving may continue.
    fn check(&self) -> Result<(), String>;

    /// About to move `path`: whether it may, holding whatever that one path
    /// needs until the next call or the hold is dropped.
    fn moving(&self, _path: &str) -> Result<(), String> {
        self.check()
    }
}

/// One kind of thing the trash can hold: its exclusion and its references.
/// The trash itself does the moving.
pub trait Kind {
    /// Whether this kind holds `unit`.
    fn handles(&self, unit: &Unit) -> bool;

    /// Hold the thing's places in the journal for the length of a move.
    fn hold<'a>(&'a self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'a>, String>;

    /// Whether `path` holds the thing itself, rather than nothing or only the
    /// marker its detach left there. Asked under the hold.
    fn present(&self, journal: &Path, path: &str) -> bool;

    /// After the files moved, point the journal's references at what is now
    /// true. `files` names every journal-relative file that moved. Runs again
    /// when a step is resumed, so it must be safe to repeat; an item moves on
    /// only once this has succeeded, so a failure is retried by the sweep.
    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String>;

    /// The thing's paths as they are now, worked out under its hold when it
    /// moves: for a thing chosen earlier and moved later (a set the owner
    /// confirmed), and for one moved at once, so what changed since it was chosen
    /// is what moves. `None` keeps the paths it was chosen with. `chosen` are
    /// those paths, for a kind whose unit doesn't say which thing it is.
    fn current_paths(
        &self,
        _journal: &Path,
        _unit: &Unit,
        _chosen: &[String],
    ) -> Option<Vec<String>> {
        None
    }

    /// Paths the thing names but doesn't hold, kept with its item so recovering
    /// it can tell what it shares with another thing in the trash.
    fn refs(&self, _journal: &Path, _unit: &Unit) -> Vec<String> {
        Vec::new()
    }

    /// Files outside the thing's primary paths that must travel with it.
    /// Discovered under its hold before publishing the detach intent; after
    /// that, the persisted paths govern every resumed move, recover and erase.
    fn companions(&self, _journal: &Path, _item: &Item) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    /// Whether one of the item's paths still has a home to come back to.
    /// One that doesn't stays with the item and goes when the item does: it
    /// belonged to a thing the owner removed since (a recording, an entity, a
    /// facet), and putting it back would make a folder holding only it. The
    /// recover records it erased. A home still waiting in the trash is the
    /// kind's hold to refuse, not this.
    fn returns(&self, _journal: &Path, _item: &Item, _path: &str) -> bool {
        true
    }

    /// The unit for an item [`detach_linked`] kept only `paths` of.
    fn narrowed(&self, unit: &Unit, _paths: &[String]) -> Unit {
        unit.clone()
    }
}

/// Why a trash step did not happen.
#[derive(Debug, thiserror::Error)]
pub enum TrashError {
    /// There is nothing at a path the owner asked to trash.
    #[error("there is no such {0} in your journal")]
    Missing(String),
    /// No item has that id.
    #[error("that is no longer in the trash")]
    NotInTrash,
    /// The item is part-way through another step.
    #[error("that is already being recovered or erased")]
    InFlight,
    /// No kind handles this item.
    #[error("this build can't recover that")]
    UnknownKind,
    /// It left the journal for the trash, and a step after the move failed;
    /// the trash finishes it on its next sweep.
    #[error("it's in the trash, which will finish putting it away")]
    Unfinished(String),
    /// The kind's hold refused, in words the owner can read.
    #[error("{0}")]
    Held(String),
    /// The trash could not be read or changed.
    #[error("your trash couldn't be changed: {0}")]
    Io(String),
}

impl From<PathError> for TrashError {
    fn from(error: PathError) -> Self {
        Self::Io(error.to_string())
    }
}

/// Move the paths of one thing into the trash.
///
/// `paths` are journal-relative and must exist. The thing is moved whole or
/// not at all: a stop part-way leaves an item in [`State::Detaching`], which
/// [`sweep`] finishes. It is erased after the days the owner set now
/// ([`erase_after_days`]). Records [`Event::Trashed`].
pub fn detach(
    journal: &Path,
    kind: &dyn Kind,
    unit: Unit,
    action: Action,
    surface: Surface,
    paths: Vec<String>,
) -> Result<Item, TrashError> {
    detach_labelled(journal, kind, unit, action, surface, paths, None)
}

/// [`detach`], keeping the owner's own name for the thing with the item.
pub fn detach_labelled(
    journal: &Path,
    kind: &dyn Kind,
    unit: Unit,
    action: Action,
    surface: Surface,
    paths: Vec<String>,
    label: Option<String>,
) -> Result<Item, TrashError> {
    detach_as(
        journal,
        kind,
        item_id(Utc::now()),
        unit,
        action,
        surface,
        paths,
        label,
        None,
    )
}

/// [`detach_labelled`] under an item id the caller chose, so a set can write
/// the id down before anything moves, and under the set's operation.
#[expect(
    clippy::too_many_arguments,
    reason = "one private step of the public detach, and of a set's"
)]
fn detach_as(
    journal: &Path,
    kind: &dyn Kind,
    id: String,
    unit: Unit,
    action: Action,
    surface: Surface,
    paths: Vec<String>,
    label: Option<String>,
    operation: Option<String>,
) -> Result<Item, TrashError> {
    if paths.is_empty() {
        return Err(TrashError::Missing(describe(&unit).to_owned()));
    }
    let now = Utc::now();
    let erase_at = now + Duration::days(i64::from(erase_after_days(journal)));
    let mut item = Item {
        schema: SCHEMA,
        id,
        state: State::Detaching,
        unit,
        action,
        surface,
        trashed_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
        erase_at: erase_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        paths,
        bytes: 0,
        pending: None,
        refs: Vec::new(),
        label,
        operation,
        linked: false,
    };
    let hold = kind.hold(journal, &item).map_err(TrashError::Held)?;
    hold.check().map_err(TrashError::Held)?;
    // What the thing holds is worked out again under its hold, so a writer that
    // changed it since it was chosen can't have its own work taken with it.
    if let Some(paths) = kind.current_paths(journal, &item.unit, &item.paths) {
        if paths.is_empty() {
            return Err(TrashError::Missing(describe(&item.unit).to_owned()));
        }
        item.paths = paths;
    }
    item.refs = kind.refs(journal, &item.unit);
    let _lock = lock(journal)?;
    let companions = kind.companions(journal, &item).map_err(TrashError::Io)?;
    item.paths.extend(companions);
    for path in &item.paths {
        if !kind.present(journal, path) {
            return Err(TrashError::Missing(describe(&item.unit).to_owned()));
        }
    }
    item.bytes = item
        .paths
        .iter()
        .map(|path| tree_bytes(&journal.join(path)))
        .sum();
    ensure_trash(journal)?;
    let dir = trash_root(journal).join(&item.id);
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        // A set names its item before moving it, so a stop here leaves the
        // folder with no record yet, and nothing moved into it.
        Err(error)
            if error.kind() == std::io::ErrorKind::AlreadyExists
                && read_item(journal, &item.id)?.is_none() => {}
        Err(error) => return Err(TrashError::Io(error.to_string())),
    }
    if let Err(error) = publish_new(journal, &item) {
        abandon(journal, &item);
        return Err(error);
    }
    match finish_detach(journal, kind, &mut item, hold.as_ref()) {
        Ok(()) => Ok(item),
        // Nothing has moved: the recording is where it was, so leave no item
        // behind for a sweep to finish later.
        Err(error) if !any_moved(journal, &item) => {
            abandon(journal, &item);
            Err(error)
        }
        // It has left the journal; the sweep finishes what remains.
        Err(error) => {
            log::warn!("trash item {}: moved, finishing later: {error}", item.id);
            Err(TrashError::Unfinished(item.id.clone()))
        }
    }
}

/// Whether any of an item's paths is already in the trash.
fn any_moved(journal: &Path, item: &Item) -> bool {
    item.paths.iter().any(|path| {
        trash_entry(journal, &files_rel(&item.id, path))
            .and_then(|held| Ok(path_lexists(&held)?))
            .unwrap_or(true)
    })
}

/// Remove an item whose detach moved nothing.
fn abandon(journal: &Path, item: &Item) {
    #[expect(
        clippy::disallowed_methods,
        reason = "system internal: the trash removing an item folder that never held anything"
    )]
    let removed = solstone_core_journal_io::removal::remove_contained_tree(
        journal,
        &trash_root(journal).join(&item.id),
    );
    if let Err(error) = removed {
        log::warn!("trash item {}: an empty item was left: {error}", item.id);
    }
}

/// Move the files of one item back where they were, and heal its references.
/// Something now in the way is overwritten. Records [`Event::Recovered`].
pub fn attach(
    journal: &Path,
    kinds: &[&dyn Kind],
    id: &str,
    action: Action,
    surface: Surface,
) -> Result<Item, TrashError> {
    attach_as(journal, kinds, id, action, surface, None)
}

/// [`attach`] as one of several items an owner act recovers together,
/// recorded under that act's `operation`.
pub fn attach_in(
    journal: &Path,
    kinds: &[&dyn Kind],
    id: &str,
    action: Action,
    surface: Surface,
    operation: &OperationId,
) -> Result<Item, TrashError> {
    attach_as(
        journal,
        kinds,
        id,
        action,
        surface,
        Some(operation.to_string()),
    )
}

fn attach_as(
    journal: &Path,
    kinds: &[&dyn Kind],
    id: &str,
    action: Action,
    surface: Surface,
    operation: Option<String>,
) -> Result<Item, TrashError> {
    let mut item = read_item(journal, id)?.ok_or(TrashError::NotInTrash)?;
    let kind = kind_for(kinds, &item.unit)?;
    let hold = kind.hold(journal, &item).map_err(TrashError::Held)?;
    hold.check().map_err(TrashError::Held)?;
    let _lock = lock(journal)?;
    // Read again under the lock: another step may have settled it.
    item = read_item(journal, id)?.ok_or(TrashError::NotInTrash)?;
    match item.state {
        State::Trashed => {}
        State::Attaching => {}
        State::Detaching | State::Erasing => return Err(TrashError::InFlight),
    }
    if item.state == State::Trashed {
        item.state = State::Attaching;
        item.pending = Some(Pending {
            action,
            surface,
            at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            operation,
        });
        publish(journal, &item)?;
    }
    finish_attach(journal, kind, &item, hold.as_ref())?;
    Ok(item)
}

/// Erase one item now. Records [`Event::Erased`].
pub fn erase(journal: &Path, id: &str, action: Action, surface: Surface) -> Result<(), TrashError> {
    erase_as(journal, id, action, surface, None)
}

/// [`erase`] as one of several items an owner act erases together, recorded
/// under that act's `operation`.
pub fn erase_in(
    journal: &Path,
    id: &str,
    action: Action,
    surface: Surface,
    operation: &OperationId,
) -> Result<(), TrashError> {
    erase_as(journal, id, action, surface, Some(operation.to_string()))
}

fn erase_as(
    journal: &Path,
    id: &str,
    action: Action,
    surface: Surface,
    operation: Option<String>,
) -> Result<(), TrashError> {
    let lock = lock(journal)?;
    let mut item = read_item(journal, id)?.ok_or(TrashError::NotInTrash)?;
    match item.state {
        State::Trashed => {
            item.state = State::Erasing;
            item.pending = Some(Pending {
                action,
                surface,
                at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                operation,
            });
            publish(journal, &item)?;
        }
        State::Erasing => {}
        State::Detaching | State::Attaching => return Err(TrashError::InFlight),
    }
    // Once it is marked erasing nothing else takes it, so the unlink, which
    // can take a while for a long recording, runs without the trash's lock.
    drop(lock);
    finish_erase(journal, &item)?;
    erase_linked_within(journal, &item);
    Ok(())
}

/// A linked item whose every file lived inside the item just erased goes with
/// it, under the same act: an owner who erases a recording has erased the
/// originals released from it too. A stop before this leaves it on its own
/// timer, and a recover of it refuses, its home being gone.
fn erase_linked_within(journal: &Path, erased: &Item) {
    let Ok(items) = all_items(journal) else {
        return;
    };
    let pending = erased.pending.clone();
    for other in items {
        let inside = !other.paths.is_empty()
            && other.paths.iter().all(|path| {
                erased
                    .paths
                    .iter()
                    .any(|home| path.starts_with(&format!("{home}/")))
            });
        if !other.linked || other.state != State::Trashed || other.id == erased.id || !inside {
            continue;
        }
        let (action, surface, operation) = pending
            .clone()
            .map_or((Action::Erase, timer_surface(), None), |pending| {
                (pending.action, pending.surface, pending.operation)
            });
        if let Err(error) = erase_as(journal, &other.id, action, surface, operation) {
            log::warn!(
                "trash item {}: not erased with {}: {error}",
                other.id,
                erased.id
            );
        }
    }
}

/// Everything in the trash, newest first. Items part-way through a step are
/// left out: they are on their way in, back or out.
pub fn list(journal: &Path) -> Result<Vec<Item>, TrashError> {
    let mut items: Vec<Item> = all_items(journal)?
        .into_iter()
        .filter(|item| item.state == State::Trashed)
        .collect();
    items.sort_by(|left, right| {
        right
            .trashed_at
            .cmp(&left.trashed_at)
            .then(right.id.cmp(&left.id))
    });
    Ok(items)
}

/// Items part-way through a step: on their way in, back or out, or stopped
/// there until the next sweep finishes them.
pub fn unfinished(journal: &Path) -> Result<usize, TrashError> {
    Ok(all_items(journal)?
        .iter()
        .filter(|item| item.state != State::Trashed)
        .count())
}

/// Items stopped part way, oldest first, for a page that shows each: the
/// ones [`unfinished`] counts, but only while no step is moving one now (a
/// step holds the trash's lock while it moves), so an item a live request is
/// moving isn't shown as stopped.
pub fn stopped(journal: &Path) -> Result<Vec<Item>, TrashError> {
    let Ok(_quiet) = hold_lock(
        trash_root(journal).join(LOCK_STEM),
        LockOptions {
            timeout: std::time::Duration::ZERO,
            ..LockOptions::default()
        },
    ) else {
        return Ok(Vec::new());
    };
    let mut items: Vec<Item> = all_items(journal)?
        .into_iter()
        .filter(|item| matches!(item.state, State::Detaching | State::Attaching))
        // A linked item waits on its own request's removal, outside the
        // trash's lock: it is moving, not stopped, while that holds it.
        .filter(|item| !(item.linked && linked::running(journal, &item.id)))
        .collect();
    items.sort_by(|left, right| left.trashed_at.cmp(&right.trashed_at));
    Ok(items)
}

/// Items moving now, or stopped part way and waiting for the sweep: on their
/// way into the trash or back out of it. A writer that would write where one
/// of them moves waits for it.
pub fn moving(journal: &Path) -> Result<Vec<Item>, TrashError> {
    // An item whose record can't be read could be one of them.
    Ok(every_item(journal, true)?
        .into_iter()
        .filter(|item| matches!(item.state, State::Detaching | State::Attaching))
        .collect())
}

/// Where an item keeps its own copy of journal-relative `path`, for a kind that
/// needs to read or settle it before it moves back. Inside the trash.
pub fn held_file(journal: &Path, id: &str, path: &str) -> Result<PathBuf, TrashError> {
    if !valid_id(id) {
        return Err(TrashError::NotInTrash);
    }
    trash_entry(journal, &files_rel(id, path))
}

/// Set aside a live journal path into item `id`, as recovering it replaces
/// what is there: for a kind that puts its things back itself, inside its own
/// [`Kind::references`]. The item's removal takes it, and the recover records
/// it erased. Nothing at `path` is a no-op, so a resumed recovery can ask
/// again; something at `path` that was already set aside once is an error.
pub fn set_aside(journal: &Path, item: &Item, path: &str) -> Result<(), TrashError> {
    let live = journal_entry(journal, path)?;
    if !path_lexists(&live)? {
        return Ok(());
    }
    let displaced = displaced_rel(&item.id, path);
    ensure_trash_parent(journal, &displaced)?;
    let aside = trash_entry(journal, &displaced)?;
    if path_lexists(&aside)? {
        return Err(TrashError::Io(format!(
            "item {} already set aside what was at {path}",
            item.id
        )));
    }
    // Recorded before it moves, so a stop between the two leaves no erase
    // unrecorded.
    record_pending(journal, item, Event::Erased);
    move_entry(&live, &aside)?;
    sync_parent(journal, path);
    Ok(())
}

/// Keep `bytes` with item `id` under `name`, for the kind's own attach: what a
/// detach rewrote elsewhere, so attach can put it back. Written once: a
/// resumed detach never replaces what the first one kept. Goes with the item.
pub fn keep(journal: &Path, id: &str, name: &str, bytes: &[u8]) -> Result<(), TrashError> {
    let path = kept_path(journal, id, name)?;
    if path_lexists(&path)? {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| TrashError::Io(error.to_string()))?;
    }
    match write_bytes_exclusive(&path, bytes, AtomicWriteOptions::default()) {
        Ok(()) => Ok(()),
        Err(_) if path_lexists(&path).unwrap_or(false) => Ok(()),
        Err(error) => Err(TrashError::Io(error.to_string())),
    }
}

/// What [`keep`] kept with item `id` under `name`, if anything.
pub fn kept(journal: &Path, id: &str, name: &str) -> Result<Option<Vec<u8>>, TrashError> {
    let path = kept_path(journal, id, name)?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(TrashError::Io(error.to_string())),
    }
}

fn kept_path(journal: &Path, id: &str, name: &str) -> Result<PathBuf, TrashError> {
    if !valid_id(id) || name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(TrashError::NotInTrash);
    }
    trash_entry(journal, &format!("{id}/{KEPT_DIR}/{name}"))
}

/// The journal-relative paths of everything that could still come back out of
/// the trash: every item but one already being erased.
///
/// An item that can't be read is an error, not nothing: what it holds could
/// still come back.
pub fn held_paths(journal: &Path) -> Result<Vec<String>, TrashError> {
    let dir = trash_root(journal);
    let listing = match std::fs::read_dir(&dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(TrashError::Io(error.to_string())),
    };
    let mut paths = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|error| TrashError::Io(error.to_string()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name == SETS_DIR
            || !valid_id(&name)
            || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        if let Some(item) = read_item(journal, &name)?
            && item.state != State::Erasing
        {
            paths.extend(item.paths);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Every item that holds `path` (or a path above it), whatever its state but
/// erasing, other than `except`.
pub fn holders(journal: &Path, path: &str, except: &str) -> Result<Vec<String>, TrashError> {
    let mut found = Vec::new();
    for item in all_items(journal)? {
        if item.id == except || item.state == State::Erasing {
            continue;
        }
        if item
            .paths
            .iter()
            .any(|held| held == path || path.starts_with(&format!("{held}/")))
        {
            found.push(item.id);
        }
    }
    Ok(found)
}

/// One item by id, whatever its state.
pub fn item(journal: &Path, id: &str) -> Result<Option<Item>, TrashError> {
    read_item(journal, id)
}

/// The days the owner set before the trash erases an item, or the default.
/// An unreadable or out-of-range setting reads as the default.
pub fn erase_after_days(journal: &Path) -> u32 {
    let Ok(read) = solstone_core_journal_config::read_journal_config(journal) else {
        // Settings that can't be read must never erase sooner than the owner
        // asked: wait the longest the setting allows.
        return MAX_ERASE_AFTER_DAYS;
    };
    Some(read)
        .and_then(|read| {
            read.config?
                .get(SETTINGS_SECTION)
                .and_then(|section| section.get(ERASE_AFTER_DAYS_KEY))
                .and_then(serde_json::Value::as_u64)
        })
        .and_then(|days| u32::try_from(days).ok())
        .filter(|days| (MIN_ERASE_AFTER_DAYS..=MAX_ERASE_AFTER_DAYS).contains(days))
        .unwrap_or(DEFAULT_ERASE_AFTER_DAYS)
}

/// What one [`sweep`] did.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Swept {
    /// Steps a stop had interrupted, now finished.
    pub resumed: usize,
    /// Items whose time ran out, now erased.
    pub erased: usize,
    /// Items still waiting.
    pub waiting: usize,
    /// Steps that failed and stay for the next sweep, in words for a log.
    pub failed: Vec<String>,
}

/// One thing of a set the owner chose together.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Chosen {
    pub unit: Unit,
    /// Its journal-relative paths, as its kind listed them when the owner
    /// chose it. A kind that can say what the thing is now
    /// ([`Kind::current_paths`]) is asked again when it moves.
    pub paths: Vec<String>,
    /// The owner's own name for it, as [`Item::label`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Where one thing of a set stands.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Entry {
    chosen: Chosen,
    /// The item it becomes, named before anything of it moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    item: Option<String>,
    /// That item has existed. An item gone after this was erased or recovered
    /// by the owner, so nothing more is owed; only one that never arrived is
    /// still to move.
    #[serde(default)]
    arrived: bool,
    /// Nothing more is owed for it: it moved (and was erased if the owner
    /// asked), or it was refused for good.
    #[serde(default)]
    settled: bool,
}

/// A set the owner chose together and confirmed once, kept until every thing
/// in it is settled, so a stop part-way through is finished by [`sweep`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Set {
    schema: u32,
    id: String,
    action: Action,
    #[serde(flatten)]
    surface: Surface,
    /// Erase each now, as well as moving it.
    erase: bool,
    entries: Vec<Entry>,
    /// The operation every item of the set records under, so the history
    /// shows the set as the one act the owner confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
}

/// Where sets wait, inside the trash.
const SETS_DIR: &str = "sets";

/// What became of one thing of a set.
#[derive(Debug)]
pub enum SetResult {
    /// In the trash; `erased` when the owner asked it erased now and it was.
    Moved { item: Box<Item>, erased: bool },
    /// Not moved, and why.
    Refused(TrashError),
}

/// Move a set of things to the trash under one owner act, each as its own
/// item, in the order given. One that is refused never stops the rest. The set
/// is written down first and kept until each thing is settled, so a stop
/// part-way through is finished by [`sweep`]: the owner confirmed the whole
/// set. Results come back in the order of `chosen`.
pub fn detach_set(
    journal: &Path,
    kinds: &[&dyn Kind],
    chosen: Vec<Chosen>,
    action: Action,
    surface: Surface,
    erase: bool,
) -> Result<Vec<SetResult>, TrashError> {
    let at = Utc::now();
    let id = item_id(at);
    let mut set = Set {
        schema: SCHEMA,
        operation: Some(OperationId::durable(at, &id).to_string()),
        id,
        action,
        surface,
        erase,
        entries: chosen
            .into_iter()
            .map(|chosen| Entry {
                chosen,
                item: None,
                arrived: false,
                settled: false,
            })
            .collect(),
    };
    // Held from before the set exists until its run ends, so a sweep never
    // takes a set a request is still running for one a stop left.
    ensure_sets(journal)?;
    let running = set_lock(journal, &set.id, LockOptions::default())?;
    publish_set(journal, &set, true)?;
    let results = run_set(journal, kinds, &mut set, false);
    drop(running);
    remove_set_lock(journal, &set.id);
    Ok(results.into_iter().map(|(_, result)| result).collect())
}

/// Work through what of a set is still owed. `resuming` is a sweep finishing
/// a set a stop interrupted: a thing held for now is left for the next sweep,
/// where a live request reports it refused at once. Returns each entry's
/// result, by index, for the entries this run acted on.
fn run_set(
    journal: &Path,
    kinds: &[&dyn Kind],
    set: &mut Set,
    resuming: bool,
) -> Vec<(usize, SetResult)> {
    let mut results = Vec::new();
    for index in 0..set.entries.len() {
        let Some(entry) = set.entries.get(index).cloned() else {
            continue;
        };
        if entry.settled {
            continue;
        }
        // Its item's id is written down before anything of it moves, so a
        // stop part-way knows which item it became.
        let mut entry = entry;
        if entry.item.is_none() {
            entry.item = Some(item_id(Utc::now()));
            if let Some(slot) = set.entries.get_mut(index) {
                *slot = entry.clone();
            }
            if let Err(error) = publish_set(journal, set, false) {
                log::warn!("trash set {}: its progress wasn't written: {error}", set.id);
            }
        }
        let result = settle(journal, kinds, set, entry, resuming);
        let (entry, result) = result;
        if let Some(slot) = set.entries.get_mut(index) {
            *slot = entry;
        }
        if let Err(error) = publish_set(journal, set, false) {
            log::warn!("trash set {}: its progress wasn't written: {error}", set.id);
        }
        if let Some(result) = result {
            results.push((index, result));
        }
    }
    if set.entries.iter().all(|entry| entry.settled) {
        remove_set(journal, &set.id);
    }
    results
}

/// Move one thing of a set, then erase it if the owner asked. Returns the entry
/// as it now stands and, when this run acted on it, the result.
fn settle(
    journal: &Path,
    kinds: &[&dyn Kind],
    set: &Set,
    mut entry: Entry,
    resuming: bool,
) -> (Entry, Option<SetResult>) {
    let Some(id) = entry.item.clone() else {
        entry.settled = true;
        return (entry, Some(SetResult::Refused(TrashError::NotInTrash)));
    };
    let existing = match read_item(journal, &id) {
        Ok(existing) => existing,
        // An item that can't be read can't be settled by trying again.
        Err(error) => {
            log::warn!("trash set {}: item {id}: {error}", set.id);
            entry.settled = true;
            return (entry, Some(SetResult::Refused(error)));
        }
    };
    let item = match existing {
        Some(item) => {
            entry.arrived = true;
            item
        }
        // It arrived and has gone: the owner recovered or erased it. Moving
        // it again would undo what they just did.
        None if entry.arrived => {
            entry.settled = true;
            return (entry, None);
        }
        // Not moved yet. A stop before this leaves it to do; a thing gone
        // since is refused as missing, so every entry answers.
        None => {
            let kind = match kind_for(kinds, &entry.chosen.unit) {
                Ok(kind) => kind,
                Err(error) => {
                    entry.settled = true;
                    return (entry, Some(SetResult::Refused(error)));
                }
            };
            let paths = kind
                .current_paths(journal, &entry.chosen.unit, &entry.chosen.paths)
                .unwrap_or_else(|| entry.chosen.paths.clone());
            match detach_as(
                journal,
                kind,
                id.clone(),
                entry.chosen.unit.clone(),
                set.action,
                set.surface,
                paths,
                entry.chosen.label.clone(),
                set.operation.clone(),
            ) {
                Ok(_) | Err(TrashError::Unfinished(_)) => {}
                Err(error @ (TrashError::Held(_) | TrashError::InFlight)) if resuming => {
                    // Held for now: the next sweep tries again.
                    return (entry, Some(SetResult::Refused(error)));
                }
                Err(error) => {
                    entry.settled = true;
                    return (entry, Some(SetResult::Refused(error)));
                }
            }
            match read_item(journal, &id) {
                Ok(Some(item)) => {
                    entry.arrived = true;
                    item
                }
                // A detach that found nothing to move keeps no item.
                Ok(None) => {
                    entry.settled = true;
                    let what = describe(&entry.chosen.unit).to_owned();
                    return (entry, Some(SetResult::Refused(TrashError::Missing(what))));
                }
                Err(error) => {
                    entry.settled = true;
                    return (entry, Some(SetResult::Refused(error)));
                }
            }
        }
    };
    if !set.erase {
        entry.settled = item.state != State::Detaching;
        return (
            entry,
            Some(SetResult::Moved {
                item: Box::new(item),
                erased: false,
            }),
        );
    }
    match item.state {
        State::Trashed | State::Erasing => {
            match erase_as(journal, &id, set.action, set.surface, set.operation.clone()) {
                Ok(()) => {
                    entry.settled = true;
                    (
                        entry,
                        Some(SetResult::Moved {
                            item: Box::new(item),
                            erased: true,
                        }),
                    )
                }
                Err(error) => {
                    log::warn!("trash item {id}: erase now was refused: {error}");
                    (
                        entry,
                        Some(SetResult::Moved {
                            item: Box::new(item),
                            erased: false,
                        }),
                    )
                }
            }
        }
        // Still moving in, or being recovered: a sweep settles it later.
        State::Detaching | State::Attaching => (
            entry,
            Some(SetResult::Moved {
                item: Box::new(item),
                erased: false,
            }),
        ),
    }
}

fn set_rel(id: &str) -> String {
    format!("{SETS_DIR}/{id}.json")
}

fn set_lock(journal: &Path, id: &str, options: LockOptions) -> Result<FileLock, TrashError> {
    hold_lock(trash_entry(journal, &set_rel(id))?, options)
        .map_err(|_| TrashError::Held("that set is being moved now".to_owned()))
}

fn ensure_sets(journal: &Path) -> Result<(), TrashError> {
    ensure_trash(journal)?;
    solstone_core_journal_io::paths::create_directory_with_mode(
        &trash_root(journal).join(SETS_DIR),
        0o700,
    )?;
    Ok(())
}

fn publish_set(journal: &Path, set: &Set, new: bool) -> Result<(), TrashError> {
    ensure_sets(journal)?;
    let bytes =
        serde_json::to_vec_pretty(set).map_err(|error| TrashError::Io(error.to_string()))?;
    let path = trash_entry(journal, &set_rel(&set.id))?;
    let options = AtomicWriteOptions { mode: Some(0o600) };
    if new {
        write_bytes_exclusive(&path, &bytes, options)
    } else {
        atomic_replace(path, &bytes, options)
    }
    .map_err(|error| TrashError::Io(error.to_string()))
}

fn remove_set(journal: &Path, id: &str) {
    let removed =
        trash_entry(journal, &set_rel(id)).and_then(|path| match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(TrashError::Io(error.to_string())),
        });
    if let Err(error) = removed {
        log::warn!("trash set {id}: its record was left: {error}");
    }
}

/// A set's run lock leaves a sidecar file; once the set is gone and its lock
/// let go, the sidecar goes too.
fn remove_set_lock(journal: &Path, id: &str) {
    if trash_entry(journal, &set_rel(id)).is_ok_and(|path| path.exists()) {
        return;
    }
    if let Ok(path) = trash_entry(journal, &format!("{}.lock", set_rel(id))) {
        let _ = std::fs::remove_file(path);
    }
}

/// Sets still owed something, oldest first.
fn waiting_sets(journal: &Path) -> Vec<Set> {
    let Ok(listing) = std::fs::read_dir(trash_root(journal).join(SETS_DIR)) else {
        return Vec::new();
    };
    let mut sets: Vec<Set> = listing
        .flatten()
        .filter_map(|entry| {
            let name = entry
                .file_name()
                .to_str()?
                .strip_suffix(".json")?
                .to_owned();
            if !valid_id(&name) {
                return None;
            }
            let bytes = std::fs::read(trash_entry(journal, &set_rel(&name)).ok()?).ok()?;
            serde_json::from_slice::<Set>(&bytes)
                .inspect_err(|error| log::warn!("trash set {name} can't be read: {error}"))
                .ok()
        })
        .collect();
    sets.sort_by(|left, right| left.id.cmp(&right.id));
    sets
}

/// Finish what of each waiting set is owed, skipping a set a live request is
/// running now.
fn sweep_sets(journal: &Path, kinds: &[&dyn Kind], swept: &mut Swept) {
    for set in waiting_sets(journal) {
        let id = set.id.clone();
        sweep_set(journal, kinds, set, swept);
        remove_set_lock(journal, &id);
    }
}

/// Finish one waiting set, unless a live request holds it.
fn sweep_set(journal: &Path, kinds: &[&dyn Kind], mut set: Set, swept: &mut Swept) {
    {
        let Ok(_running) = set_lock(
            journal,
            &set.id,
            LockOptions {
                timeout: std::time::Duration::ZERO,
                ..LockOptions::default()
            },
        ) else {
            return;
        };
        // Read again under the lock: the request may have just finished it.
        let Some(current) = waiting_sets(journal)
            .into_iter()
            .find(|waiting| waiting.id == set.id)
        else {
            return;
        };
        set = current;
        for (_, result) in run_set(journal, kinds, &mut set, true) {
            match result {
                SetResult::Moved { .. } => swept.resumed += 1,
                // Gone already, or settled: nothing to say.
                SetResult::Refused(TrashError::Missing(_)) => {}
                SetResult::Refused(error) => swept.failed.push(format!("set {}: {error}", set.id)),
            }
        }
    }
}

/// Finish every step a stop interrupted, then erase what is due by `now`.
/// Runs at start and on the maintenance schedule.
pub fn sweep(journal: &Path, kinds: &[&dyn Kind], now: DateTime<Utc>) -> Swept {
    let mut swept = Swept::default();
    let items = match all_items(journal) {
        Ok(items) => items,
        Err(error) => {
            swept.failed.push(error.to_string());
            return swept;
        }
    };
    for item in items {
        let result = match item.state {
            State::Detaching => resume_detach(journal, kinds, &item.id).map(|()| {
                swept.resumed += 1;
            }),
            State::Attaching => attach(
                journal,
                kinds,
                &item.id,
                Action::Recover,
                Surface::Policy {
                    policy: solstone_core_removals::Policy::Trash,
                },
            )
            .map(|_| {
                swept.resumed += 1;
            }),
            State::Erasing => {
                let pending = item.pending.clone();
                let (action, surface) = pending
                    .map(|pending| (pending.action, pending.surface))
                    .unwrap_or((Action::Erase, timer_surface()));
                erase(journal, &item.id, action, surface).map(|()| {
                    swept.resumed += 1;
                })
            }
            State::Trashed => match item.erase_at() {
                Some(at) if at <= now => erase(journal, &item.id, Action::Erase, timer_surface())
                    .map(|()| {
                        swept.erased += 1;
                    }),
                Some(_) => {
                    swept.waiting += 1;
                    Ok(())
                }
                None => {
                    swept.waiting += 1;
                    Err(TrashError::Io(format!(
                        "item {} has an unreadable erase date",
                        item.id
                    )))
                }
            },
        };
        if let Err(error) = result {
            swept.failed.push(format!("{}: {error}", item.id));
        }
    }
    // After the items, so a set's thing whose move a stop interrupted is in
    // the trash now and can be erased if the owner asked.
    sweep_sets(journal, kinds, &mut swept);
    linked::tidy_run_locks(journal);
    swept
}

/// The surface of the trash's own timer.
fn timer_surface() -> Surface {
    Surface::Policy {
        policy: solstone_core_removals::Policy::Trash,
    }
}

fn resume_detach(journal: &Path, kinds: &[&dyn Kind], id: &str) -> Result<(), TrashError> {
    let item = read_item(journal, id)?.ok_or(TrashError::NotInTrash)?;
    let kind = kind_for(kinds, &item.unit)?;
    if item.linked {
        return linked::resume(journal, kind, id);
    }
    let hold = kind.hold(journal, &item).map_err(TrashError::Held)?;
    hold.check().map_err(TrashError::Held)?;
    let _lock = lock(journal)?;
    let Some(mut item) = read_item(journal, id)? else {
        return Ok(());
    };
    if item.state != State::Detaching {
        return Ok(());
    }
    finish_detach(journal, kind, &mut item, hold.as_ref())
}

/// Move whatever of `item` is still in place into the trash, then tell the
/// kind, record, and mark it trashed. Safe to run again.
fn finish_detach(
    journal: &Path,
    kind: &dyn Kind,
    item: &mut Item,
    hold: &dyn Hold,
) -> Result<(), TrashError> {
    let mut files = Vec::new();
    for path in &item.paths {
        let trashed = files_rel(&item.id, path);
        let held = trash_entry(journal, &trashed)?;
        if !path_lexists(&held)? {
            let live = journal_entry(journal, path)?;
            if !kind.present(journal, path) {
                // Not in the trash and not in place (or only a marker is
                // there): nothing of it is left to move.
                continue;
            }
            hold.moving(path).map_err(TrashError::Held)?;
            ensure_trash_parent(journal, &trashed)?;
            move_entry(&live, &held)?;
            sync_parent(journal, path);
            sync_trash_parent(journal, &trashed);
        }
        files.extend(tree_files(&held, path));
    }
    if files.is_empty() && !any_moved(journal, item) {
        // Nothing of it was ever moved: there is nothing to keep or record.
        abandon(journal, item);
        return Ok(());
    }
    kind.references(journal, item, Change::Detached, &files)
        .map_err(TrashError::Io)?;
    let at = item.trashed_at().unwrap_or_else(Utc::now);
    removal(
        journal,
        item,
        item.operation.as_deref(),
        at,
        item.action,
        item.surface,
    )
    .record_event(Event::Trashed, item.unit.clone());
    item.state = State::Trashed;
    publish(journal, item)
}

/// Put every path of `item` back, overwriting what is in the way, then tell
/// the kind, record, and remove the item. Safe to run again.
fn finish_attach(
    journal: &Path,
    kind: &dyn Kind,
    item: &Item,
    hold: &dyn Hold,
) -> Result<(), TrashError> {
    let mut overwrote = false;
    let mut files = Vec::new();
    for path in &item.paths {
        let trashed = files_rel(&item.id, path);
        let held = trash_entry(journal, &trashed)?;
        if path_lexists(&held)? {
            hold.moving(path).map_err(TrashError::Held)?;
            // Asked under the path's own hold, so its home can't go between
            // the answer and the move.
            if !kind.returns(journal, item, path) {
                // No home for it now: it goes with the item, and the recover
                // says so.
                if !overwrote {
                    record_pending(journal, item, Event::Erased);
                    overwrote = true;
                }
                continue;
            }
            ensure_journal_parent(journal, path)?;
            let live = journal_entry(journal, path)?;
            if path_lexists(&live)? {
                // Something is where it was: the tombstone it left, or the
                // same thing arriving again. Recovery overwrites: set it
                // aside inside the item, where the item's removal takes it,
                // and the record says so when it was more than a marker.
                let displaced = displaced_rel(&item.id, path);
                ensure_trash_parent(journal, &displaced)?;
                let aside = trash_entry(journal, &displaced)?;
                if path_lexists(&aside)? {
                    return Err(TrashError::Io(format!(
                        "item {} already set aside what was at {path}",
                        item.id
                    )));
                }
                let overwrote = kind.present(journal, path);
                move_entry(&live, &aside)?;
                if overwrote {
                    record_pending(journal, item, Event::Erased);
                }
            }
            move_entry(&held, &live)?;
            sync_parent(journal, path);
        }
        files.extend(tree_files(&journal_entry(journal, path)?, path));
    }
    kind.references(journal, item, Change::Attached, &files)
        .map_err(TrashError::Io)?;
    record_pending(journal, item, Event::Recovered);
    remove_item_dir(journal, item)
}

/// Record `event` for an item under the owner act it is attaching or erasing for.
fn record_pending(journal: &Path, item: &Item, event: Event) {
    if let Some(pending) = &item.pending {
        let at = DateTime::parse_from_rfc3339(&pending.at)
            .map(|at| at.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        removal(
            journal,
            item,
            pending.operation.as_deref(),
            at,
            pending.action,
            pending.surface,
        )
        .record_event(event, item.unit.clone());
    }
}

/// The removal an item's step records through: under the owner act's
/// operation when it spans several items, each entry then named by its item;
/// otherwise under the item's own, which already names it.
fn removal(
    journal: &Path,
    item: &Item,
    shared: Option<&str>,
    at: DateTime<Utc>,
    action: Action,
    surface: Surface,
) -> Removal {
    match shared {
        Some(operation) => {
            Removal::with_id(journal, OperationId::recorded(operation), action, surface)
                .for_item(&item.id)
        }
        None => Removal::with_id(journal, OperationId::durable(at, &item.id), action, surface),
    }
}

/// Unlink the item's files, record, and remove the item. Safe to run again.
fn finish_erase(journal: &Path, item: &Item) -> Result<(), TrashError> {
    let pending = item.pending.clone().unwrap_or(Pending {
        action: Action::Erase,
        surface: timer_surface(),
        at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        operation: None,
    });
    let at = DateTime::parse_from_rfc3339(&pending.at)
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let files = trash_root(journal).join(&item.id).join(FILES_DIR);
    removal(
        journal,
        item,
        pending.operation.as_deref(),
        at,
        pending.action,
        pending.surface,
    )
    .commit_event(Event::Erased, item.unit.clone(), |io| {
        io.remove_contained_tree(&files)
    })
    .map_err(|error| TrashError::Io(error.to_string()))?;
    remove_item_dir(journal, item)
}

/// Remove an item's own folder once its files are gone or back: its record,
/// and anything attach set aside.
fn remove_item_dir(journal: &Path, item: &Item) -> Result<(), TrashError> {
    // The item record goes last, so a stop before it leaves a step that a
    // sweep finishes.
    let dir = trash_root(journal).join(&item.id);
    for part in [FILES_DIR, DISPLACED_DIR] {
        #[expect(
            clippy::disallowed_methods,
            reason = "system internal: the trash removing its own item folder after the item's files were erased or put back; what attach set aside was in the recovered item's place, which recovery overwrites by the owner's act"
        )]
        solstone_core_journal_io::removal::remove_contained_tree(journal, &dir.join(part))?;
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "system internal: the trash removing its own item folder, now holding only its record"
    )]
    solstone_core_journal_io::removal::remove_contained_tree(journal, &dir)?;
    sync_trash_root(journal);
    Ok(())
}

fn kind_for<'k>(kinds: &[&'k dyn Kind], unit: &Unit) -> Result<&'k dyn Kind, TrashError> {
    kinds
        .iter()
        .copied()
        .find(|kind| kind.handles(unit))
        .ok_or(TrashError::UnknownKind)
}

fn describe(unit: &Unit) -> &'static str {
    match unit {
        Unit::Memory { .. } => "memory",
        Unit::Segment { .. } => "recording",
        Unit::Import { .. } => "import",
        Unit::Workout { .. } => "workout",
        Unit::Originals { .. } => "original",
        _ => "thing",
    }
}

fn item_id(at: DateTime<Utc>) -> String {
    let mut bytes = [0_u8; 4];
    if getrandom::fill(&mut bytes).is_err() {
        bytes = std::process::id().to_be_bytes();
    }
    format!(
        "{}-{:02x}{:02x}{:02x}{:02x}",
        at.format("%Y%m%dT%H%M%S%3fZ"),
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3]
    )
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

/// Where the trash is. Paths inside it are resolved against it as their own
/// root: the journal's resolver refuses the trash for everyone else.
fn trash_root(journal: &Path) -> PathBuf {
    journal.join(TRASH_DIR)
}

/// `rel` is relative to the trash folder.
fn files_rel(id: &str, path: &str) -> String {
    format!("{id}/{FILES_DIR}/{path}")
}

fn displaced_rel(id: &str, path: &str) -> String {
    format!("{id}/{DISPLACED_DIR}/{path}")
}

/// An entry inside the trash, its parent resolved and contained, its leaf not
/// followed.
fn trash_entry(journal: &Path, rel: &str) -> Result<PathBuf, TrashError> {
    entry_below(&trash_root(journal), rel)
}

/// An entry in the journal, its parent resolved and contained, its leaf not
/// followed.
fn journal_entry(journal: &Path, rel: &str) -> Result<PathBuf, TrashError> {
    entry_below(journal, rel)
}

fn entry_below(root: &Path, rel: &str) -> Result<PathBuf, TrashError> {
    match rel.rsplit_once('/') {
        Some((parent, leaf)) => Ok(contained_path(root, parent)?.join(leaf)),
        None => Ok(root
            .canonicalize()
            .map_err(|error| TrashError::Io(error.to_string()))?
            .join(rel)),
    }
}

fn ensure_trash(journal: &Path) -> Result<(), TrashError> {
    solstone_core_journal_io::paths::create_directory_with_mode(&trash_root(journal), 0o700)?;
    Ok(())
}

fn lock(journal: &Path) -> Result<FileLock, TrashError> {
    ensure_trash(journal)?;
    hold_lock(trash_root(journal).join(LOCK_STEM), LockOptions::default())
        .map_err(|_| TrashError::Held("your trash is busy; try again".to_owned()))
}

fn ensure_parent_below(root: &Path, rel: &str) -> Result<(), TrashError> {
    if let Some((parent, _)) = rel.rsplit_once('/') {
        // Resolve the deepest existing ancestor, then create below it.
        let lexical = root.join(parent);
        solstone_core_journal_io::paths::ensure_directory(&lexical)?;
        contained_path(root, parent)?;
    }
    Ok(())
}

fn ensure_trash_parent(journal: &Path, rel: &str) -> Result<(), TrashError> {
    ensure_parent_below(&trash_root(journal), rel)
}

fn ensure_journal_parent(journal: &Path, rel: &str) -> Result<(), TrashError> {
    ensure_parent_below(journal, rel)
}

fn sync_parent(journal: &Path, rel: &str) {
    if let Some((parent, _)) = rel.rsplit_once('/')
        && let Err(error) = sync_dir(journal, parent)
    {
        log::warn!("trash: {parent} was not flushed: {error}");
    }
}

/// Flush the trash folder's own entries. Windows has no directory flush.
fn sync_trash_root(journal: &Path) {
    #[cfg(not(windows))]
    if let Err(error) = std::fs::File::open(trash_root(journal)).and_then(|dir| dir.sync_all()) {
        log::warn!("trash: the trash folder was not flushed: {error}");
    }
    #[cfg(windows)]
    let _ = journal;
}

fn sync_trash_parent(journal: &Path, rel: &str) {
    sync_parent(&trash_root(journal), rel);
}

/// Rename one entry to another place in the journal's volume, never over
/// something.
///
/// Windows refuses to rename a directory while a reader holds a file below
/// it, so a short read is waited out, for up to ten seconds; a move is never
/// replaced by an erase.
fn move_entry(from: &Path, to: &Path) -> Result<(), TrashError> {
    let mut retries: u16 = if cfg!(windows) { 400 } else { 0 };
    loop {
        if path_lexists(to)? {
            return Err(TrashError::Io(format!("{} is already taken", to.display())));
        }
        // Both ends were resolved and contained just before.
        let renamed = std::fs::rename(from, to);
        match renamed {
            Ok(()) => return Ok(()),
            Err(error) if retries > 0 && matches!(error.raw_os_error(), Some(5 | 32)) => {
                retries = retries.saturating_sub(1);
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) => {
                return Err(TrashError::Io(format!("{}: {error}", from.display())));
            }
        }
    }
}

fn read_item(journal: &Path, id: &str) -> Result<Option<Item>, TrashError> {
    if !valid_id(id) {
        return Ok(None);
    }
    let path = trash_entry(journal, &format!("{id}/{ITEM_FILE}"))?;
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| TrashError::Io(format!("item {id}: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(TrashError::Io(error.to_string())),
    }
}

fn all_items(journal: &Path) -> Result<Vec<Item>, TrashError> {
    every_item(journal, false)
}

/// Every item, an unreadable record an error when `strict`: for a reader whose
/// answer must not miss one.
fn every_item(journal: &Path, strict: bool) -> Result<Vec<Item>, TrashError> {
    let dir = trash_root(journal);
    let listing = match std::fs::read_dir(&dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(TrashError::Io(error.to_string())),
    };
    let mut items = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|error| TrashError::Io(error.to_string()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !valid_id(&name) || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        match read_item(journal, &name) {
            Ok(Some(item)) => items.push(item),
            // A folder with no record is a detach that stopped before its
            // record was published: nothing has moved into it.
            Ok(None) => {}
            Err(error) if strict => return Err(error),
            Err(error) => log::warn!("trash: {error}"),
        }
    }
    Ok(items)
}

fn item_file(journal: &Path, item: &Item) -> Result<PathBuf, TrashError> {
    trash_entry(journal, &format!("{}/{ITEM_FILE}", item.id))
}

fn publish_new(journal: &Path, item: &Item) -> Result<(), TrashError> {
    let bytes =
        serde_json::to_vec_pretty(item).map_err(|error| TrashError::Io(error.to_string()))?;
    write_bytes_exclusive(
        &item_file(journal, item)?,
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| TrashError::Io(error.to_string()))
}

fn publish(journal: &Path, item: &Item) -> Result<(), TrashError> {
    let bytes =
        serde_json::to_vec_pretty(item).map_err(|error| TrashError::Io(error.to_string()))?;
    atomic_replace(
        item_file(journal, item)?,
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| TrashError::Io(error.to_string()))
}

/// Every file below `path`, named as it reads below `named`.
fn tree_files(path: &Path, named: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![(path.to_path_buf(), named.to_owned())];
    while let Some((path, name)) = stack.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            if let Ok(listing) = std::fs::read_dir(&path) {
                for entry in listing.flatten() {
                    if let Some(child) = entry.file_name().to_str() {
                        stack.push((entry.path(), format!("{name}/{child}")));
                    }
                }
            }
        } else {
            found.push(name);
        }
    }
    found.sort();
    found
}

fn tree_bytes(path: &Path) -> u64 {
    let mut total = 0_u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            if let Ok(listing) = std::fs::read_dir(&path) {
                stack.extend(listing.flatten().map(|entry| entry.path()));
            }
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    total
}

impl fmt::Display for State {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Detaching => "detaching",
            Self::Trashed => "trashed",
            Self::Attaching => "attaching",
            Self::Erasing => "erasing",
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]
mod tests {
    use super::*;
    use solstone_core_removals::Page;
    use std::cell::RefCell;
    use std::fs;

    struct Free<'a>(&'a std::cell::Cell<u32>);
    impl Hold for Free<'_> {
        fn check(&self) -> Result<(), String> {
            // Counts down to a refusal when a test arms it.
            match self.0.get() {
                0 => Ok(()),
                1 => Err("held".into()),
                n => {
                    self.0.set(n - 1);
                    Ok(())
                }
            }
        }
    }

    #[derive(Default)]
    struct Fake {
        seen: RefCell<Vec<(Change, Vec<String>)>>,
        fail_references: std::cell::Cell<bool>,
        refuse_check_at: std::cell::Cell<u32>,
        companion: bool,
        homeless: Vec<String>,
        aside: Vec<String>,
    }

    impl Kind for Fake {
        fn handles(&self, unit: &Unit) -> bool {
            matches!(unit, Unit::Segment { .. })
        }
        fn hold<'a>(&'a self, _: &Path, _: &Item) -> Result<Box<dyn Hold + 'a>, String> {
            Ok(Box::new(Free(&self.refuse_check_at)))
        }
        fn present(&self, journal: &Path, path: &str) -> bool {
            let place = journal.join(path);
            place.is_file() || (place.is_dir() && !place.join("tombstone.json").exists())
        }
        fn returns(&self, _: &Path, _: &Item, path: &str) -> bool {
            !self.homeless.iter().any(|gone| gone == path)
        }
        fn companions(&self, _: &Path, _: &Item) -> Result<Vec<String>, String> {
            Ok(if self.companion {
                vec!["config/private-note.json".into()]
            } else {
                Vec::new()
            })
        }
        fn references(
            &self,
            journal: &Path,
            item: &Item,
            change: Change,
            files: &[String],
        ) -> Result<(), String> {
            if self.fail_references.get() {
                return Err("synthetic failure".into());
            }
            if change == Change::Attached {
                for path in &self.aside {
                    set_aside(journal, item, path).map_err(|error| error.to_string())?;
                }
            }
            if change == Change::Detached {
                let place = journal.join(SEGMENT);
                fs::create_dir_all(&place).unwrap();
                let _ = fs::write(place.join("tombstone.json"), b"{}");
            }
            self.seen.borrow_mut().push((change, files.to_vec()));
            Ok(())
        }
    }

    const SEGMENT: &str = "chronicle/20261108/phone/070000_300";

    #[test]
    fn companion_paths_survive_an_interrupted_detach_and_recovery() {
        let (_dir, root) = journal();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config/private-note.json"), b"private text").unwrap();
        let kind = Fake {
            companion: true,
            ..Fake::default()
        };
        kind.fail_references.set(true);
        assert!(matches!(
            detach(
                &root,
                &kind,
                unit(),
                Action::SegmentDelete,
                web(),
                vec![SEGMENT.into()]
            ),
            Err(TrashError::Unfinished(_))
        ));
        let item = all_items(&root).unwrap().remove(0);
        assert_eq!(item.state, State::Detaching);
        assert!(item.paths.contains(&"config/private-note.json".into()));
        assert!(!root.join("config/private-note.json").exists());
        kind.fail_references.set(false);
        let swept = sweep(&root, &[&kind], Utc::now());
        assert_eq!(swept.resumed, 1);
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        assert_eq!(
            fs::read(root.join("config/private-note.json")).unwrap(),
            b"private text"
        );
    }

    fn journal() -> (tempfile_dir::Dir, PathBuf) {
        let dir = tempfile_dir::Dir::new();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join(SEGMENT)).unwrap();
        fs::write(root.join(SEGMENT).join("audio.flac"), b"sound").unwrap();
        fs::write(root.join(SEGMENT).join("transcript.jsonl"), b"{}\n").unwrap();
        (dir, root)
    }

    fn unit() -> Unit {
        Unit::Segment {
            day: "20261108".into(),
            stream: "phone".into(),
            segment: "070000_300".into(),
        }
    }

    fn web() -> Surface {
        Surface::Web {
            page: Page::Transcripts,
        }
    }

    fn events(root: &Path) -> Vec<(Event, Action)> {
        let mut found: Vec<_> = solstone_core_removals::history(root, 100)
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| (entry.event, entry.action))
            .collect();
        found.sort_by_key(|(event, _)| format!("{event:?}"));
        found
    }

    fn trash(root: &Path, kind: &Fake) -> Item {
        detach(
            root,
            kind,
            unit(),
            Action::SegmentDelete,
            web(),
            vec![SEGMENT.into()],
        )
        .unwrap()
    }

    #[test]
    fn trashing_moves_the_recording_whole_and_records_it() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        assert!(!root.join(SEGMENT).join("audio.flac").exists());
        let held = root
            .join(TRASH_DIR)
            .join(&item.id)
            .join("files")
            .join(SEGMENT);
        assert!(
            root.join(TRASH_DIR)
                .join(&item.id)
                .join("item.json")
                .is_file()
        );
        assert_eq!(fs::read(held.join("audio.flac")).unwrap(), b"sound");
        assert_eq!(item.state, State::Trashed);
        assert_eq!(item.bytes, 8);
        assert_eq!(list(&root).unwrap().len(), 1);
        let seen = kind.seen.borrow();
        assert_eq!(seen[0].0, Change::Detached);
        assert_eq!(
            seen[0].1,
            vec![
                format!("{SEGMENT}/audio.flac"),
                format!("{SEGMENT}/transcript.jsonl")
            ]
        );
        assert_eq!(events(&root), vec![(Event::Trashed, Action::SegmentDelete)]);
    }

    #[test]
    fn a_missing_recording_is_refused_and_leaves_nothing() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let error = detach(
            &root,
            &kind,
            unit(),
            Action::SegmentDelete,
            web(),
            vec!["chronicle/20261108/phone/080000_300".into()],
        )
        .unwrap_err();
        assert!(matches!(error, TrashError::Missing(_)));
        assert!(list(&root).unwrap().is_empty());
        assert!(events(&root).is_empty());
    }

    #[test]
    fn recovering_puts_it_back_exactly_and_records_it() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        assert_eq!(
            fs::read(root.join(SEGMENT).join("audio.flac")).unwrap(),
            b"sound"
        );
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
        assert!(list(&root).unwrap().is_empty());
        assert_eq!(kind.seen.borrow()[1].0, Change::Attached);
        assert_eq!(
            events(&root),
            vec![
                (Event::Recovered, Action::Recover),
                (Event::Trashed, Action::SegmentDelete)
            ]
        );
    }

    #[test]
    fn recovering_overwrites_what_arrived_in_its_place() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        // The same recording arrives again: its tombstone gives way to it.
        fs::remove_file(root.join(SEGMENT).join("tombstone.json")).unwrap();
        fs::write(root.join(SEGMENT).join("audio.flac"), b"again").unwrap();
        fs::write(root.join(SEGMENT).join("extra.json"), b"{}").unwrap();
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        assert_eq!(
            fs::read(root.join(SEGMENT).join("audio.flac")).unwrap(),
            b"sound"
        );
        assert!(!root.join(SEGMENT).join("extra.json").exists());
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
        // The copy it replaced is recorded as erased by the recover.
        assert!(events(&root).contains(&(Event::Erased, Action::Recover)));
    }

    #[test]
    fn a_kind_putting_its_things_back_sets_aside_what_it_replaces() {
        let (_dir, root) = journal();
        let kept = "facets/work/notes.md";
        let kind = Fake {
            aside: vec![kept.into(), "facets/work/absent.md".into()],
            ..Fake::default()
        };
        let item = trash(&root, &kind);
        fs::create_dir_all(root.join("facets/work")).unwrap();
        fs::write(root.join(kept), b"what the merge kept").unwrap();
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        assert!(!root.join(kept).exists());
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
        assert!(events(&root).contains(&(Event::Erased, Action::Recover)));
    }

    #[test]
    fn a_path_with_no_home_goes_with_the_item_instead_of_coming_back() {
        let (_dir, root) = journal();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config/private-note.json"), b"private text").unwrap();
        let mut kind = Fake {
            companion: true,
            ..Fake::default()
        };
        let item = trash(&root, &kind);
        kind.homeless = vec!["config/private-note.json".into()];
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        assert!(root.join(SEGMENT).join("audio.flac").exists());
        assert!(!root.join("config/private-note.json").exists());
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
        // Nothing leaves without the record saying so.
        assert!(events(&root).contains(&(Event::Erased, Action::Recover)));
    }

    #[test]
    fn what_a_kind_keeps_is_written_once_and_goes_with_the_item() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        keep(&root, &item.id, "before.json", b"first").unwrap();
        keep(&root, &item.id, "before.json", b"second").unwrap();
        assert_eq!(
            kept(&root, &item.id, "before.json").unwrap().unwrap(),
            b"first"
        );
        assert!(kept(&root, &item.id, "other.json").unwrap().is_none());
        assert!(keep(&root, &item.id, "../escape", b"x").is_err());
        erase(&root, &item.id, Action::Erase, web()).unwrap();
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
    }

    #[test]
    fn a_stopped_step_is_listed_only_while_nothing_is_moving() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        kind.fail_references.set(true);
        let Err(TrashError::Unfinished(id)) = detach(
            &root,
            &kind,
            unit(),
            Action::SegmentDelete,
            web(),
            vec![SEGMENT.into()],
        ) else {
            panic!("the detach should stop after the move");
        };
        assert_eq!(unfinished(&root).unwrap(), 1);
        let listed = stopped(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].state, State::Detaching);
        // A step moving now holds the lock: nothing is shown as stopped.
        let _moving = lock(&root).unwrap();
        assert!(stopped(&root).unwrap().is_empty());
        assert_eq!(unfinished(&root).unwrap(), 1);
    }

    #[test]
    fn recovering_over_its_own_tombstone_records_no_erase() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        assert!(
            !events(&root)
                .iter()
                .any(|(event, _)| *event == Event::Erased)
        );
    }

    /// A refusal before anything moved leaves the recording in place and no
    /// item for a sweep to finish later.
    #[test]
    fn a_detach_refused_before_the_move_leaves_nothing_behind() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        kind.refuse_check_at.set(2);
        let error = detach(
            &root,
            &kind,
            unit(),
            Action::SegmentDelete,
            web(),
            vec![SEGMENT.into()],
        )
        .unwrap_err();
        assert!(matches!(error, TrashError::Held(_)), "{error:?}");
        assert!(root.join(SEGMENT).join("audio.flac").is_file());
        assert_eq!(unfinished(&root).unwrap(), 0);
        assert!(events(&root).is_empty());
        kind.refuse_check_at.set(0);
        sweep(&root, &[&kind], Utc::now());
        assert!(
            root.join(SEGMENT).join("audio.flac").is_file(),
            "no later sweep moves it"
        );
    }

    /// A failure after the move says so, never "nothing was deleted", and the
    /// sweep finishes the step once its references can be healed.
    #[test]
    fn a_detach_that_fails_after_the_move_is_finished_by_the_sweep() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        kind.fail_references.set(true);
        let error = detach(
            &root,
            &kind,
            unit(),
            Action::SegmentDelete,
            web(),
            vec![SEGMENT.into()],
        )
        .unwrap_err();
        let TrashError::Unfinished(id) = error else {
            panic!("expected unfinished, got {error:?}");
        };
        assert!(!root.join(SEGMENT).join("audio.flac").exists());
        assert!(list(&root).unwrap().is_empty());
        assert_eq!(unfinished(&root).unwrap(), 1);
        assert!(
            events(&root).is_empty(),
            "nothing recorded before it is settled"
        );
        kind.fail_references.set(false);
        let swept = sweep(&root, &[&kind], Utc::now());
        assert_eq!(swept.resumed, 1, "{swept:?}");
        assert_eq!(list(&root).unwrap()[0].id, id);
        assert!(root.join(SEGMENT).join("tombstone.json").is_file());
        assert_eq!(events(&root), vec![(Event::Trashed, Action::SegmentDelete)]);
    }

    /// Only the marker a removal left is where it was: there is nothing to
    /// trash, from any surface.
    #[test]
    fn a_removed_recording_is_not_trashed_again() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        trash(&root, &kind);
        let again = detach(
            &root,
            &kind,
            unit(),
            Action::RetentionTool,
            Surface::Cli,
            vec![SEGMENT.into()],
        );
        assert!(matches!(again, Err(TrashError::Missing(_))));
        assert_eq!(list(&root).unwrap().len(), 1);
        assert_eq!(events(&root).len(), 1);
    }

    /// An item that never moved anything, whose place another removal has
    /// since emptied to its tombstone, is dropped by the sweep, not filled
    /// with that tombstone.
    #[test]
    fn a_stale_detach_never_takes_another_removals_tombstone() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let other = trash(&root, &kind);
        let stale = Item {
            schema: SCHEMA,
            id: item_id(Utc::now() + Duration::seconds(1)),
            state: State::Detaching,
            unit: unit(),
            action: Action::SegmentDelete,
            surface: web(),
            trashed_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            erase_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            paths: vec![SEGMENT.into()],
            bytes: 8,
            pending: None,
            label: None,
            refs: Vec::new(),
            operation: None,
            linked: false,
        };
        fs::create_dir(root.join(TRASH_DIR).join(&stale.id)).unwrap();
        publish_new(&root, &stale).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert!(root.join(SEGMENT).join("tombstone.json").is_file());
        assert!(!root.join(TRASH_DIR).join(&stale.id).exists());
        assert_eq!(list(&root).unwrap().len(), 1);
        assert_eq!(list(&root).unwrap()[0].id, other.id);
        assert_eq!(events(&root).len(), 1);
    }

    #[test]
    fn unreadable_settings_never_erase_sooner_than_asked() {
        let (_dir, root) = journal();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config/journal.json"), b"{not json").unwrap();
        assert_eq!(erase_after_days(&root), MAX_ERASE_AFTER_DAYS);
    }

    #[test]
    fn erasing_unlinks_and_records_it() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        erase(&root, &item.id, Action::Erase, web()).unwrap();
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
        assert!(!root.join(SEGMENT).join("audio.flac").exists());
        assert!(matches!(
            attach(&root, &[&kind], &item.id, Action::Recover, web()),
            Err(TrashError::NotInTrash)
        ));
        assert_eq!(
            events(&root),
            vec![
                (Event::Erased, Action::Erase),
                (Event::Trashed, Action::SegmentDelete)
            ]
        );
    }

    #[test]
    fn the_timer_erases_only_what_has_waited_long_enough() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        let trashed = item.trashed_at().unwrap();
        assert_eq!(item.erase_at().unwrap(), trashed + Duration::days(7));
        let early = sweep(&root, &[&kind], trashed + Duration::days(6));
        assert_eq!((early.erased, early.waiting), (0, 1));
        let due = sweep(&root, &[&kind], trashed + Duration::days(7));
        assert_eq!((due.erased, due.waiting), (1, 0));
        assert!(list(&root).unwrap().is_empty());
        let history = solstone_core_removals::history(&root, 10).unwrap();
        let erased = history
            .entries
            .iter()
            .find(|entry| entry.event == Event::Erased)
            .unwrap();
        assert_eq!(
            erased.surface,
            Surface::Policy {
                policy: solstone_core_removals::Policy::Trash
            }
        );
    }

    /// A stop after the move but before the item was marked trashed: the
    /// recording is wholly in the trash, and a sweep finishes the step.
    #[test]
    fn a_detach_stopped_after_the_move_is_finished_by_a_sweep() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let mut item = trash(&root, &kind);
        item.state = State::Detaching;
        publish(&root, &item).unwrap();
        assert!(list(&root).unwrap().is_empty());
        let swept = sweep(&root, &[&kind], Utc::now());
        assert_eq!(swept.resumed, 1);
        assert_eq!(list(&root).unwrap().len(), 1);
        assert!(!root.join(SEGMENT).join("audio.flac").exists());
        // Recorded once, whatever the number of runs.
        assert_eq!(events(&root).len(), 1);
    }

    fn chosen() -> Chosen {
        Chosen {
            unit: unit(),
            paths: vec![SEGMENT.into()],
            label: Some("the owner's name".into()),
        }
    }

    fn set_of(erase: bool) -> Set {
        Set {
            schema: SCHEMA,
            id: item_id(Utc::now()),
            action: Action::SegmentDelete,
            surface: web(),
            erase,
            entries: vec![Entry {
                chosen: chosen(),
                item: None,
                arrived: false,
                settled: false,
            }],
            operation: None,
        }
    }

    /// A set stopped after it was written down and before anything moved: the
    /// sweep moves what the owner confirmed, then forgets the set.
    #[test]
    fn a_set_stopped_before_anything_moved_is_finished_by_a_sweep() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        publish_set(&root, &set_of(false), true).unwrap();
        let swept = sweep(&root, &[&kind], Utc::now());
        assert_eq!(swept.resumed, 1);
        let items = list(&root).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label.as_deref(), Some("the owner's name"));
        assert!(waiting_sets(&root).is_empty());
        // A second sweep has nothing left of it to do.
        assert_eq!(sweep(&root, &[&kind], Utc::now()).resumed, 0);
    }

    /// A set stopped after its thing moved but before the set said so: the
    /// sweep finds it gone, moves nothing twice, and forgets the set.
    #[test]
    fn a_set_stopped_after_its_thing_moved_moves_nothing_twice() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        publish_set(&root, &set_of(false), true).unwrap();
        trash(&root, &kind);
        let swept = sweep(&root, &[&kind], Utc::now());
        assert_eq!(swept.resumed, 0);
        assert!(swept.failed.is_empty());
        assert_eq!(list(&root).unwrap().len(), 1);
        assert!(waiting_sets(&root).is_empty());
    }

    /// The owner asked the set erased; a stop came after the move and before
    /// the erase. The sweep erases it.
    #[test]
    fn a_set_asked_erased_is_erased_after_a_stop() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        let mut set = set_of(true);
        set.entries[0].item = Some(item.id.clone());
        publish_set(&root, &set, true).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert!(list(&root).unwrap().is_empty());
        assert!(waiting_sets(&root).is_empty());
        assert!(
            events(&root)
                .iter()
                .any(|(event, _)| *event == Event::Erased)
        );
    }

    /// The stop came mid-move: the item is on its way in, and the set knows
    /// which it is. The sweep finishes the move and then the erase.
    #[test]
    fn a_set_asked_erased_is_erased_after_a_stop_mid_move() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let mut item = trash(&root, &kind);
        item.state = State::Detaching;
        publish(&root, &item).unwrap();
        let mut set = set_of(true);
        set.entries[0].item = Some(item.id.clone());
        publish_set(&root, &set, true).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert!(list(&root).unwrap().is_empty());
        assert!(all_items(&root).unwrap().is_empty());
        assert!(waiting_sets(&root).is_empty());
    }

    /// The stop came after the set named the item and before it existed.
    #[test]
    fn a_set_asked_erased_is_erased_when_stopped_before_its_item_exists() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let mut set = set_of(true);
        set.entries[0].item = Some(item_id(Utc::now()));
        publish_set(&root, &set, true).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert!(all_items(&root).unwrap().is_empty());
        assert!(!root.join(SEGMENT).join("audio.flac").exists());
        assert!(waiting_sets(&root).is_empty());
    }

    /// The owner recovered it while the set still owed its erase: a sweep
    /// leaves it alone.
    #[test]
    fn a_set_never_moves_again_what_the_owner_took_back() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        let mut set = set_of(true);
        set.entries[0].item = Some(item.id.clone());
        set.entries[0].arrived = true;
        publish_set(&root, &set, true).unwrap();
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert!(root.join(SEGMENT).join("audio.flac").exists());
        assert!(all_items(&root).unwrap().is_empty());
        assert!(waiting_sets(&root).is_empty());
    }

    /// A recover a stop interrupted is finished first, and the set leaves it.
    #[test]
    fn a_set_leaves_alone_a_recover_a_stop_interrupted() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let mut item = trash(&root, &kind);
        item.state = State::Attaching;
        item.pending = Some(Pending {
            action: Action::Recover,
            surface: web(),
            at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            operation: None,
        });
        publish(&root, &item).unwrap();
        let mut set = set_of(true);
        set.entries[0].item = Some(item.id.clone());
        set.entries[0].arrived = true;
        publish_set(&root, &set, true).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert!(root.join(SEGMENT).join("audio.flac").exists());
        assert!(waiting_sets(&root).is_empty());
    }

    /// The stop came after the item's folder and before its record.
    #[test]
    fn a_set_moves_a_thing_whose_item_folder_has_no_record_yet() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let id = item_id(Utc::now());
        fs::create_dir_all(root.join(TRASH_DIR).join(&id)).unwrap();
        let mut set = set_of(false);
        set.entries[0].item = Some(id.clone());
        publish_set(&root, &set, true).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert_eq!(item(&root, &id).unwrap().unwrap().state, State::Trashed);
        assert!(!root.join(SEGMENT).join("audio.flac").exists());
        assert!(waiting_sets(&root).is_empty());
    }

    /// Every thing of a set answers, a thing already gone included, in order.
    #[test]
    fn a_set_answers_once_for_each_thing_it_was_given() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let gone = Chosen {
            unit: unit(),
            paths: vec!["chronicle/20261108/phone/080000_300".into()],
            label: None,
        };
        let results = detach_set(
            &root,
            &[&kind],
            vec![gone, chosen()],
            Action::SegmentDelete,
            web(),
            false,
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        assert!(matches!(
            results[0],
            SetResult::Refused(TrashError::Missing(_))
        ));
        assert!(matches!(results[1], SetResult::Moved { .. }));
    }

    /// A thing held on resume stays owed, and the next sweep moves it.
    #[test]
    fn a_set_thing_held_on_resume_is_moved_by_a_later_sweep() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        publish_set(&root, &set_of(false), true).unwrap();
        kind.refuse_check_at.set(1);
        let swept = sweep(&root, &[&kind], Utc::now());
        assert_eq!(swept.resumed, 0);
        assert_eq!(waiting_sets(&root).len(), 1);
        kind.refuse_check_at.set(0);
        sweep(&root, &[&kind], Utc::now());
        assert_eq!(list(&root).unwrap().len(), 1);
        assert!(waiting_sets(&root).is_empty());
    }

    /// A sweep leaves alone a set a live request is running.
    #[test]
    fn a_sweep_skips_a_set_a_request_is_running() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let set = set_of(false);
        publish_set(&root, &set, true).unwrap();
        let running = set_lock(&root, &set.id, LockOptions::default()).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert!(list(&root).unwrap().is_empty());
        drop(running);
        sweep(&root, &[&kind], Utc::now());
        assert_eq!(list(&root).unwrap().len(), 1);
        // Nothing of the set is left behind, its lock's sidecar included.
        let sets = std::fs::read_dir(root.join(TRASH_DIR).join(SETS_DIR))
            .unwrap()
            .count();
        assert_eq!(sets, 0);
    }

    /// A set the owner asked erased is erased, each thing as its own item.
    #[test]
    fn a_set_asked_erased_leaves_nothing_in_the_trash() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let results = detach_set(
            &root,
            &[&kind],
            vec![chosen()],
            Action::SegmentDelete,
            web(),
            true,
        )
        .unwrap();
        assert!(matches!(results[0], SetResult::Moved { erased: true, .. }));
        assert!(list(&root).unwrap().is_empty());
        assert!(waiting_sets(&root).is_empty());
    }

    /// A kind whose unit says nothing of which thing it is, as an entity's.
    #[derive(Default)]
    struct Plain(std::cell::Cell<u32>);

    impl Kind for Plain {
        fn handles(&self, unit: &Unit) -> bool {
            matches!(unit, Unit::Entity)
        }
        fn hold<'a>(&'a self, _: &Path, _: &Item) -> Result<Box<dyn Hold + 'a>, String> {
            Ok(Box::new(Free(&self.0)))
        }
        fn present(&self, journal: &Path, path: &str) -> bool {
            journal.join(path).exists()
        }
        fn references(&self, _: &Path, _: &Item, _: Change, _: &[String]) -> Result<(), String> {
            Ok(())
        }
    }

    const PEOPLE: [&str; 2] = ["entities/alice", "entities/bob"];

    fn people(root: &Path) -> Vec<Chosen> {
        PEOPLE
            .iter()
            .map(|path| {
                fs::create_dir_all(root.join(path)).unwrap();
                fs::write(root.join(path).join("entity.json"), b"{}").unwrap();
                Chosen {
                    unit: Unit::Entity,
                    paths: vec![(*path).into()],
                    label: None,
                }
            })
            .collect()
    }

    fn entries(root: &Path) -> Vec<solstone_core_removals::Entry> {
        solstone_core_removals::history(root, 100).unwrap().entries
    }

    fn one_operation(entries: &[solstone_core_removals::Entry]) -> String {
        let operations: std::collections::BTreeSet<&str> = entries
            .iter()
            .map(|entry| entry.operation.as_str())
            .collect();
        assert_eq!(operations.len(), 1, "{operations:?}");
        operations.into_iter().next().unwrap().to_owned()
    }

    /// Two things whose units read the same, moved as one set: two entries,
    /// under the set's one operation.
    #[test]
    fn a_set_records_each_thing_under_its_one_operation() {
        let (_dir, root) = journal();
        let kind = Plain::default();
        let results = detach_set(
            &root,
            &[&kind],
            people(&root),
            Action::EntityDelete,
            web(),
            false,
        )
        .unwrap();
        assert!(
            results
                .iter()
                .all(|result| matches!(result, SetResult::Moved { .. }))
        );
        let found = entries(&root);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|entry| entry.event == Event::Trashed));
        let operation = one_operation(&found);
        for item in list(&root).unwrap() {
            assert_eq!(item.operation.as_deref(), Some(operation.as_str()));
        }
    }

    /// A set asked erased now records its erases under the same operation.
    #[test]
    fn a_set_erased_now_records_its_erases_under_its_operation() {
        let (_dir, root) = journal();
        let kind = Plain::default();
        detach_set(
            &root,
            &[&kind],
            people(&root),
            Action::EntityDelete,
            web(),
            true,
        )
        .unwrap();
        let found = entries(&root);
        assert_eq!(found.len(), 4);
        assert_eq!(
            found
                .iter()
                .filter(|entry| entry.event == Event::Erased)
                .count(),
            2
        );
        one_operation(&found);
    }

    /// A set's item stopped after its record and before it said so: the
    /// sweep finishes it, and it is recorded once, under the set's operation.
    #[test]
    fn a_set_item_resumed_after_a_stop_records_once_under_the_set_operation() {
        let (_dir, root) = journal();
        let kind = Plain::default();
        detach_set(
            &root,
            &[&kind],
            people(&root),
            Action::EntityDelete,
            web(),
            false,
        )
        .unwrap();
        let mut item = list(&root).unwrap().remove(0);
        item.state = State::Detaching;
        publish(&root, &item).unwrap();
        let swept = sweep(&root, &[&kind], Utc::now());
        assert_eq!(swept.resumed, 1);
        let found = entries(&root);
        assert_eq!(found.len(), 2);
        assert_eq!(
            item.operation.as_deref(),
            Some(one_operation(&found).as_str())
        );
    }

    /// A set a stop left before anything moved: the sweep moves each thing
    /// under the set's operation.
    #[test]
    fn a_set_finished_by_a_sweep_keeps_its_operation() {
        let (_dir, root) = journal();
        let kind = Plain::default();
        let set = Set {
            schema: SCHEMA,
            id: item_id(Utc::now()),
            action: Action::EntityDelete,
            surface: web(),
            erase: false,
            entries: people(&root)
                .into_iter()
                .map(|chosen| Entry {
                    chosen,
                    item: None,
                    arrived: false,
                    settled: false,
                })
                .collect(),
            operation: Some("20261111T000000000Z-20261111T000000000Zabcd1234".into()),
        };
        publish_set(&root, &set, true).unwrap();
        sweep(&root, &[&kind], Utc::now());
        let found = entries(&root);
        assert_eq!(found.len(), 2);
        assert_eq!(Some(one_operation(&found)), set.operation);
    }

    /// Several items recovered together, then several erased together: each
    /// act's entries share its one operation.
    #[test]
    fn a_selection_recovered_or_erased_together_records_under_one_operation() {
        let (_dir, root) = journal();
        let kind = Plain::default();
        let trash_people = |root: &Path| -> Vec<String> {
            people(root)
                .into_iter()
                .map(|chosen| {
                    detach(
                        root,
                        &kind,
                        chosen.unit,
                        Action::EntityDelete,
                        web(),
                        chosen.paths,
                    )
                    .unwrap()
                    .id
                })
                .collect()
        };
        let recover = OperationId::fresh(Utc::now());
        for id in trash_people(&root) {
            attach_in(&root, &[&kind], &id, Action::Recover, web(), &recover).unwrap();
        }
        let recovered: Vec<_> = entries(&root)
            .into_iter()
            .filter(|entry| entry.event == Event::Recovered)
            .collect();
        assert_eq!(recovered.len(), 2);
        assert_eq!(one_operation(&recovered), recover.as_str());

        let erase = OperationId::fresh(Utc::now());
        for id in trash_people(&root) {
            erase_in(&root, &id, Action::Erase, web(), &erase).unwrap();
        }
        let erased: Vec<_> = entries(&root)
            .into_iter()
            .filter(|entry| entry.event == Event::Erased)
            .collect();
        assert_eq!(erased.len(), 2);
        assert_eq!(one_operation(&erased), erase.as_str());
    }

    /// An item written before items carried an operation still recovers and
    /// erases, each under its own.
    #[test]
    fn an_item_written_without_an_operation_still_recovers_and_erases() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        let raw = fs::read_to_string(root.join(TRASH_DIR).join(&item.id).join(ITEM_FILE)).unwrap();
        assert!(!raw.contains("operation"));
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        let again = trash(&root, &kind);
        erase(&root, &again.id, Action::Erase, web()).unwrap();
        assert_eq!(
            events(&root),
            vec![
                (Event::Erased, Action::Erase),
                (Event::Recovered, Action::Recover),
                (Event::Trashed, Action::SegmentDelete),
                (Event::Trashed, Action::SegmentDelete),
            ]
        );
    }

    /// The paths the trash could still give back: every item but one being
    /// erased.
    #[test]
    fn held_paths_name_what_could_come_back() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        assert_eq!(held_paths(&root).unwrap(), vec![SEGMENT.to_owned()]);
        erase(&root, &item.id, Action::Erase, web()).unwrap();
        assert!(held_paths(&root).unwrap().is_empty());
    }

    /// A stop after the item was published but before anything moved: the
    /// sweep moves it, as the owner asked.
    #[test]
    fn a_detach_stopped_before_the_move_is_finished_by_a_sweep() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = Item {
            schema: SCHEMA,
            id: item_id(Utc::now()),
            state: State::Detaching,
            unit: unit(),
            action: Action::SegmentDelete,
            surface: web(),
            trashed_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            erase_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            paths: vec![SEGMENT.into()],
            bytes: 8,
            pending: None,
            label: None,
            refs: Vec::new(),
            operation: None,
            linked: false,
        };
        ensure_trash(&root).unwrap();
        fs::create_dir(root.join(TRASH_DIR).join(&item.id)).unwrap();
        publish_new(&root, &item).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert!(!root.join(SEGMENT).join("audio.flac").exists());
        assert_eq!(list(&root).unwrap().len(), 1);
    }

    /// A stop half-way through a recovery that had set aside what was in the
    /// way: the sweep puts the item back and drops what was set aside.
    #[test]
    fn an_attach_stopped_part_way_is_finished_by_a_sweep() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let mut item = trash(&root, &kind);
        fs::create_dir_all(root.join(SEGMENT)).unwrap();
        fs::write(root.join(SEGMENT).join("audio.flac"), b"again").unwrap();
        // Recovery published its intent and set aside what was there, then stopped.
        item.state = State::Attaching;
        item.pending = Some(Pending {
            action: Action::Recover,
            surface: web(),
            at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            operation: None,
        });
        publish(&root, &item).unwrap();
        let displaced = displaced_rel(&item.id, SEGMENT);
        ensure_trash_parent(&root, &displaced).unwrap();
        move_entry(
            &journal_entry(&root, SEGMENT).unwrap(),
            &trash_entry(&root, &displaced).unwrap(),
        )
        .unwrap();
        let swept = sweep(&root, &[&kind], Utc::now());
        assert_eq!(swept.resumed, 1, "{swept:?}");
        assert_eq!(
            fs::read(root.join(SEGMENT).join("audio.flac")).unwrap(),
            b"sound"
        );
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
        assert!(
            events(&root)
                .iter()
                .any(|(event, action)| *event == Event::Recovered && *action == Action::Recover)
        );
    }

    /// A stop part-way through an erase: the sweep finishes it.
    #[test]
    fn an_erase_stopped_part_way_is_finished_by_a_sweep() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let mut item = trash(&root, &kind);
        item.state = State::Erasing;
        item.pending = Some(Pending {
            action: Action::EmptyTrash,
            surface: web(),
            at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            operation: None,
        });
        publish(&root, &item).unwrap();
        fs::remove_file(
            root.join(TRASH_DIR)
                .join(&item.id)
                .join("files")
                .join(SEGMENT)
                .join("audio.flac"),
        )
        .unwrap();
        let swept = sweep(&root, &[&kind], Utc::now());
        assert_eq!(swept.resumed, 1);
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
        assert!(events(&root).contains(&(Event::Erased, Action::EmptyTrash)));
    }

    #[test]
    fn an_in_flight_item_refuses_another_step() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let mut item = trash(&root, &kind);
        item.state = State::Erasing;
        publish(&root, &item).unwrap();
        assert!(matches!(
            attach(&root, &[&kind], &item.id, Action::Recover, web()),
            Err(TrashError::InFlight)
        ));
    }

    #[test]
    fn the_setting_reads_in_range_or_the_default() {
        let (_dir, root) = journal();
        assert_eq!(erase_after_days(&root), DEFAULT_ERASE_AFTER_DAYS);
        fs::create_dir_all(root.join("config")).unwrap();
        for (value, expected) in [("12", 12), ("0", 7), ("31", 7), ("\"x\"", 7), ("30", 30)] {
            fs::write(
                root.join("config/journal.json"),
                format!("{{\"{SETTINGS_SECTION}\":{{\"{ERASE_AFTER_DAYS_KEY}\":{value}}}}}"),
            )
            .unwrap();
            assert_eq!(erase_after_days(&root), expected, "{value}");
        }
    }

    /// A shorter setting never brings an item's erase date forward.
    #[test]
    fn a_new_setting_applies_only_to_what_is_trashed_after_it() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            format!("{{\"{SETTINGS_SECTION}\":{{\"{ERASE_AFTER_DAYS_KEY}\":1}}}}"),
        )
        .unwrap();
        let swept = sweep(
            &root,
            &[&kind],
            item.trashed_at().unwrap() + Duration::days(2),
        );
        assert_eq!((swept.erased, swept.waiting), (0, 1));
    }

    #[test]
    fn an_id_cannot_reach_outside_the_trash() {
        let (_dir, root) = journal();
        for id in ["../config", "", "a/b", ".."] {
            assert!(read_item(&root, id).unwrap().is_none());
        }
    }

    const AUDIO: &str = "chronicle/20261108/phone/070000_300/audio.flac";

    fn link(root: &Path, kind: &Fake, paths: &[&str]) -> Linked {
        detach_linked(
            root,
            kind,
            unit(),
            Action::OriginalsRelease,
            web(),
            paths.iter().map(|path| (*path).to_owned()).collect(),
            &[("class", b"policy_raw_release")],
            None,
        )
        .unwrap()
    }

    /// The mechanism removed the journal's name: the trash keeps the file,
    /// records it trashed, and recovering puts the same bytes back.
    #[test]
    fn a_linked_file_the_release_removed_is_kept_and_comes_back() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO]);
        assert!(
            root.join(AUDIO).is_file(),
            "linking changes nothing in place"
        );
        fs::remove_file(root.join(AUDIO)).unwrap();
        let item = finish_linked(&root, &kind, linked).unwrap().unwrap();
        assert_eq!(item.state, State::Trashed);
        assert_eq!(item.paths, vec![AUDIO.to_owned()]);
        assert_eq!(item.bytes, 5);
        assert_eq!(
            kept(&root, &item.id, "class").unwrap().unwrap(),
            b"policy_raw_release"
        );
        let held = root
            .join(TRASH_DIR)
            .join(&item.id)
            .join(FILES_DIR)
            .join(AUDIO);
        assert_eq!(fs::read(&held).unwrap(), b"sound");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                fs::metadata(&held).unwrap().nlink(),
                1,
                "the trash's name is the last"
            );
        }
        assert_eq!(list(&root).unwrap().len(), 1);
        assert_eq!(
            events(&root),
            vec![(Event::Trashed, Action::OriginalsRelease)]
        );
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        assert_eq!(fs::read(root.join(AUDIO)).unwrap(), b"sound");
        assert!(list(&root).unwrap().is_empty());
        assert!(
            !root
                .join(TRASH_DIR)
                .join(format!("{}.run.lock", item.id))
                .exists()
        );
    }

    /// The mechanism refused: the journal still has the file, so the trash
    /// drops its second name and keeps no item.
    #[test]
    fn a_linked_file_the_release_left_leaves_no_item() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO]);
        assert!(finish_linked(&root, &kind, linked).unwrap().is_none());
        assert_eq!(fs::read(root.join(AUDIO)).unwrap(), b"sound");
        assert!(all_items(&root).unwrap().is_empty());
        assert!(events(&root).is_empty());
        assert!(
            kind.seen.borrow().is_empty(),
            "no references for what stayed"
        );
    }

    /// Of two files, the release removed one: the item keeps only that one.
    #[test]
    fn a_linked_item_keeps_only_what_left() {
        let (_dir, root) = journal();
        let other = "chronicle/20261108/phone/070000_300/screen.webm";
        fs::write(root.join(other), b"pixels!").unwrap();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO, other]);
        fs::remove_file(root.join(other)).unwrap();
        let item = finish_linked(&root, &kind, linked).unwrap().unwrap();
        assert_eq!(item.paths, vec![other.to_owned()]);
        assert_eq!(item.bytes, 7);
        assert_eq!(fs::read(root.join(AUDIO)).unwrap(), b"sound");
        let dropped = root
            .join(TRASH_DIR)
            .join(&item.id)
            .join(FILES_DIR)
            .join(AUDIO);
        assert!(!dropped.exists());
        assert_eq!(
            kind.seen.borrow()[0],
            (Change::Detached, vec![other.to_owned()])
        );
    }

    /// A stop after linking: a live request's item is left alone and isn't
    /// shown as stopped; once its request is gone the sweep settles it by
    /// what left, either way.
    #[test]
    fn a_stopped_linked_item_is_settled_by_the_sweep_only_once_its_request_is_gone() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO]);
        let id = linked.id().to_owned();
        fs::remove_file(root.join(AUDIO)).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert_eq!(item(&root, &id).unwrap().unwrap().state, State::Detaching);
        assert!(stopped(&root).unwrap().is_empty(), "running, not stopped");
        drop(linked);
        assert_eq!(stopped(&root).unwrap().len(), 1);
        sweep(&root, &[&kind], Utc::now());
        assert_eq!(item(&root, &id).unwrap().unwrap().state, State::Trashed);

        let (_dir, root) = journal();
        let linked = link(&root, &kind, &[AUDIO]);
        let id = linked.id().to_owned();
        drop(linked);
        sweep(&root, &[&kind], Utc::now());
        assert!(item(&root, &id).unwrap().is_none());
        assert_eq!(fs::read(root.join(AUDIO)).unwrap(), b"sound");
    }

    /// Settled by what the trash's name is the name of: a different file now
    /// at the journal's name means the original was released, and is kept.
    #[test]
    fn a_replaced_name_still_keeps_the_released_original() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO]);
        fs::remove_file(root.join(AUDIO)).unwrap();
        fs::write(root.join(AUDIO), b"another").unwrap();
        let item = finish_linked(&root, &kind, linked).unwrap().unwrap();
        let held = root
            .join(TRASH_DIR)
            .join(&item.id)
            .join(FILES_DIR)
            .join(AUDIO);
        assert_eq!(fs::read(held).unwrap(), b"sound");
        assert_eq!(fs::read(root.join(AUDIO)).unwrap(), b"another");
    }

    /// Kept whatever a mechanism reports: the journal's name is gone and no
    /// other name holds the file, so the trash's is the last. The date is
    /// provisional until then, never already past.
    #[test]
    fn a_gone_name_is_kept_and_the_date_is_never_past() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO]);
        let before = item(&root, linked.id()).unwrap().unwrap();
        assert!(before.erase_at().unwrap() > Utc::now() + Duration::days(6));
        fs::remove_file(root.join(AUDIO)).unwrap();
        let kept = finish_linked(&root, &kind, linked).unwrap().unwrap();
        assert_eq!(kept.state, State::Trashed);
    }

    /// Where there are no hard links the item holds a copy: dropped when the
    /// journal's name still holds the same bytes, kept when it's gone, and
    /// erased with the item.
    #[test]
    fn a_copy_is_dropped_only_when_the_journal_still_holds_its_bytes() {
        linked::NO_HARD_LINKS.with(|cell| cell.set(true));
        let (_dir, root) = journal();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO]);
        assert_eq!(
            kept(&root, linked.id(), "linked-copies").unwrap().unwrap(),
            AUDIO.as_bytes()
        );
        assert!(finish_linked(&root, &kind, linked).unwrap().is_none());
        assert_eq!(fs::read(root.join(AUDIO)).unwrap(), b"sound");

        let linked = link(&root, &kind, &[AUDIO]);
        fs::remove_file(root.join(AUDIO)).unwrap();
        let item = finish_linked(&root, &kind, linked).unwrap().unwrap();
        let held = root
            .join(TRASH_DIR)
            .join(&item.id)
            .join(FILES_DIR)
            .join(AUDIO);
        assert_eq!(fs::read(&held).unwrap(), b"sound");
        erase(&root, &item.id, Action::Erase, web()).unwrap();
        assert!(!held.exists());
        linked::NO_HARD_LINKS.with(|cell| cell.set(false));
    }

    /// The recording went to the trash, this file in it, before the release:
    /// that item holds the file, so the trash's second name goes. A name
    /// outside the trash that also holds it proves nothing, and ours stays.
    #[test]
    fn a_file_only_another_trash_item_holds_is_left_to_it() {
        let (_dir, root) = journal();
        let kind = Fake {
            homeless: vec![AUDIO.into()],
            ..Fake::default()
        };
        let linked = link(&root, &kind, &[AUDIO]);
        let recording = trash(&root, &kind);
        assert!(
            root.join(TRASH_DIR)
                .join(&recording.id)
                .join(FILES_DIR)
                .join(AUDIO)
                .is_file()
        );
        assert!(finish_linked(&root, &kind, linked).unwrap().is_none());

        let (_dir, root) = journal();
        let linked = link(&root, &kind, &[AUDIO]);
        let outside = root.join("backup-copy.flac");
        fs::hard_link(root.join(AUDIO), &outside).unwrap();
        fs::remove_file(root.join(AUDIO)).unwrap();
        assert!(
            finish_linked(&root, &kind, linked).unwrap().is_some(),
            "an outside name proves nothing"
        );
    }

    /// Two acts can't both keep the same file: the second finds it already
    /// held by a linked item, and has nothing to link.
    #[test]
    fn a_file_another_linked_item_holds_is_not_linked_twice() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let first = link(&root, &kind, &[AUDIO]);
        let second = detach_linked(
            &root,
            &kind,
            unit(),
            Action::OriginalsRelease,
            web(),
            vec![AUDIO.into()],
            &[],
            None,
        );
        assert!(matches!(second, Err(TrashError::Missing(_))));
        drop(first);
    }

    /// The owner's days count from when it settled, not when it was linked.
    #[test]
    fn a_linked_item_counts_its_days_from_when_it_settles() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO]);
        let id = linked.id().to_owned();
        let mut stale = item(&root, &id).unwrap().unwrap();
        stale.trashed_at = "2026-01-01T00:00:00.000Z".into();
        stale.erase_at = "2026-01-02T00:00:00.000Z".into();
        publish(&root, &stale).unwrap();
        fs::remove_file(root.join(AUDIO)).unwrap();
        let item = finish_linked(&root, &kind, linked).unwrap().unwrap();
        assert!(item.erase_at().unwrap() > Utc::now() + Duration::days(6));
        sweep(&root, &[&kind], Utc::now());
        assert_eq!(list(&root).unwrap().len(), 1);
    }

    /// Erasing the recording erases the originals released from it, which
    /// wait in their own item, in the same act.
    #[test]
    fn erasing_a_recording_erases_its_originals_item_too() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO]);
        fs::remove_file(root.join(AUDIO)).unwrap();
        let originals = finish_linked(&root, &kind, linked).unwrap().unwrap();
        // The fake's references leave a tombstone; the recording is still here.
        fs::remove_file(root.join(SEGMENT).join("tombstone.json")).unwrap();
        let recording = trash(&root, &kind);
        erase(&root, &recording.id, Action::Erase, web()).unwrap();
        assert!(item(&root, &originals.id).unwrap().is_none());
        assert!(!root.join(TRASH_DIR).join(&originals.id).exists());
    }

    /// Erase takes the trash's name, the last one, and records it erased.
    #[test]
    fn erasing_a_linked_item_frees_the_file() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let linked = link(&root, &kind, &[AUDIO]);
        fs::remove_file(root.join(AUDIO)).unwrap();
        let item = finish_linked(&root, &kind, linked).unwrap().unwrap();
        erase(&root, &item.id, Action::Erase, web()).unwrap();
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
        assert!(!root.join(AUDIO).exists());
    }

    /// A path that isn't there refuses before anything is published.
    #[test]
    fn linking_a_missing_file_refuses_and_leaves_nothing() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let refused = detach_linked(
            &root,
            &kind,
            unit(),
            Action::OriginalsRelease,
            web(),
            vec!["chronicle/20261108/phone/070000_300/gone.flac".into()],
            &[],
            None,
        );
        assert!(matches!(refused, Err(TrashError::Missing(_))));
        assert!(all_items(&root).unwrap().is_empty());
    }

    /// A temporary journal root, removed when dropped.
    mod tempfile_dir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Self {
                let mut bytes = [0_u8; 8];
                getrandom::fill(&mut bytes).unwrap();
                let name: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                let path = std::env::temp_dir().join(format!("solstone-trash-test-{name}"));
                std::fs::create_dir_all(&path).unwrap();
                Self(path.canonicalize().unwrap())
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
