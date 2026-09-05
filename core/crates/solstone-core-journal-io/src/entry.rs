// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Contained operations on a single directory entry.
//!
//! [`remove_dir_all`](crate::removal::remove_dir_all) resolves the *target* it is
//! about to delete, which is right for a recursive tree. `unlink(2)` and
//! `rename(2)` are different: they mutate a **directory entry**, and they do not
//! follow the final path component. So these resolve the **parent** and validate
//! the leaf as a plain component.
//!
//! That is not a stylistic difference, and getting it wrong is measurable:
//! resolving the leaf through [`contained_path`] returns
//! `Err(Io { NotFound, .. })` for a **dangling symlink**, which is
//! indistinguishable at the caller from the `NotFound` a genuinely absent entry
//! produces after `unlink`. A caller that treats the two alike reports *already
//! gone* for an entry still on disk that it failed to remove — and for a
//! whole-segment removal, one dangling symlink inside a segment would make the
//! owner's delete fail forever with a reason that says the file was not there.
//! Resolving the leaf is also over-strict: it refuses a symlink pointing outside
//! the journal, which `unlink` would have removed safely, leaving the entry
//! permanently un-removable.

use std::fs;
use std::io;
#[cfg(unix)]
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use crate::errors::PathError;
use crate::paths::{contained_path, path_lexists};

/// Whether a directory entry was there to begin with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Removed {
    /// The entry existed and this call unlinked it.
    Unlinked,
    /// The entry was already absent. Nothing was removed by this call.
    ///
    /// ⛔ Callers reporting to an owner must not describe this as a removal they
    /// performed. It is the difference between *"I deleted this"* and *"this was
    /// already gone"*, and a receipt that conflates them describes a run that did
    /// not happen.
    AlreadyAbsent,
}

/// Split a journal-relative path into its parent's rel and its leaf name.
///
/// A single-component `rel` has the journal root as its parent, expressed as the
/// empty string, which [`contained_path`] rejects — so the root is resolved
/// directly for that case.
fn split_leaf(rel: &str) -> Result<(Option<&str>, &str), PathError> {
    let trimmed = rel.trim_end_matches('/');
    let leaf = trimmed.rsplit('/').next().unwrap_or_default();
    if leaf.is_empty() || leaf == "." || leaf == ".." {
        return Err(PathError::InvalidRelativePath {
            rel: rel.to_owned(),
            message: "journal path must end in a plain component",
        });
    }
    let parent = trimmed
        .len()
        .checked_sub(leaf.len())
        .and_then(|end| end.checked_sub(1))
        .map(|end| &trimmed[..end]);
    Ok((parent.filter(|value| !value.is_empty()), leaf))
}

/// Resolve `rel`'s parent below `root` and return `parent/leaf`.
///
/// The parent is resolved through [`contained_path`] immediately before the
/// caller's syscall, so a symlinked parent that escapes the journal is refused
/// while a symlinked *leaf* stays operable.
fn contained_entry(root: &Path, rel: &str) -> Result<PathBuf, PathError> {
    let (parent, leaf) = split_leaf(rel)?;
    let directory = match parent {
        Some(parent) => contained_path(root, parent)?,
        // A single-component entry sits in the journal root, whose containment is
        // trivially satisfied. `contained_path` cannot express it: it rejects `.`
        // and the empty string, so resolve the root itself.
        None => fs::canonicalize(root).map_err(|source| PathError::Io {
            path: root.to_path_buf(),
            source,
        })?,
    };
    Ok(directory.join(leaf))
}

/// Remove the single entry `rel` below `root`, reporting whether it was there.
///
/// ⚠ Unlike [`remove_dir_all`](crate::removal::remove_dir_all), an absent target
/// is **not** silently a success: it returns [`Removed::AlreadyAbsent`] so a
/// caller can describe its own run truthfully. ⛔ And an entry this call could
/// not *inspect* is an error, never an absence — see [`Removed::AlreadyAbsent`].
///
/// Removes the entry, not what it points at: a symlink is unlinked even when its
/// target lies outside the journal.
pub fn remove_file(root: &Path, rel: &str) -> Result<Removed, PathError> {
    let path = contained_entry(root, rel)?;
    // ⛔ Never `unwrap_or(false)`. `path_lexists` reports an inspection failure
    // (a permission error on the parent, say) as `Err`, and folding that into
    // "absent" would authorize an irreversible claim about a file nothing looked
    // at. Unknown is not no.
    if !path_lexists(&path)? {
        return Ok(Removed::AlreadyAbsent);
    }
    match fs::remove_file(&path) {
        Ok(()) => Ok(Removed::Unlinked),
        // Lost a race with another remover between the check and the syscall.
        // Reported as the absence it is, not as a removal this call performed.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Removed::AlreadyAbsent),
        Err(source) => Err(PathError::Io { path, source }),
    }
}

/// Rename `from_rel` to `to_rel`, both contained below `root`.
///
/// Both ends' parents are resolved immediately before the syscall, because
/// `rename(2)` follows symlinks in prefix components.
///
/// ⛔ **Does not inspect the destination.** `rename(2)` silently replaces an
/// existing empty directory and fails `ENOTEMPTY` on a non-empty one, so a caller
/// staging owner data owns the collision refusal. Losing that refusal destroys
/// whatever occupied the destination.
pub fn rename_within(root: &Path, from_rel: &str, to_rel: &str) -> Result<(), PathError> {
    let from = contained_entry(root, from_rel)?;
    let to = contained_entry(root, to_rel)?;
    fs::rename(&from, &to).map_err(|source| PathError::Io { path: from, source })
}

/// Flush the directory `dir_rel`'s own entries to disk.
///
/// 🔴 Returns its error. The crate's internal `fsync_dir` warns and returns
/// success, which cannot carry a durability claim: if a record says a removal
/// happened at a given moment, the removal has to be durable at that moment.
/// `fsync` on a file does not persist the directory entry naming it, so a
/// caller that has just unlinked something and intends to say so must call this.
///
/// ⚠ Takes the **directory's** own rel, not a file's. A parameter meaning
/// "the parent of this file" invites a caller to sync the wrong directory and be
/// told `Ok`.
pub fn sync_dir(root: &Path, dir_rel: &str) -> Result<(), PathError> {
    let path = contained_path(root, dir_rel)?;
    let directory = fs::File::open(&path).map_err(|source| PathError::Io {
        path: path.clone(),
        source,
    })?;
    directory
        .sync_all()
        .map_err(|source| PathError::Io { path, source })
}

/// Flush the journal root's entries, including newly created domain directories.
#[cfg(unix)]
pub fn sync_root(root: &Path) -> Result<(), PathError> {
    fs::File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| PathError::Io {
            path: root.to_owned(),
            source,
        })
}

/// Flush an already-bound directory's own entries to disk.
///
/// Returns its error. This never opens a directory via `AT_FDCWD`.
#[cfg(unix)]
pub(crate) fn sync_dir_bound(directory: &impl AsFd) -> Result<(), io::Error> {
    nix::unistd::fsync(directory).map_err(|error| io::Error::from_raw_os_error(error as i32))
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::test_support::TempDir;

    fn journal() -> TempDir {
        let temporary = TempDir::new();
        fs::create_dir_all(temporary.path().join("chronicle/20260805/field.audio")).unwrap();
        temporary
    }

    #[test]
    fn removes_a_contained_entry_and_reports_it() {
        let temporary = journal();
        let rel = "chronicle/20260805/field.audio/audio.flac";
        fs::write(temporary.path().join(rel), b"bytes").unwrap();
        assert_eq!(
            remove_file(temporary.path(), rel).unwrap(),
            Removed::Unlinked
        );
        assert!(!temporary.path().join(rel).exists());
    }

    #[test]
    fn an_absent_entry_is_reported_as_already_absent_not_as_a_removal() {
        let temporary = journal();
        assert_eq!(
            remove_file(temporary.path(), "chronicle/20260805/field.audio/gone.flac").unwrap(),
            Removed::AlreadyAbsent
        );
    }

    /// A dangling symlink is present and removable, and must not read as absent.
    ///
    /// Resolving the leaf through `contained_path` yields `Err(Io { NotFound })`
    /// here, which is exactly what an absent entry looks like to a caller.
    #[test]
    fn a_dangling_symlink_is_removed_and_never_reported_as_absent() {
        let temporary = journal();
        let rel = "chronicle/20260805/field.audio/dangling.flac";
        symlink("/nonexistent/target", temporary.path().join(rel)).unwrap();
        assert_eq!(
            remove_file(temporary.path(), rel).unwrap(),
            Removed::Unlinked,
            "a dangling symlink is an entry that exists; removing it is a removal"
        );
        assert!(!path_lexists(&temporary.path().join(rel)).unwrap());
    }

    /// `unlink` does not follow the final component, so an escaping symlink leaf
    /// is removable — refusing it would leave the entry permanently stuck.
    #[test]
    fn a_symlink_leaf_pointing_outside_the_journal_is_removed_not_refused() {
        let temporary = TempDir::new();
        let root = temporary.path().join("journal");
        fs::create_dir_all(root.join("chronicle/20260805/field.audio")).unwrap();
        let outside = temporary.path().join("outside.flac");
        fs::write(&outside, b"survives").unwrap();
        let rel = "chronicle/20260805/field.audio/escaping.flac";
        symlink(&outside, root.join(rel)).unwrap();

        assert_eq!(remove_file(&root, rel).unwrap(), Removed::Unlinked);
        assert!(!path_lexists(&root.join(rel)).unwrap());
        assert!(outside.exists(), "the target outside the journal survives");
        assert_eq!(fs::read(&outside).unwrap(), b"survives");
    }

    #[test]
    fn a_symlinked_parent_escaping_the_journal_is_refused() {
        let temporary = TempDir::new();
        let root = temporary.path().join("journal");
        fs::create_dir_all(root.join("chronicle/20260805")).unwrap();
        let outside = temporary.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("audio.flac"), b"keep").unwrap();
        symlink(&outside, root.join("chronicle/20260805/field.audio")).unwrap();

        let error = remove_file(&root, "chronicle/20260805/field.audio/audio.flac").unwrap_err();
        assert!(matches!(error, PathError::Escape(_)), "got {error:?}");
        assert!(outside.join("audio.flac").exists());
    }

    #[test]
    fn a_literal_escape_is_refused() {
        let temporary = journal();
        let error = remove_file(temporary.path(), "chronicle/../../outside.flac").unwrap_err();
        assert!(
            matches!(
                error,
                PathError::Escape(_) | PathError::InvalidRelativePath { .. }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn a_single_component_entry_resolves_against_the_journal_root() {
        let temporary = journal();
        fs::write(temporary.path().join("tombstone.json"), b"{}").unwrap();
        assert_eq!(
            remove_file(temporary.path(), "tombstone.json").unwrap(),
            Removed::Unlinked
        );
    }

    #[test]
    fn a_trailing_component_that_is_not_plain_is_refused() {
        let temporary = journal();
        for rel in ["chronicle/20260805/.", "chronicle/20260805/..", ""] {
            assert!(remove_file(temporary.path(), rel).is_err(), "{rel:?}");
        }
    }

    #[test]
    fn renames_within_the_journal_and_moves_the_bytes() {
        let temporary = journal();
        let from = "chronicle/20260805/field.audio/070000_17";
        let to = "chronicle/20260805/field.audio/.removing_070000_17";
        fs::create_dir_all(temporary.path().join(from)).unwrap();
        fs::write(temporary.path().join(from).join("audio.flac"), b"bytes").unwrap();

        rename_within(temporary.path(), from, to).unwrap();

        assert!(!temporary.path().join(from).exists());
        assert_eq!(
            fs::read(temporary.path().join(to).join("audio.flac")).unwrap(),
            b"bytes"
        );
    }

    #[test]
    fn a_rename_whose_source_parent_escapes_is_refused() {
        let temporary = TempDir::new();
        let root = temporary.path().join("journal");
        fs::create_dir_all(root.join("chronicle/20260805")).unwrap();
        let outside = temporary.path().join("elsewhere");
        fs::create_dir_all(outside.join("070000_17")).unwrap();
        symlink(&outside, root.join("chronicle/20260805/field.audio")).unwrap();

        let error = rename_within(
            &root,
            "chronicle/20260805/field.audio/070000_17",
            "chronicle/20260805/field.audio/.removing_070000_17",
        )
        .unwrap_err();
        assert!(matches!(error, PathError::Escape(_)), "got {error:?}");
        assert!(outside.join("070000_17").exists());
    }

    #[test]
    fn a_rename_whose_destination_parent_escapes_is_refused() {
        let temporary = TempDir::new();
        let root = temporary.path().join("journal");
        fs::create_dir_all(root.join("chronicle/20260805/field.audio/070000_17")).unwrap();
        let outside = temporary.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("chronicle/20260805/away")).unwrap();

        let error = rename_within(
            &root,
            "chronicle/20260805/field.audio/070000_17",
            "chronicle/20260805/away/070000_17",
        )
        .unwrap_err();
        assert!(matches!(error, PathError::Escape(_)), "got {error:?}");
        assert!(
            temporary
                .path()
                .join("journal/chronicle/20260805/field.audio/070000_17")
                .exists()
        );
    }

    #[test]
    fn syncs_an_existing_directory() {
        let temporary = journal();
        sync_dir(temporary.path(), "chronicle/20260805/field.audio").unwrap();
    }

    #[test]
    fn sync_dir_bound_syncs_an_open_directory() {
        use nix::fcntl::{AT_FDCWD, OFlag, openat};
        use nix::sys::stat::Mode;

        let temporary = journal();
        let directory = openat(
            AT_FDCWD,
            &temporary.path().join("chronicle/20260805/field.audio"),
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::empty(),
        )
        .unwrap();
        sync_dir_bound(&directory).unwrap();
    }

    /// A missing directory is an error, not a silent success.
    ///
    /// This is the route pinned deliberately: a *regular file* passed here
    /// returns `Ok` on Linux, so a "the parent is a file" test proves nothing, and
    /// a mode-`0o000` directory succeeds under root.
    #[test]
    fn syncing_a_missing_directory_returns_its_error() {
        let temporary = journal();
        let error = sync_dir(temporary.path(), "chronicle/20260806/field.audio").unwrap_err();
        assert!(
            matches!(
                error,
                PathError::Io { .. } | PathError::Escape(_) | PathError::InvalidRelativePath { .. }
            ),
            "got {error:?}"
        );
    }
}
