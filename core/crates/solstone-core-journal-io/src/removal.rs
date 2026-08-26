// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Contained recursive directory removal.

use std::fs;
use std::io;
use std::path::Path;

use crate::errors::PathError;
use crate::paths::{contained_path, realpath_non_strict};

/// Recursively remove `rel` below `root`, treating an absent target as removed.
///
/// The target is resolved through [`contained_path`] immediately before removal,
/// so literal and symlink-aware escapes are refused before the filesystem changes.
/// Callers that need mutual exclusion must hold it separately.
pub fn remove_dir_all(root: &Path, rel: &str) -> Result<(), PathError> {
    let path = contained_path(root, rel)?;
    match fs::remove_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(PathError::Io { path, source }),
    }
}

/// Recursively remove an already-resolved path contained in `root`.
///
/// `path` is the discovered directory, not a UTF-8 relative string. An absent
/// target is treated as removed, matching [`remove_dir_all`].
pub fn remove_contained_tree(root: &Path, path: &Path) -> Result<(), PathError> {
    let root_resolved = realpath_non_strict(root)?;
    let path_resolved = realpath_non_strict(path)?;
    if path_resolved == root_resolved {
        return Err(PathError::InvalidRelativePath {
            rel: path_resolved.to_string_lossy().into_owned(),
            message: "cannot remove the journal root",
        });
    }
    if !path_resolved.starts_with(&root_resolved) {
        return Err(PathError::Escape(crate::errors::PathEscapeError {
            path: path_resolved,
            rel: path.to_string_lossy().into_owned(),
        }));
    }
    match fs::remove_dir_all(&path_resolved) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(PathError::Io {
            path: path_resolved,
            source,
        }),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn removes_a_contained_tree_and_keeps_its_parent() {
        let temporary = TempDir::new();
        let parent = temporary.path().join("entities");
        let target = parent.join("alice/history");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("event.json"), b"event").unwrap();

        remove_dir_all(temporary.path(), "entities/alice").unwrap();

        assert!(parent.is_dir());
        assert!(!parent.join("alice").exists());
    }

    #[test]
    fn removing_an_absent_path_is_idempotent() {
        let temporary = TempDir::new();

        remove_dir_all(temporary.path(), "entities/missing").unwrap();

        assert!(!temporary.path().join("entities/missing").exists());
    }

    #[test]
    fn rejects_a_literal_escape_before_removing_anything() {
        let temporary = TempDir::new();
        let root = temporary.path().join("journal");
        let outside = temporary.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep"), b"keep").unwrap();

        assert!(matches!(
            remove_dir_all(&root, "../outside"),
            Err(PathError::InvalidRelativePath { .. })
        ));
        assert!(outside.join("keep").is_file());
    }

    #[test]
    fn rejects_a_symlink_escape_before_removing_anything() {
        let temporary = TempDir::new();
        let root = temporary.path().join("journal");
        let outside = temporary.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"keep").unwrap();
        symlink(&outside, root.join("escape")).unwrap();

        assert!(matches!(
            remove_dir_all(&root, "escape/tree"),
            Err(PathError::Escape(_))
        ));
        assert!(outside.join("keep").is_file());
    }

    #[test]
    fn removal_failure_is_a_path_io_error() {
        let temporary = TempDir::new();
        let target = temporary.path().join("file");
        fs::write(&target, b"not a directory").unwrap();

        let error = remove_dir_all(temporary.path(), "file").unwrap_err();

        assert!(matches!(error, PathError::Io { path, .. } if path == target));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn remove_contained_tree_deletes_a_non_utf8_directory() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let temporary = TempDir::new();
        let root = temporary.path().join("journal");
        let target = root.join(OsStr::from_bytes(b"seg\xff"));
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), b"gone").unwrap();

        remove_contained_tree(&root, &target).unwrap();

        assert!(!target.exists());
        assert!(root.is_dir());
    }

    #[test]
    fn remove_contained_tree_refuses_the_journal_root() {
        let temporary = TempDir::new();
        let root = temporary.path().join("journal");
        fs::create_dir(&root).unwrap();

        assert!(matches!(
            remove_contained_tree(&root, &root),
            Err(PathError::InvalidRelativePath { .. })
        ));
        assert!(root.is_dir());
    }
}
