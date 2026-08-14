// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One-time relocation of root-level `YYYYMMDD` day directories into
//! `chronicle/`.
//!
//! A day directory that predates `chronicle/` sits directly under the journal
//! root. Moving it is a rename when the destination is free. When the
//! destination already exists — the race where current code created
//! `chronicle/<day>` before this migration ran — the two trees are merged and
//! the root copy is removed, so no day ends up split across both locations.
//!
//! The merge moves entries rather than copying bytes. Both trees live inside
//! the journal, so a rename is the cheap equivalent of the copy-then-delete the
//! retired Python migration performed, and it reaches the same end state: the
//! destination holds the source's files, a colliding destination file is
//! replaced by the source's, and the source is gone.
//!
//! ⚠ A root day directory cannot be addressed by journal-relative path.
//! `journal-io`'s `resolve_journal_path` rewrites any rel whose first component
//! is a `YYYYMMDD` key into `chronicle/<rel>`, so `20260304/090000_60/a.txt`
//! resolves to the *destination* of this very migration. Only a single-component
//! rel escapes that rewrite, because the entry API resolves the journal root
//! itself rather than the rel. So the merge first renames the whole root tree
//! into `chronicle/` under a staging name — one syscall, one safe rel — and only
//! then merges with rels that all start with `chronicle`.

use std::error::Error;
use std::fmt;
use std::path::Path;

use solstone_core_journal_io::{
    DirEntryKind, ensure_directory, list_dir_entries, remove_dir_all, rename_within,
};

/// A chronicle migration step that could not be completed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChronicleMigrationError(String);

impl ChronicleMigrationError {
    /// Describe a chronicle migration failure.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ChronicleMigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ChronicleMigrationError {}

/// Name prefix for the in-`chronicle/` staging directory a merge moves through.
///
/// Leading dot keeps it out of day enumeration; the non-day first component is
/// what makes every rel below it addressable at all.
const STAGING_PREFIX: &str = ".maint-merge-";

/// What one root-day-to-chronicle migration run relocated.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChronicleMigrationReport {
    /// Day directories relocated (or, when `dry_run`, planned).
    pub moved: usize,
    /// Day directories merged into an existing `chronicle/<day>`.
    pub merged: usize,
    /// Day directories deliberately left alone.
    pub skipped: usize,
    /// Whether the run planned only.
    pub dry_run: bool,
}

impl ChronicleMigrationReport {
    /// Whether this run did enough to invalidate derived index state.
    ///
    /// The retired migration deleted the index and validated the end state only
    /// when it had actually relocated something outside a dry run, so callers
    /// gate their index cleanup on exactly this.
    pub fn requires_index_cleanup(&self) -> bool {
        !self.dry_run && (self.moved > 0 || self.skipped > 0)
    }
}

/// Move every root-level `YYYYMMDD` directory into `chronicle/`.
///
/// A completed run proves no root day directory survived and that `chronicle/`
/// exists; either failure is an error rather than a quiet partial migration.
pub fn migrate_root_days_to_chronicle(
    journal: &Path,
    dry_run: bool,
) -> Result<ChronicleMigrationReport, ChronicleMigrationError> {
    let mut report = ChronicleMigrationReport {
        dry_run,
        ..ChronicleMigrationReport::default()
    };
    let days = root_day_dirs(journal)?;
    if days.is_empty() {
        return Ok(report);
    }

    let chronicle = journal.join("chronicle");
    if !dry_run {
        ensure_directory(&chronicle).map_err(|error| ChronicleMigrationError(error.to_string()))?;
    }

    for day in days {
        let target = chronicle.join(&day);
        if target.exists() {
            if !dry_run {
                let staging = format!("chronicle/{STAGING_PREFIX}{day}");
                rename_within(journal, &day, &staging)
                    .map_err(|error| ChronicleMigrationError(error.to_string()))?;
                merge_directory(journal, &journal.join(&staging), &target)?;
                remove_dir_all(journal, &staging)
                    .map_err(|error| ChronicleMigrationError(error.to_string()))?;
            }
            report.merged += 1;
            report.moved += 1;
            continue;
        }
        if !dry_run {
            rename_within(journal, &day, &format!("chronicle/{day}"))
                .map_err(|error| ChronicleMigrationError(error.to_string()))?;
        }
        report.moved += 1;
    }

    if report.requires_index_cleanup() {
        validate_end_state(journal)?;
    }
    Ok(report)
}

/// Move every entry of `source` into `destination`, merging subdirectories.
fn merge_directory(
    journal: &Path,
    source: &Path,
    destination: &Path,
) -> Result<(), ChronicleMigrationError> {
    ensure_directory(destination).map_err(|error| ChronicleMigrationError(error.to_string()))?;
    for entry in
        list_dir_entries(source).map_err(|error| ChronicleMigrationError(error.to_string()))?
    {
        let name = entry.name.to_string_lossy().into_owned();
        let target = destination.join(&name);
        if entry.kind == DirEntryKind::Directory && target.exists() {
            merge_directory(journal, &entry.path, &target)?;
            continue;
        }
        // `rename(2)` replaces an existing file, which is what the retired
        // `copytree(dirs_exist_ok=True)` did to a colliding destination file.
        rename_within(
            journal,
            &relative(journal, &entry.path)?,
            &relative(journal, &target)?,
        )
        .map_err(|error| ChronicleMigrationError(error.to_string()))?;
    }
    Ok(())
}

/// Root-level `YYYYMMDD` directories, sorted by name.
fn root_day_dirs(journal: &Path) -> Result<Vec<String>, ChronicleMigrationError> {
    let mut days = list_dir_entries(journal)
        .map_err(|error| ChronicleMigrationError(error.to_string()))?
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::Directory)
        .map(|entry| entry.name.to_string_lossy().into_owned())
        .filter(|name| is_day_name(name))
        .collect::<Vec<_>>();
    days.sort();
    Ok(days)
}

/// Whether a directory name is exactly eight ASCII digits.
fn is_day_name(name: &str) -> bool {
    name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit())
}

/// Prove the migration actually finished before the caller acts on it.
fn validate_end_state(journal: &Path) -> Result<(), ChronicleMigrationError> {
    let remaining = root_day_dirs(journal)?;
    if !remaining.is_empty() {
        return Err(ChronicleMigrationError(format!(
            "root day directories remain after chronicle migration: {}",
            remaining.join(", ")
        )));
    }
    if !journal.join("chronicle").is_dir() {
        return Err(ChronicleMigrationError::new(
            "chronicle/ missing after chronicle migration",
        ));
    }
    Ok(())
}

fn relative(journal: &Path, path: &Path) -> Result<String, ChronicleMigrationError> {
    path.strip_prefix(journal)
        .ok()
        .and_then(|rel| rel.to_str())
        .map(str::to_owned)
        .ok_or_else(|| ChronicleMigrationError(format!("path is outside the journal: {path:?}")))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::test_support::TempDir;

    fn write(path: PathBuf, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
        fs::write(path, contents).expect("file written");
    }

    #[test]
    fn root_day_directories_move_under_chronicle() {
        let temp = TempDir::new();
        let root = temp.path();
        write(root.join("20260304/090000_60/audio.jsonl"), "one");
        write(root.join("20260305/100000_60/audio.jsonl"), "two");
        // Neither of these is a day directory.
        write(root.join("notes/readme.md"), "keep");
        fs::create_dir_all(root.join("2026030")).expect("short name");

        let report = migrate_root_days_to_chronicle(root, false).expect("migration runs");

        assert_eq!(report.moved, 2);
        assert_eq!(report.merged, 0);
        assert_eq!(
            fs::read_to_string(root.join("chronicle/20260304/090000_60/audio.jsonl"))
                .expect("moved content"),
            "one"
        );
        assert!(
            root.join("chronicle/20260305/100000_60/audio.jsonl")
                .is_file()
        );
        assert!(!root.join("20260304").exists());
        assert!(!root.join("20260305").exists());
        assert!(root.join("notes/readme.md").is_file());
        assert!(root.join("2026030").is_dir());
    }

    #[test]
    fn an_existing_chronicle_day_is_merged_and_the_root_copy_is_removed() {
        let temp = TempDir::new();
        let root = temp.path();
        write(root.join("20260304/090000_60/audio.jsonl"), "root");
        write(root.join("20260304/090000_60/only-root.txt"), "root only");
        write(root.join("20260304/110000_60/audio.jsonl"), "root segment");
        write(
            root.join("chronicle/20260304/090000_60/audio.jsonl"),
            "chronicle",
        );
        write(
            root.join("chronicle/20260304/100000_60/audio.jsonl"),
            "chronicle only",
        );

        let report = migrate_root_days_to_chronicle(root, false).expect("migration runs");

        assert_eq!(report.moved, 1);
        assert_eq!(report.merged, 1);
        assert!(!root.join("20260304").exists(), "the root copy is removed");
        assert_eq!(
            fs::read_to_string(root.join("chronicle/20260304/090000_60/audio.jsonl"))
                .expect("merged file"),
            "root",
            "the root copy wins a collision, as copytree overwrite did"
        );
        assert_eq!(
            fs::read_to_string(root.join("chronicle/20260304/090000_60/only-root.txt"))
                .expect("root-only file"),
            "root only"
        );
        assert_eq!(
            fs::read_to_string(root.join("chronicle/20260304/100000_60/audio.jsonl"))
                .expect("pre-existing file"),
            "chronicle only",
            "content only the destination had survives the merge"
        );
        assert!(
            root.join("chronicle/20260304/110000_60/audio.jsonl")
                .is_file()
        );
    }

    #[test]
    fn a_journal_with_no_root_days_is_a_no_op() {
        let temp = TempDir::new();
        write(
            temp.path().join("chronicle/20260304/090000_60/audio.jsonl"),
            "held",
        );

        let report = migrate_root_days_to_chronicle(temp.path(), false).expect("migration runs");

        assert_eq!(report, ChronicleMigrationReport::default());
        assert!(!report.requires_index_cleanup());
        assert!(
            temp.path()
                .join("chronicle/20260304/090000_60/audio.jsonl")
                .is_file()
        );
    }

    #[test]
    fn a_dry_run_plans_the_move_and_creates_nothing() {
        let temp = TempDir::new();
        let root = temp.path();
        write(root.join("20260304/090000_60/audio.jsonl"), "one");

        let report = migrate_root_days_to_chronicle(root, true).expect("migration plans");

        assert!(report.dry_run);
        assert_eq!(report.moved, 1);
        assert!(
            !report.requires_index_cleanup(),
            "a plan never invalidates the index"
        );
        assert!(root.join("20260304/090000_60/audio.jsonl").is_file());
        assert!(!root.join("chronicle").exists());
    }
}
