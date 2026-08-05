// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Contained recursive directory removal.

use std::fs;
use std::io;
use std::path::Path;

use crate::errors::PathError;
use crate::paths::contained_path;

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

#[cfg(test)]
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
}
