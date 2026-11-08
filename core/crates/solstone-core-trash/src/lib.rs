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
}

/// One kind of thing the trash can hold: its exclusion and its references.
/// The trash itself does the moving.
pub trait Kind {
    /// Whether this kind holds `unit`.
    fn handles(&self, unit: &Unit) -> bool;

    /// Hold the thing's places in the journal for the length of a move.
    fn hold<'a>(&'a self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'a>, String>;

    /// After the files moved, point the journal's references at what is now
    /// true. `files` names every journal-relative file that moved. Runs again
    /// when a step is resumed, so it must be safe to repeat.
    fn references(&self, journal: &Path, item: &Item, change: Change, files: &[String]);
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
    let now = Utc::now();
    let erase_at = now + Duration::days(i64::from(erase_after_days(journal)));
    let mut item = Item {
        schema: SCHEMA,
        id: item_id(now),
        state: State::Detaching,
        unit,
        action,
        surface,
        trashed_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
        erase_at: erase_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        paths,
        bytes: 0,
        pending: None,
    };
    let hold = kind.hold(journal, &item).map_err(TrashError::Held)?;
    hold.check().map_err(TrashError::Held)?;
    let _lock = lock(journal)?;
    for path in &item.paths {
        if !path_lexists(&journal_entry(journal, path)?)? {
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
    std::fs::create_dir(&dir).map_err(|error| TrashError::Io(error.to_string()))?;
    publish_new(journal, &item)?;
    finish_detach(journal, kind, &mut item, hold.as_ref())?;
    Ok(item)
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
        });
        publish(journal, &item)?;
    }
    finish_attach(journal, kind, &item, hold.as_ref())?;
    Ok(item)
}

/// Erase one item now. Records [`Event::Erased`].
pub fn erase(journal: &Path, id: &str, action: Action, surface: Surface) -> Result<(), TrashError> {
    let _lock = lock(journal)?;
    let mut item = read_item(journal, id)?.ok_or(TrashError::NotInTrash)?;
    match item.state {
        State::Trashed => {
            item.state = State::Erasing;
            item.pending = Some(Pending {
                action,
                surface,
                at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            });
            publish(journal, &item)?;
        }
        State::Erasing => {}
        State::Detaching | State::Attaching => return Err(TrashError::InFlight),
    }
    finish_erase(journal, &item)
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

/// One item by id, whatever its state.
pub fn item(journal: &Path, id: &str) -> Result<Option<Item>, TrashError> {
    read_item(journal, id)
}

/// The days the owner set before the trash erases an item, or the default.
/// An unreadable or out-of-range setting reads as the default.
pub fn erase_after_days(journal: &Path) -> u32 {
    solstone_core_journal_config::read_journal_config(journal)
        .ok()
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
                Some(at) if at <= now => {
                    erase(journal, &item.id, Action::Erase, timer_surface()).map(|()| {
                        swept.erased += 1;
                    })
                }
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
            if !path_lexists(&live)? {
                // In neither place: nothing of it is left to move.
                continue;
            }
            hold.check().map_err(TrashError::Held)?;
            ensure_trash_parent(journal, &trashed)?;
            move_entry(&live, &held)?;
            sync_parent(journal, path);
            sync_trash_parent(journal, &trashed);
        }
        files.extend(tree_files(&held, path));
    }
    kind.references(journal, item, Change::Detached, &files);
    let at = item.trashed_at().unwrap_or_else(Utc::now);
    Removal::with_id(
        journal,
        OperationId::durable(at, &item.id),
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
    let mut files = Vec::new();
    for path in &item.paths {
        let trashed = files_rel(&item.id, path);
        let held = trash_entry(journal, &trashed)?;
        if path_lexists(&held)? {
            hold.check().map_err(TrashError::Held)?;
            ensure_journal_parent(journal, path)?;
            let live = journal_entry(journal, path)?;
            if path_lexists(&live)? {
                // Something is where it was: the tombstone it left, or the
                // same recording arriving again. Recovery overwrites: set it
                // aside inside the item, where the item's removal takes it.
                let displaced = displaced_rel(&item.id, path);
                ensure_trash_parent(journal, &displaced)?;
                let aside = trash_entry(journal, &displaced)?;
                if path_lexists(&aside)? {
                    return Err(TrashError::Io(format!(
                        "item {} already set aside what was at {path}",
                        item.id
                    )));
                }
                move_entry(&live, &aside)?;
            }
            move_entry(&held, &live)?;
            sync_parent(journal, path);
        }
        files.extend(tree_files(&journal_entry(journal, path)?, path));
    }
    kind.references(journal, item, Change::Attached, &files);
    if let Some(pending) = &item.pending {
        let at = DateTime::parse_from_rfc3339(&pending.at)
            .map(|at| at.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        Removal::with_id(
            journal,
            OperationId::durable(at, &item.id),
            pending.action,
            pending.surface,
        )
        .record_event(Event::Recovered, item.unit.clone());
    }
    remove_item_dir(journal, item)
}

/// Unlink the item's files, record, and remove the item. Safe to run again.
fn finish_erase(journal: &Path, item: &Item) -> Result<(), TrashError> {
    let pending = item.pending.clone().unwrap_or(Pending {
        action: Action::Erase,
        surface: timer_surface(),
        at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    });
    let at = DateTime::parse_from_rfc3339(&pending.at)
        .map(|at| at.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let files = trash_root(journal).join(&item.id).join(FILES_DIR);
    Removal::with_id(
        journal,
        OperationId::durable(at, &item.id),
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
            Err(error) => log::warn!("trash: {error}"),
        }
    }
    Ok(items)
}

fn item_file(journal: &Path, item: &Item) -> Result<PathBuf, TrashError> {
    trash_entry(journal, &format!("{}/{ITEM_FILE}", item.id))
}

fn publish_new(journal: &Path, item: &Item) -> Result<(), TrashError> {
    let bytes = serde_json::to_vec_pretty(item).map_err(|error| TrashError::Io(error.to_string()))?;
    write_bytes_exclusive(
        &item_file(journal, item)?,
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| TrashError::Io(error.to_string()))
}

fn publish(journal: &Path, item: &Item) -> Result<(), TrashError> {
    let bytes = serde_json::to_vec_pretty(item).map_err(|error| TrashError::Io(error.to_string()))?;
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

    struct Free;
    impl Hold for Free {
        fn check(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Fake {
        seen: RefCell<Vec<(Change, Vec<String>)>>,
    }

    impl Kind for Fake {
        fn handles(&self, unit: &Unit) -> bool {
            matches!(unit, Unit::Segment { .. })
        }
        fn hold<'a>(&'a self, _: &Path, _: &Item) -> Result<Box<dyn Hold + 'a>, String> {
            Ok(Box::new(Free))
        }
        fn references(&self, _: &Path, _: &Item, change: Change, files: &[String]) {
            self.seen.borrow_mut().push((change, files.to_vec()));
        }
    }

    const SEGMENT: &str = "chronicle/20261108/phone/070000_300";

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
        assert!(!root.join(SEGMENT).exists());
        let held = root.join(TRASH_DIR).join(&item.id).join("files").join(SEGMENT);
        assert!(root.join(TRASH_DIR).join(&item.id).join("item.json").is_file());
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
        assert_eq!(fs::read(root.join(SEGMENT).join("audio.flac")).unwrap(), b"sound");
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
        fs::create_dir_all(root.join(SEGMENT)).unwrap();
        fs::write(root.join(SEGMENT).join("audio.flac"), b"again").unwrap();
        fs::write(root.join(SEGMENT).join("extra.json"), b"{}").unwrap();
        attach(&root, &[&kind], &item.id, Action::Recover, web()).unwrap();
        assert_eq!(fs::read(root.join(SEGMENT).join("audio.flac")).unwrap(), b"sound");
        assert!(!root.join(SEGMENT).join("extra.json").exists());
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
    }

    #[test]
    fn erasing_unlinks_and_records_it() {
        let (_dir, root) = journal();
        let kind = Fake::default();
        let item = trash(&root, &kind);
        erase(&root, &item.id, Action::Erase, web()).unwrap();
        assert!(!root.join(TRASH_DIR).join(&item.id).exists());
        assert!(!root.join(SEGMENT).exists());
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
        assert!(!root.join(SEGMENT).exists());
        // Recorded once, whatever the number of runs.
        assert_eq!(events(&root).len(), 1);
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
        };
        ensure_trash(&root).unwrap();
        fs::create_dir(root.join(TRASH_DIR).join(&item.id)).unwrap();
        publish_new(&root, &item).unwrap();
        sweep(&root, &[&kind], Utc::now());
        assert!(!root.join(SEGMENT).exists());
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
        assert_eq!(fs::read(root.join(SEGMENT).join("audio.flac")).unwrap(), b"sound");
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
        let swept = sweep(&root, &[&kind], item.trashed_at().unwrap() + Duration::days(2));
        assert_eq!((swept.erased, swept.waiting), (0, 1));
    }

    #[test]
    fn an_id_cannot_reach_outside_the_trash() {
        let (_dir, root) = journal();
        for id in ["../config", "", "a/b", ".."] {
            assert!(read_item(&root, id).unwrap().is_none());
        }
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
