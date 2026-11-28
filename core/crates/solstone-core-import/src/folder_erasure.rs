// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Version identities from folder segments the owner put in the trash. A
//! recovery releases only that trash item's identities. Permanent erasure
//! leaves them here without retaining the file, its name or its contents.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use solstone_core_format::folder::{FOLDER_ACTIVITY_FILE, StoredFolderActivity};
use solstone_core_retention::receipt::Target;
use solstone_core_trash::Item;

fn store(journal: &Path) -> PathBuf {
    journal.join("config/folder-erasure/versions.sqlite3")
}

/// Comparison state includes the previous text and belongs in the trash
/// alongside the segment while that version is the current baseline.
pub fn note_relative_path(folder: &str, path: &str) -> String {
    let folder_hash = format!("{:x}", Sha256::digest(folder.as_bytes()));
    let note_hash = format!("{:x}", Sha256::digest(path.as_bytes()));
    format!(
        "config/folders/{}/notes/{}/{note_hash}.json",
        &folder_hash[..32],
        &note_hash[..2]
    )
}

pub(crate) fn note_companions(journal: &Path, paths: &[String]) -> Result<Vec<String>, String> {
    let mut found = BTreeSet::new();
    for rel in paths {
        if solstone_core_retention::layout::parse_segment_rel(rel).is_none() {
            continue;
        }
        if let Some(activity) = read_activity(&journal.join(rel))? {
            found.extend(current_notes(journal, activity)?);
        }
    }
    Ok(found.into_iter().collect())
}

fn read_activity(dir: &Path) -> Result<Option<StoredFolderActivity>, String> {
    match fs::read(dir.join("talents").join(FOLDER_ACTIVITY_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| e.to_string()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn current_notes(journal: &Path, activity: StoredFolderActivity) -> Result<Vec<String>, String> {
    let mut found = BTreeSet::new();
    for event in activity.events {
        if event.kind.as_deref() != Some("notes") {
            continue;
        }
        let note = note_relative_path(&activity.folder_id, &event.path);
        let bytes = match fs::read(journal.join(&note)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        #[derive(serde::Deserialize)]
        struct Baseline {
            content_hash: String,
        }
        let held: Baseline = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if Some(held.content_hash.as_str()) == event.content_hash.as_deref() {
            found.insert(note);
        }
    }
    Ok(found.into_iter().collect())
}

pub struct ErasedVersions(Option<Connection>);

impl ErasedVersions {
    pub fn open(journal: &Path) -> Result<Self, String> {
        let path = store(journal);
        if !path.try_exists().map_err(|e| e.to_string())? {
            return Ok(Self(None));
        }
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map(|db| Self(Some(db)))
            .map_err(|e| e.to_string())
    }

    pub fn contains(&self, key: &str) -> Result<bool, String> {
        let Some(db) = &self.0 else { return Ok(false) };
        db.prepare_cached("SELECT EXISTS(SELECT 1 FROM erased WHERE version = ?1)")
            .and_then(|mut q| q.query_row([key], |row| row.get(0)))
            .map_err(|e| e.to_string())
    }
}

/// A metadata-only file has no byte hash. Match its relative path, byte
/// length if known and modification time (event time on older clients).
pub fn details_key(path: &str, size: Option<u64>, modified_ms: i64) -> String {
    let mut digest = Sha256::new();
    digest.update(path.as_bytes());
    digest.update([0]);
    if let Some(n) = size {
        digest.update(n.to_be_bytes());
    }
    digest.update(modified_ms.to_be_bytes());
    format!("details:{:x}", digest.finalize())
}

pub(crate) fn remember(journal: &Path, item: &Item, segments: &[Target]) -> Result<(), String> {
    let mut held = Vec::new();
    for segment in segments {
        let rel = solstone_core_retention::layout::segment_rel(
            &segment.day,
            &segment.stream,
            &segment.dir,
        );
        let path =
            solstone_core_trash::held_file(journal, &item.id, &rel).map_err(|e| e.to_string())?;
        held.push(path);
    }
    remember_directories(journal, &item.id, &held)
}

pub(crate) fn remember_directories(
    journal: &Path,
    identity: &str,
    dirs: &[PathBuf],
) -> Result<(), String> {
    let mut keys = Vec::new();
    for dir in dirs {
        let Some(activity) = read_activity(dir)? else {
            continue;
        };
        for event in activity.events {
            let details = details_key(
                &event.path,
                event.size,
                event.modified_at_ms.unwrap_or(event.at_ms),
            );
            // Older clients could attach an unverified hash to a details-only event.
            if let Some(hash) = event.kind.and(event.content_hash) {
                keys.push(hash);
                keys.push(format!("without-bytes:{details}"));
            } else {
                keys.push(details);
            }
        }
    }
    if keys.is_empty() {
        return Ok(());
    }
    let path = store(journal);
    solstone_core_journal_io::paths::create_directory_with_mode(
        &journal.join("config/folder-erasure"),
        0o700,
    )
    .map_err(|e| e.to_string())?;
    let mut db = Connection::open(path).map_err(|e| e.to_string())?;
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS erased (version TEXT NOT NULL, item TEXT NOT NULL, PRIMARY KEY(version, item)) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS erased_by_item ON erased(item);").map_err(|e| e.to_string())?;
    {
        let mut insert = tx
            .prepare("INSERT OR IGNORE INTO erased VALUES (?1, ?2)")
            .map_err(|e| e.to_string())?;
        for key in keys {
            insert
                .execute(params![key, identity])
                .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn prepare_permanent(journal: &Path, item: &Item, held: &Path) -> Result<(), String> {
    if !held
        .join("talents")
        .join(FOLDER_ACTIVITY_FILE)
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        return Ok(());
    }
    // Permanent erase runs outside the trash lock. Serialize with folder
    // arrivals while clearing comparison text left by older trash items.
    let _hold = solstone_core_segment::hold_source_mutation(journal, "folder")
        .map_err(|e| e.to_string())?;
    remember_directories(journal, &item.id, &[held.to_owned()])?;
    if let Some(activity) = read_activity(held)? {
        for note in current_notes(journal, activity)? {
            solstone_core_trash::set_aside(journal, item, &note).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub(crate) fn recover(journal: &Path, item: &Item) -> Result<(), String> {
    let path = store(journal);
    if path.try_exists().map_err(|e| e.to_string())? {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .and_then(|db| db.execute("DELETE FROM erased WHERE item = ?1", [&item.id]))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
