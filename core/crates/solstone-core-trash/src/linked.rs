// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A detach that another guarded removal carries out.
//!
//! Some things leave the journal through a mechanism of their own that already
//! proves what it removes and recovers from a stop part way: retention's door
//! releasing an original recording file from a segment that stays. Rather than
//! move those files itself, the trash keeps a second name for each one first
//! ([`detach_linked`]), the mechanism removes the journal's name as it always
//! has, and [`finish_linked`] keeps what left and drops the rest. Erasing the
//! item unlinks the last name, so the space comes back then.
//!
//! # Settling
//!
//! Under the kind's hold, and only once its check passes (for the door: no
//! release part way anywhere in the journal), each path is settled. ⛔ The
//! trash's name is dropped only on positive evidence that the file's bytes
//! are still held elsewhere, because dropping the last name loses the owner's
//! original for good, while keeping an extra one costs space until it is
//! erased:
//!
//! - the journal's name is the same file: the mechanism didn't remove it;
//! - another item in the trash holds the same file (the recording, trashed
//!   since with this file in it): that item keeps it;
//! - for a copy, made where the filesystem has no hard links: the journal's
//!   name holds the same bytes.
//!
//! Otherwise it is kept, whatever the mechanism reported: a mechanism can
//! remove a file and report that it didn't (a flush that failed after the
//! unlink).
//!
//! # Locks
//!
//! While the request that linked an item is running it holds the item's run
//! lock, and nothing else settles it; a stop leaves the item
//! [`State::Detaching`] with `linked` set, for the sweep. The order is hold,
//! run lock, trash lock in [`detach_linked`], and run lock, hold, trash lock in
//! settling. The door's admission is a zero-timeout try, the sweep's run lock
//! is a try, and the run lock [`detach_linked`] takes is on an item nothing
//! else knows yet; the segment and trash locks are bounded waits (ten seconds
//! by default). So no cycle can wait forever, but a detach can wait that long
//! on the trash's lock while it holds the journal's admission. ⛔ Never make
//! any of these an unbounded wait.

use std::path::{Path, PathBuf};

use chrono::{Duration, SecondsFormat, Utc};
use solstone_core_journal_io::entry::{same_file, sync_dir};
use solstone_core_journal_io::locking::{FileLock, LockOptions, hold_lock};
use solstone_core_journal_io::paths::path_lexists;
use solstone_core_removals::{Action, Event, Surface, Unit};

use super::{
    FILES_DIR, Item, Kind, SCHEMA, State, TrashError, abandon, describe, ensure_trash,
    ensure_trash_parent, erase_after_days, files_rel, holders, item_id, journal_entry, keep, kept,
    lock, publish, publish_new, read_item, removal, trash_entry, trash_root, tree_bytes,
};

/// What the item keeps: the paths it holds as copies, one per line.
const COPIES: &str = "linked-copies";

/// An item [`detach_linked`] published, held until [`finish_linked`].
#[must_use = "an item left unfinished waits for the sweep"]
pub struct Linked {
    id: String,
    running: Option<FileLock>,
}

impl Linked {
    /// The item's id.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Keep a second name for each of `paths` in a new item, before a mechanism of
/// the kind's own removes them from the journal. `kept` are bytes the kind
/// keeps with the item ([`keep`]) for its references; `operation` is the
/// owner act's, when it trashes several things together. Nothing in the
/// journal changes. Which paths are there is decided under the kind's hold,
/// after whatever it puts right first (the door's recovery puts back an
/// original a stop left part way), so a path the mechanism could still remove
/// is never left out: only one that isn't there is left out. None left is
/// [`TrashError::Missing`]. A path
/// that can't be kept durably refuses the whole item and leaves nothing
/// behind: the caller then removes nothing.
#[expect(
    clippy::too_many_arguments,
    reason = "the detach's own arguments, and the kind's kept bytes and the act's operation"
)]
pub fn detach_linked(
    journal: &Path,
    kind: &dyn Kind,
    unit: Unit,
    action: Action,
    surface: Surface,
    paths: Vec<String>,
    kept: &[(&str, &[u8])],
    operation: Option<String>,
) -> Result<Linked, TrashError> {
    if paths.is_empty() {
        return Err(TrashError::Missing(describe(&unit).to_owned()));
    }
    let now = Utc::now();
    let mut item = Item {
        schema: SCHEMA,
        id: item_id(now),
        state: State::Detaching,
        unit,
        action,
        surface,
        trashed_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
        // Provisional, so a reader before it settles never sees an erase that
        // has already come; settling counts the days again from then.
        erase_at: (now + Duration::days(i64::from(erase_after_days(journal))))
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        paths,
        bytes: 0,
        pending: None,
        refs: Vec::new(),
        label: None,
        operation,
        linked: true,
    };
    let hold = kind.hold(journal, &item).map_err(TrashError::Held)?;
    hold.check().map_err(TrashError::Held)?;
    ensure_trash(journal)?;
    let running = run_lock(journal, &item.id, LockOptions::default())?;
    let _lock = lock(journal)?;
    // Another linked item holding the same file is no reason to leave one
    // out: the door is handed it either way, and two names to one file both
    // keep it, so nothing is lost.
    item.paths.retain(|path| kind.present(journal, path));
    if item.paths.is_empty() {
        return Err(TrashError::Missing(describe(&item.unit).to_owned()));
    }
    item.unit = kind.narrowed(&item.unit, &item.paths);
    item.bytes = item
        .paths
        .iter()
        .map(|path| tree_bytes(&journal.join(path)))
        .sum();
    std::fs::create_dir(trash_root(journal).join(&item.id))
        .map_err(|error| TrashError::Io(error.to_string()))?;
    let linked = publish_new(journal, &item).and_then(|()| {
        for (name, bytes) in kept {
            keep(journal, &item.id, name, bytes)?;
        }
        let mut copies = Vec::new();
        for path in &item.paths {
            hold.moving(path).map_err(TrashError::Held)?;
            let trashed = files_rel(&item.id, path);
            ensure_trash_parent(journal, &trashed)?;
            let held = trash_entry(journal, &trashed)?;
            if link_entry(&journal_entry(journal, path)?, &held)? == Linking::Copied {
                copies.push(path.clone());
            }
            flush_chain(journal, &item.id, path)?;
        }
        if !copies.is_empty() {
            keep(journal, &item.id, COPIES, copies.join("\n").as_bytes())?;
        }
        flush_trash_root(journal)
    });
    if let Err(error) = linked {
        // Nothing in the journal changed: a second name, or a copy, is all
        // the item ever held.
        abandon(journal, &item);
        drop(running);
        remove_run_lock(journal, &item.id);
        return Err(error);
    }
    Ok(Linked {
        id: item.id,
        running: Some(running),
    })
}

/// Settle an item [`detach_linked`] published, once the mechanism has run:
/// keep what left the journal, drop what is still there. `None` when nothing
/// left, and then no item remains. An error leaves the item for the sweep,
/// as does a hold that refuses because the journal is busy.
pub fn finish_linked(
    journal: &Path,
    kind: &dyn Kind,
    mut linked: Linked,
) -> Result<Option<Item>, TrashError> {
    let result = settle(journal, kind, &linked.id);
    drop(linked.running.take());
    if result.is_ok() {
        remove_run_lock(journal, &linked.id);
    }
    result
}

/// The sweep's step for a linked item a stop left, unless a live request
/// still holds it.
pub(crate) fn resume(journal: &Path, kind: &dyn Kind, id: &str) -> Result<(), TrashError> {
    let Ok(running) = run_lock(
        journal,
        id,
        LockOptions {
            timeout: std::time::Duration::ZERO,
            ..LockOptions::default()
        },
    ) else {
        return Ok(());
    };
    let result = settle(journal, kind, id).map(|_| ());
    drop(running);
    if result.is_ok() {
        remove_run_lock(journal, id);
    }
    result
}

/// Whether a live request holds a linked item now.
pub(crate) fn running(journal: &Path, id: &str) -> bool {
    run_lock(
        journal,
        id,
        LockOptions {
            timeout: std::time::Duration::ZERO,
            ..LockOptions::default()
        },
    )
    .is_err()
}

/// Run-lock sidecars whose item is settled or gone: a probe of a busy item
/// can leave one behind.
pub(crate) fn tidy_run_locks(journal: &Path) {
    let Ok(listing) = std::fs::read_dir(trash_root(journal)) else {
        return;
    };
    for entry in listing.flatten() {
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_suffix(".run.lock"))
            .map(str::to_owned)
        else {
            continue;
        };
        // An item that can't be read may still be waiting: keep its lock.
        let waiting = read_item(journal, &id).map_or(true, |item| {
            item.is_some_and(|item| item.state == State::Detaching)
        });
        if !waiting && !running(journal, &id) {
            remove_run_lock(journal, &id);
        }
    }
}

fn settle(journal: &Path, kind: &dyn Kind, id: &str) -> Result<Option<Item>, TrashError> {
    let item = read_item(journal, id)?.ok_or(TrashError::NotInTrash)?;
    // The kind's hold, and its check passing: for the door, nothing is part
    // way through a release anywhere in the journal, so no stage still holds
    // a file this item names.
    let hold = kind.hold(journal, &item).map_err(TrashError::Held)?;
    hold.check().map_err(TrashError::Held)?;
    let _lock = lock(journal)?;
    let Some(mut item) = read_item(journal, id)? else {
        return Ok(None);
    };
    if item.state != State::Detaching {
        return Ok(Some(item));
    }
    let copies: Vec<String> = kept(journal, &item.id, COPIES)?
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .lines()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let mut keeping = Vec::new();
    for path in &item.paths {
        let held = trash_entry(journal, &files_rel(&item.id, path))?;
        if !path_lexists(&held)? {
            continue;
        }
        let live = journal_entry(journal, path)?;
        let held_elsewhere = if copies.contains(path) {
            kind.present(journal, path) && same_bytes(&live, &held).map_err(io)?
        } else if path_lexists(&live)? && same_file(&live, &held).map_err(io)? {
            true
        } else {
            held_by_another(journal, &item, path, &held)?
        };
        let released = !held_elsewhere;
        if released {
            keeping.push(path.clone());
        } else {
            hold.moving(path).map_err(TrashError::Held)?;
            drop_link(&held)?;
        }
    }
    if keeping.is_empty() {
        abandon(journal, &item);
        return Ok(None);
    }
    if keeping != item.paths {
        item.unit = kind.narrowed(&item.unit, &keeping);
        item.paths = keeping;
    }
    item.bytes = item
        .paths
        .iter()
        .filter_map(|path| trash_entry(journal, &files_rel(&item.id, path)).ok())
        .map(|held| tree_bytes(&held))
        .sum();
    // It is in the trash from now: the owner's days count from here.
    let now = Utc::now();
    item.trashed_at = now.to_rfc3339_opts(SecondsFormat::Millis, true);
    item.erase_at = (now + Duration::days(i64::from(erase_after_days(journal))))
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    publish(journal, &item)?;
    kind.references(journal, &item, super::Change::Detached, &item.paths)
        .map_err(TrashError::Io)?;
    removal(
        journal,
        &item,
        item.operation.as_deref(),
        now,
        item.action,
        item.surface,
    )
    .record_event(Event::Trashed, item.unit.clone());
    item.state = State::Trashed;
    publish(journal, &item)?;
    Ok(Some(item))
}

/// Whether another item in the trash holds the same file at `path`: the
/// recording, trashed since with this file in it. Positive evidence, never a
/// link count, which names outside the trash also raise.
fn held_by_another(
    journal: &Path,
    item: &Item,
    path: &str,
    held: &Path,
) -> Result<bool, TrashError> {
    for id in holders(journal, path, &item.id)? {
        let Some(other) = read_item(journal, &id)? else {
            continue;
        };
        if other.linked {
            continue;
        }
        let theirs = trash_entry(journal, &files_rel(&other.id, path))?;
        if path_lexists(&theirs)? && same_file(&theirs, held).map_err(io)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether two files hold the same bytes, for a copy.
fn same_bytes(left: &Path, right: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    if std::fs::symlink_metadata(left)?.len() != std::fs::symlink_metadata(right)?.len() {
        return Ok(false);
    }
    let (mut left, mut right) = (std::fs::File::open(left)?, std::fs::File::open(right)?);
    let (mut a, mut b) = (vec![0_u8; 64 * 1024], vec![0_u8; 64 * 1024]);
    loop {
        let read = left.read(&mut a)?;
        if read == 0 {
            return Ok(true);
        }
        let slice = a.get(..read).unwrap_or_default();
        right.read_exact(b.get_mut(..read).unwrap_or_default())?;
        if slice != b.get(..read).unwrap_or_default() {
            return Ok(false);
        }
    }
}

fn io(error: std::io::Error) -> TrashError {
    TrashError::Io(error.to_string())
}

#[derive(Debug, Eq, PartialEq)]
enum Linking {
    Linked,
    Copied,
}

/// A second name for `from` at `to`; where the filesystem has no hard links,
/// a durable copy. Any other failure refuses.
fn link_entry(from: &Path, to: &Path) -> Result<Linking, TrashError> {
    if path_lexists(to)? {
        return Err(TrashError::Io(format!("{} is already taken", to.display())));
    }
    if !std::fs::symlink_metadata(from)
        .map_err(|error| TrashError::Io(format!("{}: {error}", from.display())))?
        .file_type()
        .is_file()
    {
        return Err(TrashError::Io(format!("{} isn't a file", from.display())));
    }
    match hard_link(from, to) {
        Ok(()) => Ok(Linking::Linked),
        Err(error) if no_hard_links(&error) => {
            let copied = std::fs::copy(from, to).and_then(|_| {
                // Windows flushes a file only through a handle that can write.
                std::fs::OpenOptions::new().write(true).open(to)?.sync_all()
            });
            match copied {
                Ok(()) => Ok(Linking::Copied),
                Err(error) => {
                    // Leave no partial copy for a later step to take as kept.
                    let _ = drop_link(to);
                    Err(TrashError::Io(format!("{}: {error}", to.display())))
                }
            }
        }
        Err(error) => Err(TrashError::Io(format!("{}: {error}", from.display()))),
    }
}

#[cfg(test)]
thread_local! {
    /// Tests stand in for a filesystem with no hard links.
    pub(crate) static NO_HARD_LINKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A hard link, or what a filesystem without them answers.
fn hard_link(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if NO_HARD_LINKS.with(std::cell::Cell::get) {
        return Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
    }
    std::fs::hard_link(from, to)
}

/// Whether `error` says the filesystem has no hard links (FAT, exFAT).
fn no_hard_links(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::Unsupported {
        return true;
    }
    // EPERM (Linux exFAT and FAT), ENOTSUP (macOS), EOPNOTSUPP (Linux, macOS);
    // Windows: ERROR_INVALID_FUNCTION and ERROR_NOT_SUPPORTED.
    let codes: &[i32] = if cfg!(windows) {
        &[1, 50]
    } else {
        &[1, 45, 95, 102]
    };
    error
        .raw_os_error()
        .is_some_and(|code| codes.contains(&code))
}

/// Flush every directory a link for `path` created or changed, from its own
/// up to the trash folder, so the trash's name is durable before the
/// mechanism durably removes the journal's. A failure refuses.
fn flush_chain(journal: &Path, id: &str, path: &str) -> Result<(), TrashError> {
    let root = trash_root(journal);
    let mut rel: PathBuf = PathBuf::from(format!("{id}/{FILES_DIR}/{path}"));
    while rel.pop() && !rel.as_os_str().is_empty() {
        let dir = rel.to_string_lossy().replace('\\', "/");
        sync_dir(&root, &dir).map_err(|error| TrashError::Io(error.to_string()))?;
    }
    flush_trash_root(journal)
}

/// The trash folder's own entries. Windows has no directory flush for it.
fn flush_trash_root(journal: &Path) -> Result<(), TrashError> {
    #[cfg(not(windows))]
    std::fs::File::open(trash_root(journal))
        .and_then(|dir| dir.sync_all())
        .map_err(io)?;
    #[cfg(windows)]
    let _ = journal;
    Ok(())
}

/// Remove the trash's own second name for a file the journal still holds.
fn drop_link(held: &Path) -> Result<(), TrashError> {
    match std::fs::remove_file(held) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(TrashError::Io(format!("{}: {error}", held.display()))),
    }
}

fn run_lock(journal: &Path, id: &str, options: LockOptions) -> Result<FileLock, TrashError> {
    hold_lock(trash_root(journal).join(format!("{id}.run")), options)
        .map_err(|_| TrashError::Held("that is being moved now".to_owned()))
}

/// The run lock leaves a sidecar file; once its item is settled it goes too.
fn remove_run_lock(journal: &Path, id: &str) {
    let _ = std::fs::remove_file(trash_root(journal).join(format!("{id}.run.lock")));
}
