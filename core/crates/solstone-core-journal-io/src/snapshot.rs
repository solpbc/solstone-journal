// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-contained recursive snapshots for rollback operations.

use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

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
    ensure_not_reparse_point(path, &metadata)?;
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
    // Validate the supplied path lexically here. Symlink-aware inspection of
    // the observed tree happens before removal below, but resolving every
    // desired descendant against the observed tree would reject a valid
    // directory snapshot whenever the directory is currently a regular file.
    resolve_journal_path(journal, path).map_err(SnapshotError::Path)?;
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
    ensure_not_reparse_point(path, &metadata)?;
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
    ensure_not_reparse_point(path, &metadata)?;
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

#[cfg(windows)]
fn ensure_not_reparse_point(path: &Path, metadata: &fs::Metadata) -> Result<(), SnapshotError> {
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(SnapshotError::UnsupportedFileType {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(not(windows))]
fn ensure_not_reparse_point(_path: &Path, _metadata: &fs::Metadata) -> Result<(), SnapshotError> {
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
    use std::collections::BTreeMap;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;
    use crate::test_support::TempDir;

    #[derive(Debug, PartialEq, Eq)]
    enum TreeNode {
        File(Vec<u8>),
        Directory,
        Other,
    }

    fn snapshot_paths(snapshot: &JournalSnapshot) -> Vec<String> {
        let mut paths = Vec::new();
        push_snapshot_paths(snapshot, &mut paths);
        paths
    }

    fn push_snapshot_paths(snapshot: &JournalSnapshot, paths: &mut Vec<String>) {
        paths.push(snapshot_path(snapshot).to_owned());
        if let JournalSnapshot::Directory(directory) = snapshot {
            for entry in &directory.entries {
                push_snapshot_paths(entry, paths);
            }
        }
    }

    fn capture_tree(root: &Path) -> BTreeMap<String, TreeNode> {
        let mut tree = BTreeMap::new();
        capture_tree_into(root, "", &mut tree);
        tree
    }

    fn capture_tree_into(path: &Path, rel: &str, tree: &mut BTreeMap<String, TreeNode>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        if ensure_not_reparse_point(path, &metadata).is_err() {
            tree.insert(rel.to_owned(), TreeNode::Other);
            return;
        }
        let file_type = metadata.file_type();
        if file_type.is_file() {
            tree.insert(rel.to_owned(), TreeNode::File(fs::read(path).unwrap()));
            return;
        }
        if file_type.is_dir() {
            if !rel.is_empty() {
                tree.insert(rel.to_owned(), TreeNode::Directory);
            }
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().into_string().unwrap();
                let child_rel = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                capture_tree_into(&entry.path(), &child_rel, tree);
            }
            return;
        }
        tree.insert(rel.to_owned(), TreeNode::Other);
    }

    fn plant_sentinel(journal: &Path) {
        fs::create_dir_all(journal.join("keep")).unwrap();
        fs::write(journal.join("keep/sentinel.txt"), b"keep").unwrap();
    }

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

    #[test]
    fn capture_walks_multi_level_entries_in_lexical_order() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        fs::create_dir_all(journal.join("tree/z")).unwrap();
        fs::write(journal.join("tree/z/file.txt"), b"z").unwrap();
        fs::create_dir_all(journal.join("tree/m")).unwrap();
        fs::write(journal.join("tree/m/z.txt"), b"mz").unwrap();
        fs::write(journal.join("tree/m/a.txt"), b"ma").unwrap();
        fs::create_dir_all(journal.join("tree/a")).unwrap();
        fs::write(journal.join("tree/a/file.txt"), b"a").unwrap();

        let captured = capture_snapshot(&journal, "tree").unwrap();
        assert_eq!(
            snapshot_paths(&captured),
            vec![
                "tree",
                "tree/a",
                "tree/a/file.txt",
                "tree/m",
                "tree/m/a.txt",
                "tree/m/z.txt",
                "tree/z",
                "tree/z/file.txt",
            ]
        );
    }

    #[test]
    fn restores_every_desired_kind_against_every_observed_kind() {
        #[derive(Clone, Copy)]
        enum Desired {
            Missing,
            File,
            Directory,
        }
        #[derive(Clone, Copy)]
        enum Observed {
            Absent,
            File,
            Directory,
        }

        let desired_file = JournalSnapshot::File(SnapshotFile {
            path: "target".to_owned(),
            bytes: b"captured".to_vec(),
            mode: 0o644,
        });
        let desired_directory = JournalSnapshot::Directory(SnapshotDirectory {
            path: "target".to_owned(),
            entries: vec![JournalSnapshot::File(SnapshotFile {
                path: "target/child.txt".to_owned(),
                bytes: b"child".to_vec(),
                mode: 0o644,
            })],
        });
        let desired_missing = JournalSnapshot::Missing {
            path: "target".to_owned(),
        };

        for desired in [Desired::Missing, Desired::File, Desired::Directory] {
            for observed in [Observed::Absent, Observed::File, Observed::Directory] {
                let temporary = TempDir::new();
                let journal = temporary.path().join("journal");
                plant_sentinel(&journal);
                let keep_before = capture_tree(&journal.join("keep"));

                match observed {
                    Observed::Absent => {}
                    Observed::File => {
                        fs::write(journal.join("target"), b"observed").unwrap();
                    }
                    Observed::Directory => {
                        fs::create_dir_all(journal.join("target")).unwrap();
                        fs::write(journal.join("target/other.txt"), b"other").unwrap();
                    }
                }

                let snapshot = match desired {
                    Desired::Missing => &desired_missing,
                    Desired::File => &desired_file,
                    Desired::Directory => &desired_directory,
                };
                restore_snapshot(&journal, snapshot).unwrap();

                match desired {
                    Desired::Missing => {
                        assert!(!journal.join("target").exists());
                    }
                    Desired::File => {
                        assert_eq!(fs::read(journal.join("target")).unwrap(), b"captured");
                    }
                    Desired::Directory => {
                        assert_eq!(
                            fs::read(journal.join("target/child.txt")).unwrap(),
                            b"child"
                        );
                        assert!(!journal.join("target/other.txt").exists());
                    }
                }
                assert_eq!(capture_tree(&journal.join("keep")), keep_before);

                let mut expected = BTreeMap::from([
                    ("keep".to_owned(), TreeNode::Directory),
                    (
                        "keep/sentinel.txt".to_owned(),
                        TreeNode::File(b"keep".to_vec()),
                    ),
                ]);
                match desired {
                    Desired::Missing => {}
                    Desired::File => {
                        expected.insert("target".to_owned(), TreeNode::File(b"captured".to_vec()));
                    }
                    Desired::Directory => {
                        expected.insert("target".to_owned(), TreeNode::Directory);
                        expected.insert(
                            "target/child.txt".to_owned(),
                            TreeNode::File(b"child".to_vec()),
                        );
                    }
                }
                assert_eq!(capture_tree(&journal), expected);
            }
        }
    }

    #[test]
    fn restore_rejects_an_escaped_snapshot_path() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        plant_sentinel(&journal);
        let keep_before = capture_tree(&journal.join("keep"));

        assert!(matches!(
            restore_snapshot(
                &journal,
                &JournalSnapshot::Missing {
                    path: "entities/../../outside".to_owned(),
                }
            ),
            Err(SnapshotError::Path(_))
        ));
        assert_eq!(capture_tree(&journal.join("keep")), keep_before);
    }

    #[test]
    fn restore_rejects_a_child_not_contained_by_its_parent() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        plant_sentinel(&journal);
        fs::create_dir_all(journal.join("entities")).unwrap();
        fs::write(journal.join("entities/keep.txt"), b"stay").unwrap();
        let keep_before = capture_tree(&journal.join("keep"));
        let entities_before = capture_tree(&journal.join("entities"));

        let snapshot = JournalSnapshot::Directory(SnapshotDirectory {
            path: "entities".to_owned(),
            entries: vec![JournalSnapshot::Missing {
                path: "other".to_owned(),
            }],
        });
        assert!(matches!(
            restore_snapshot(&journal, &snapshot),
            Err(SnapshotError::InvalidSnapshot { path, .. }) if path == "other"
        ));
        assert_eq!(capture_tree(&journal.join("keep")), keep_before);
        assert_eq!(capture_tree(&journal.join("entities")), entities_before);
    }
}
