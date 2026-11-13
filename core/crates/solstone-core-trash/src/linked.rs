// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A detach that another guarded removal carries out.
//!
//! Some things leave the journal through a mechanism of their own that already
//! proves what it removes and recovers from a stop part way: retention's door
//! releasing an original recording file from a segment that stays. Rather than
//! move those files itself, the trash keeps a second link to each one first
//! ([`detach_linked`]), the mechanism removes the journal's name as it always
//! has, and [`finish_linked`] keeps what left and drops the rest. Erasing the
//! item unlinks the last name, so the space comes back then.
//!
//! While the request that linked an item is running it holds the item's run
//! lock, and nothing else settles it. A stop leaves the item
//! [`State::Detaching`] with `linked` set; the sweep settles it the same way,
//! under the kind's hold, so a file the mechanism put back is never kept.

use std::path::Path;

use chrono::{Duration, SecondsFormat, Utc};
use solstone_core_journal_io::locking::{FileLock, LockOptions, hold_lock};
use solstone_core_journal_io::paths::path_lexists;
use solstone_core_removals::{Action, Event, Surface, Unit};

use super::{
    Item, Kind, SCHEMA, State, TrashError, abandon, describe, ensure_trash,
    ensure_trash_parent, erase_after_days, files_rel, item_id, journal_entry, keep, lock,
    publish, publish_new, read_item, removal, sync_trash_parent, trash_entry, trash_root,
    tree_bytes,
};

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

/// Keep a second link to each of `paths` in a new item, before a mechanism of
/// the kind's own removes them from the journal. `kept` are bytes the kind
/// keeps with the item ([`keep`]) for its references. A path that can't be
/// linked leaves nothing behind and refuses the whole item: the caller then
/// removes nothing.
pub fn detach_linked(
    journal: &Path,
    kind: &dyn Kind,
    unit: Unit,
    action: Action,
    surface: Surface,
    paths: Vec<String>,
    kept: &[(&str, &[u8])],
) -> Result<Linked, TrashError> {
    if paths.is_empty() {
        return Err(TrashError::Missing(describe(&unit).to_owned()));
    }
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
        refs: Vec::new(),
        label: None,
        operation: None,
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
        for path in &item.paths {
            hold.moving(path).map_err(TrashError::Held)?;
            let trashed = files_rel(&item.id, path);
            ensure_trash_parent(journal, &trashed)?;
            link_entry(
                &journal_entry(journal, path)?,
                &trash_entry(journal, &trashed)?,
            )?;
            sync_trash_parent(journal, &trashed);
        }
        Ok(())
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
/// left, and then no item remains. An error leaves the item for the sweep.
pub fn finish_linked(
    journal: &Path,
    kind: &dyn Kind,
    mut linked: Linked,
) -> Result<Option<Item>, TrashError> {
    let result = settle(journal, kind, &linked.id);
    drop(linked.running.take());
    remove_run_lock(journal, &linked.id);
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
    remove_run_lock(journal, id);
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

fn settle(journal: &Path, kind: &dyn Kind, id: &str) -> Result<Option<Item>, TrashError> {
    let item = read_item(journal, id)?.ok_or(TrashError::NotInTrash)?;
    // The kind's hold first: the door's admission finishes or puts back any
    // release a stop left part way, so what is absent now is gone for good.
    let hold = kind.hold(journal, &item).map_err(TrashError::Held)?;
    hold.check().map_err(TrashError::Held)?;
    let _lock = lock(journal)?;
    let Some(mut item) = read_item(journal, id)? else {
        return Ok(None);
    };
    if item.state != State::Detaching {
        return Ok(Some(item));
    }
    let mut kept = Vec::new();
    for path in &item.paths {
        let held = trash_entry(journal, &files_rel(&item.id, path))?;
        if !path_lexists(&held)? {
            continue;
        }
        if kind.present(journal, path) {
            // Still in the journal: the mechanism didn't remove it.
            hold.moving(path).map_err(TrashError::Held)?;
            drop_link(&held)?;
            continue;
        }
        kept.push(path.clone());
    }
    if kept.is_empty() {
        abandon(journal, &item);
        return Ok(None);
    }
    if kept != item.paths {
        item.unit = kind.narrowed(&item.unit, &kept);
        item.paths = kept;
        item.bytes = item
            .paths
            .iter()
            .filter_map(|path| trash_entry(journal, &files_rel(&item.id, path)).ok())
            .map(|held| tree_bytes(&held))
            .sum();
        publish(journal, &item)?;
    }
    kind.references(journal, &item, super::Change::Detached, &item.paths)
        .map_err(TrashError::Io)?;
    let at = item.trashed_at().unwrap_or_else(Utc::now);
    removal(journal, &item, None, at, item.action, item.surface)
        .record_event(Event::Trashed, item.unit.clone());
    item.state = State::Trashed;
    publish(journal, &item)?;
    Ok(Some(item))
}

/// A second name for `from` at `to`; where the filesystem has no hard links,
/// a durable copy.
fn link_entry(from: &Path, to: &Path) -> Result<(), TrashError> {
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
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(TrashError::Io(format!("{}: {error}", from.display())))
        }
        Err(_) => {
            std::fs::copy(from, to)
                .and_then(|_| std::fs::File::open(to)?.sync_all())
                .map_err(|error| {
                    // Leave no partial copy for a later step to take as kept.
                    let _ = drop_link(to);
                    TrashError::Io(format!("{}: {error}", to.display()))
                })
        }
    }
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

/// The run lock leaves a sidecar file; once its holder lets go, it goes too.
fn remove_run_lock(journal: &Path, id: &str) {
    let _ = std::fs::remove_file(trash_root(journal).join(format!("{id}.run.lock")));
}
