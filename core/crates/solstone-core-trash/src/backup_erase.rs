// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What an erase leaves for the backup: the first half of the saved list.
//!
//! When the owner chose to also remove an erased thing from their backups, the
//! trash writes one entry for it here before unlinking anything: its paths in
//! the journal, where it sat in the trash, when its move began and when it
//! settled, and which backup destination was set up. The journal's backup
//! drains these entries when it can (the local erase never waits). The folder
//! is a local obligation, never copied or restored with the journal.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::atomic::atomic_replace;
use solstone_core_journal_io::entry::sync_dir;
use solstone_core_removals::{Surface, Unit};
use solstone_core_time::{Precision, utc_z};

use crate::{Item, TRASH_DIR, TrashError};

/// The folder, relative to the journal.
pub const DIR: &str = "health/backup-erase";

/// The entry format this build writes.
pub const SCHEMA: u32 = 1;

/// The name of the marker a kind's detach leaves where the thing was, and the
/// key in it that names the item.
pub const MARKER_NAME: &str = "tombstone.json";
pub const MARKER_KEY: &str = "cid";

/// One erased thing, still to leave the backups.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Entry {
    pub schema: u32,
    /// The trash item it was.
    pub id: String,
    pub unit: Unit,
    /// Where the owner erased it, for the removal history.
    #[serde(flatten)]
    pub surface: Surface,
    /// Its journal-relative paths where it was.
    pub paths: Vec<String>,
    /// Where it sat in the trash, relative to the journal.
    pub trash_path: String,
    /// When its move to the trash began, and when it settled there, if known.
    pub trashed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
    /// When it was erased here.
    pub erased_at: String,
    /// The backup destination set up when it was erased. An entry drains only
    /// to that one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
}

impl Entry {
    /// The moment a backup holds it only in the trash or by its marker: when it
    /// settled, else when its move began.
    pub fn settled(&self) -> &str {
        self.settled_at.as_deref().unwrap_or(&self.trashed_at)
    }
}

fn dir(journal: &Path) -> PathBuf {
    journal.join(DIR)
}

fn entry_path(journal: &Path, id: &str) -> Result<PathBuf, TrashError> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(TrashError::Io(format!("trash item id {id:?}")));
    }
    Ok(dir(journal).join(format!("{id}.json")))
}

/// Whether backup is set up on this journal: on, paused or unreachable.
pub fn backups_on(journal: &Path) -> bool {
    backups_on_checked(journal).unwrap_or(false)
}

/// [`backups_on`], with a configuration that can't be read as an error: an
/// erase the owner chose to take from the backups never goes ahead on a guess.
pub fn backups_on_checked(journal: &Path) -> Result<bool, TrashError> {
    let read = solstone_core_journal_config::read_journal_config(journal).map_err(|error| {
        TrashError::Io(format!("the journal's settings can't be read: {error}"))
    })?;
    Ok(read
        .config
        .and_then(|config| {
            config
                .get("backup")
                .and_then(|backup| backup.get("enabled"))
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false))
}

/// Write `item`'s entry, once and durably. Safe to repeat: a resumed erase
/// writes the same entry again.
pub(crate) fn write(journal: &Path, item: &Item) -> Result<(), TrashError> {
    let path = entry_path(journal, &item.id)?;
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir(journal)).map_err(|error| TrashError::Io(error.to_string()))?;
    let entry = Entry {
        schema: SCHEMA,
        id: item.id.clone(),
        unit: item.unit.clone(),
        surface: item
            .pending
            .as_ref()
            .map_or(crate::timer_surface(), |pending| pending.surface),
        paths: item.paths.clone(),
        trash_path: format!("{TRASH_DIR}/{}", item.id),
        trashed_at: item.trashed_at.clone(),
        settled_at: item.settled_at.clone(),
        erased_at: utc_z(Timestamp::now(), Precision::Millis),
        // Which backup it drains to. One that can't be read stops the erase,
        // for the sweep to try again, rather than leave an entry bound to none.
        destination: solstone_core_backup::destination_identity(journal)
            .map_err(|error| TrashError::Io(format!("the backup set up can't be read: {error}")))?,
    };
    let bytes =
        serde_json::to_vec_pretty(&entry).map_err(|error| TrashError::Io(error.to_string()))?;
    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(|error| TrashError::Io(error.to_string()))?;
    sync_dir(journal, DIR).map_err(|error| TrashError::Io(error.to_string()))
}

/// Every entry still to drain, oldest first. One that can't be read is named
/// in the second list and left in place.
pub fn pending(journal: &Path) -> (Vec<Entry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut unreadable = Vec::new();
    let listing = match std::fs::read_dir(dir(journal)) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (entries, unreadable);
        }
        // A folder that can't be listed is not an empty one.
        Err(_) => return (entries, vec![DIR.to_owned()]),
    };
    for found in listing {
        let Ok(found) = found else {
            unreadable.push(DIR.to_owned());
            continue;
        };
        let name = found.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        // The runner's own files sit beside the entries.
        if RUNNER_FILES.contains(&name.as_str()) {
            continue;
        }
        match std::fs::read(found.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok())
        {
            Some(entry) if entry.id == id && entry.schema == SCHEMA => entries.push(entry),
            _ => unreadable.push(name),
        }
    }
    entries.sort_by(|a, b| a.erased_at.cmp(&b.erased_at).then(a.id.cmp(&b.id)));
    (entries, unreadable)
}

/// Files the backup-erase runner keeps beside the entries; not entries.
pub const RUNNER_FILES: &[&str] = &["status.json", "pieces.json", "request.json"];

/// Retire an entry once its outcome is recorded.
pub fn retire(journal: &Path, id: &str) -> Result<(), TrashError> {
    match std::fs::remove_file(entry_path(journal, id)?) {
        Ok(()) => sync_dir(journal, DIR).map_err(|error| TrashError::Io(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(TrashError::Io(error.to_string())),
    }
}
