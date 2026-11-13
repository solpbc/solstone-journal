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
//! release part way anywhere in the journal), each path is settled by what the
//! trash's name is the name of, never by whether a name is present:
//!
//! - where the caller has the mechanism's own report of what it removed
//!   ([`finish_linked`]), a path it didn't report is dropped: a file replaced
//!   before the mechanism ran was refused by it, and is not this item's;
//! - the journal's name is the same file: the mechanism didn't remove it, so
//!   the trash's name is dropped;
//! - the thing it belongs to has left the journal since (the recording went to
//!   the trash or was removed) and another name still holds the file: that
//!   other name keeps it, so the trash's is dropped;
//! - otherwise the trash's name is now the file's only one, and it is kept.
//!
//! Where the filesystem has no hard links, the item holds a copy instead, and
//! a copy is kept only when the journal's name is gone.
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
use solstone_core_journal_io::entry::{link_count, same_file, sync_dir};
use solstone_core_journal_io::locking::{FileLock, LockOptions, hold_lock};
use solstone_core_journal_io::paths::path_lexists;
use solstone_core_removals::{Action, Event, Surface, Unit};

use super::{
    FILES_DIR, Item, Kind, SCHEMA, State, TrashError, abandon, describe, ensure_trash,
    ensure_trash_parent, erase_after_days, files_rel, item_id, journal_entry, keep, kept, lock,
    publish, publish_new, read_item, removal, trash_entry, trash_root, tree_bytes,
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
/// journal changes. A path that isn't there, or can't be kept durably, refuses
/// the whole item and leaves nothing behind: the caller then removes nothing.
/// A caller that would rather skip a path already gone leaves it out first.
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
/// keep what left the journal, drop what is still there. `released` is the
/// mechanism's own report of the journal-relative paths it removed, where the
/// caller has it; only those are kept. `None` when nothing left, and then no
/// item remains. An error leaves the item for the sweep, as does a hold that
/// refuses because the journal is busy.
pub fn finish_linked(
    journal: &Path,
    kind: &dyn Kind,
    mut linked: Linked,
    released: Option<&[String]>,
) -> Result<Option<Item>, TrashError> {
    let result = settle(journal, kind, &linked.id, released);
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
    let result = settle(journal, kind, id, None).map(|_| ());
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

fn settle(
    journal: &Path,
    kind: &dyn Kind,
    id: &str,
    reported: Option<&[String]>,
) -> Result<Option<Item>, TrashError> {
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
        let released = if reported.is_some_and(|reported| !reported.contains(path)) {
            false
        } else if copies.contains(path) {
            !kind.present(journal, path)
        } else if path_lexists(&live)? && same_file(&live, &held).map_err(io)? {
            false
        } else if !kind.returns(journal, &item, path) {
            // What it belongs to left the journal since: kept only if no other
            // name (that thing's own item) still holds it.
            link_count(&held).map_err(io)? == 1
        } else {
            true
        };
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
    match std::fs::hard_link(from, to) {
        Ok(()) => Ok(Linking::Linked),
        Err(error) if no_hard_links(&error) => {
            let copied = std::fs::copy(from, to).and_then(|_| std::fs::File::open(to)?.sync_all());
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
