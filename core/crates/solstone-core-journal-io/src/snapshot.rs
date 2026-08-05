// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-contained recursive snapshots for rollback operations.

use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::atomic::{AtomicWriteOptions, atomic_replace};
use crate::errors::SnapshotError;
use crate::paths::{contained_path, resolve_journal_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDirectory {
    pub path: String,
    pub entries: Vec<JournalSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalSnapshot {
    Missing { path: String },
    File(SnapshotFile),
    Directory(SnapshotDirectory),
}

/// Capture the current missing, regular-file, or directory-tree state at `rel`.
pub fn capture_snapshot(journal: &Path, rel: &str) -> Result<JournalSnapshot, SnapshotError> {
    let path = journal_path(journal, rel)?;
    capture_path(journal, rel, &path)
}

/// Restore a previously captured snapshot below `journal`.
pub fn restore_snapshot(journal: &Path, snapshot: &JournalSnapshot) -> Result<(), SnapshotError> {
    validate_snapshot(journal, snapshot, None)?;
    let root_rel = snapshot_path(snapshot);
    let root = journal_path(journal, root_rel)?;
    ensure_existing_tree_supported(journal, root_rel, &root)?;
    remove_existing(&root)?;
    restore_path(journal, snapshot)
}

fn capture_path(journal: &Path, rel: &str, path: &Path) -> Result<JournalSnapshot, SnapshotError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(JournalSnapshot::Missing {
                path: rel.to_owned(),
            });
        }
        Err(source) => return Err(io_error(path, source)),
    };
    let file_type = metadata.file_type();
    if file_type.is_file() {
        return Ok(JournalSnapshot::File(SnapshotFile {
            path: rel.to_owned(),
            bytes: fs::read(path).map_err(|source| io_error(path, source))?,
            mode: file_mode(&metadata),
        }));
    }
    if !file_type.is_dir() {
        return Err(SnapshotError::UnsupportedFileType {
            path: path.to_path_buf(),
        });
    }

    let mut entries = fs::read_dir(path)
        .map_err(|source| io_error(path, source))?
        .map(|entry| entry.map_err(|source| io_error(path, source)))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut snapshots = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| SnapshotError::InvalidSnapshot {
                path: rel.to_owned(),
                message: "snapshot path component must be valid UTF-8",
            })?;
        let child_rel = format!("{rel}/{name}");
        let child = journal_path(journal, &child_rel)?;
        snapshots.push(capture_path(journal, &child_rel, &child)?);
    }
    Ok(JournalSnapshot::Directory(SnapshotDirectory {
        path: rel.to_owned(),
        entries: snapshots,
    }))
}

fn restore_path(journal: &Path, snapshot: &JournalSnapshot) -> Result<(), SnapshotError> {
    match snapshot {
        JournalSnapshot::Missing { .. } => Ok(()),
        JournalSnapshot::File(file) => {
            let path = journal_path(journal, &file.path)?;
            atomic_replace(
                &path,
                &file.bytes,
                AtomicWriteOptions {
                    mode: Some(file.mode),
                },
            )
            .map_err(SnapshotError::Atomic)
        }
        JournalSnapshot::Directory(directory) => {
            let path = journal_path(journal, &directory.path)?;
            fs::create_dir_all(&path).map_err(|source| io_error(&path, source))?;
            for entry in &directory.entries {
                restore_path(journal, entry)?;
            }
            Ok(())
        }
    }
}

fn validate_snapshot(
    journal: &Path,
    snapshot: &JournalSnapshot,
    parent: Option<&str>,
) -> Result<(), SnapshotError> {
    let path = snapshot_path(snapshot);
    contained_path(journal, path).map_err(SnapshotError::Path)?;
    if let Some(parent) = parent
        && !is_child_path(parent, path)
    {
        return Err(SnapshotError::InvalidSnapshot {
            path: path.to_owned(),
            message: "snapshot entry is not contained by its parent",
        });
    }
    if let JournalSnapshot::Directory(directory) = snapshot {
        for entry in &directory.entries {
            validate_snapshot(journal, entry, Some(&directory.path))?;
        }
    }
    Ok(())
}

fn snapshot_path(snapshot: &JournalSnapshot) -> &str {
    match snapshot {
        JournalSnapshot::Missing { path } => path,
        JournalSnapshot::File(file) => &file.path,
        JournalSnapshot::Directory(directory) => &directory.path,
    }
}

fn is_child_path(parent: &str, child: &str) -> bool {
    child
        .strip_prefix(parent)
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn journal_path(journal: &Path, rel: &str) -> Result<PathBuf, SnapshotError> {
    contained_path(journal, rel).map_err(SnapshotError::Path)?;
    resolve_journal_path(journal, rel).map_err(SnapshotError::Path)
}

fn remove_existing(path: &Path) -> Result<(), SnapshotError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error(path, source)),
    };
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        return fs::remove_dir_all(path).map_err(|source| io_error(path, source));
    }
    if file_type.is_file() {
        return fs::remove_file(path).map_err(|source| io_error(path, source));
    }
    Err(SnapshotError::UnsupportedFileType {
        path: path.to_path_buf(),
    })
}

fn ensure_existing_tree_supported(
    journal: &Path,
    rel: &str,
    path: &Path,
) -> Result<(), SnapshotError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error(path, source)),
    };
    let file_type = metadata.file_type();
    if file_type.is_file() {
        return Ok(());
    }
    if !file_type.is_dir() {
        return Err(SnapshotError::UnsupportedFileType {
            path: path.to_path_buf(),
        });
    }
    for entry in fs::read_dir(path).map_err(|source| io_error(path, source))? {
        let entry = entry.map_err(|source| io_error(path, source))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| SnapshotError::InvalidSnapshot {
                path: rel.to_owned(),
                message: "snapshot path component must be valid UTF-8",
            })?;
        let child_rel = format!("{rel}/{name}");
        let child = journal_path(journal, &child_rel)?;
        ensure_existing_tree_supported(journal, &child_rel, &child)?;
    }
    Ok(())
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    metadata.mode() & 0o7777
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0
}

fn io_error(path: &Path, source: io::Error) -> SnapshotError {
    SnapshotError::Io {
        path: PathBuf::from(path),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn captures_and_restores_file_tree_bytes_and_modes() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let directory = journal.join("entities/alice/history");
        fs::create_dir_all(&directory).unwrap();
        let entity = journal.join("entities/alice/entity.json");
        let event = directory.join("event.json");
        fs::write(&entity, b"before").unwrap();
        fs::write(&event, b"event").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&entity, fs::Permissions::from_mode(0o640)).unwrap();

        let snapshot = capture_snapshot(&journal, "entities/alice").unwrap();
        fs::remove_dir_all(journal.join("entities/alice")).unwrap();
        fs::create_dir_all(journal.join("entities/alice")).unwrap();
        fs::write(journal.join("entities/alice/entity.json"), b"after").unwrap();

        restore_snapshot(&journal, &snapshot).unwrap();

        assert_eq!(fs::read(&entity).unwrap(), b"before");
        assert_eq!(fs::read(&event).unwrap(), b"event");
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&entity).unwrap().permissions().mode() & 0o7777,
            0o640
        );
    }

    #[test]
    fn missing_snapshot_removes_existing_path() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        fs::create_dir_all(&journal).unwrap();
        let snapshot = capture_snapshot(&journal, "entities/alice").unwrap();
        fs::create_dir_all(journal.join("entities/alice")).unwrap();
        fs::write(journal.join("entities/alice/entity.json"), b"present").unwrap();

        restore_snapshot(&journal, &snapshot).unwrap();

        assert!(!journal.join("entities/alice").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_during_capture_and_restore() {
        use std::os::unix::fs::symlink;

        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        fs::create_dir_all(&journal).unwrap();
        let safe = journal.join("safe");
        fs::create_dir(&safe).unwrap();
        symlink(&safe, journal.join("entities")).unwrap();

        assert!(matches!(
            capture_snapshot(&journal, "entities"),
            Err(SnapshotError::UnsupportedFileType { .. })
        ));
        let snapshot = JournalSnapshot::Missing {
            path: "entities".to_owned(),
        };
        assert!(matches!(
            restore_snapshot(&journal, &snapshot),
            Err(SnapshotError::UnsupportedFileType { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn restore_rejects_a_special_descendant_before_removal() {
        use std::os::unix::fs::symlink;

        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let safe = journal.join("safe");
        fs::create_dir_all(&safe).unwrap();
        fs::create_dir_all(journal.join("entities")).unwrap();
        symlink(&safe, journal.join("entities/alice")).unwrap();
        let snapshot = JournalSnapshot::Missing {
            path: "entities".to_owned(),
        };

        assert!(matches!(
            restore_snapshot(&journal, &snapshot),
            Err(SnapshotError::UnsupportedFileType { .. })
        ));
        assert!(journal.join("entities/alice").symlink_metadata().is_ok());
    }
}
