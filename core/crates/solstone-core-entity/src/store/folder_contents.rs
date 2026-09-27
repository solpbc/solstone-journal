// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What an entity folder holds beyond the files this store writes.

use std::path::{Path, PathBuf};

use solstone_core_journal_io::{DirEntryKind, PathError, contained_path, list_dir_entries};

/// How many files under `entities/<entity_dir>/` this store doesn't write:
/// anything but the identity file, history events and lock files. A folder
/// that holds such a file is something other than an entity record alone.
pub fn unrecognized_entity_files(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<usize, PathError> {
    let directory = contained_path(journal_root, &format!("entities/{entity_dir}"))?;
    let mut count = 0;
    for file in descendant_files(&directory)? {
        let allowed = file == directory.join("entity.json")
            || file
                .strip_prefix(directory.join("history/events"))
                .is_ok_and(|relative| relative.extension() == Some("json".as_ref()));
        if !allowed
            && !file
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".lock"))
        {
            count += 1;
        }
    }
    Ok(count)
}

fn descendant_files(directory: &Path) -> Result<Vec<PathBuf>, PathError> {
    let mut files = Vec::new();
    for entry in list_dir_entries(directory)? {
        match entry.kind {
            DirEntryKind::File => files.push(entry.path),
            DirEntryKind::Directory => files.extend(descendant_files(&entry.path)?),
            DirEntryKind::Other => {}
        }
    }
    Ok(files)
}
