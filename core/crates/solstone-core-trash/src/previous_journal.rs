// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a whole-journal restore set aside: kept until the owner removes it,
//! then one item in the trash like anything else. A backup never carries it,
//! so it can't put old keys into new snapshots.

use std::path::Path;

use serde::Serialize;
use solstone_core_journal::BEFORE_RESTORE_PREFIX;
use solstone_core_removals::{Action, Surface, Unit};

use crate::{Change, Hold, Item, Kind, TrashError};

/// The kind for a set-aside folder. Nothing in the journal points into one.
pub struct PreviousJournal;

struct Nothing;

impl Hold for Nothing {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

/// A kept journal, as the backup page shows it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Kept {
    pub folder: String,
    pub stamp: String,
    pub bytes: u64,
}

fn stamp_of(name: &str) -> Option<&str> {
    name.strip_prefix(BEFORE_RESTORE_PREFIX)
        .filter(|stamp| !stamp.is_empty())
}

impl Kind for PreviousJournal {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::PreviousJournal)
    }

    fn hold<'a>(&'a self, _: &Path, _: &Item) -> Result<Box<dyn Hold + 'a>, String> {
        Ok(Box::new(Nothing))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        stamp_of(path).is_some()
            && std::fs::symlink_metadata(journal.join(path)).is_ok_and(|meta| meta.is_dir())
    }

    fn references(&self, _: &Path, _: &Item, _: Change, _: &[String]) -> Result<(), String> {
        Ok(())
    }
}

/// Every journal a restore kept, newest first.
pub fn kept(journal: &Path) -> std::io::Result<Vec<Kept>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(journal)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(stamp) = stamp_of(name)
            && entry.file_type()?.is_dir()
        {
            found.push(Kept {
                folder: name.to_owned(),
                stamp: stamp.to_owned(),
                bytes: crate::tree_bytes(&entry.path()),
            });
        }
    }
    found.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    Ok(found)
}

/// Move a kept journal to the trash, as one thing.
pub fn remove(journal: &Path, stamp: &str, surface: Surface) -> Result<Item, TrashError> {
    if !crate::valid_id(stamp) {
        return Err(TrashError::Missing("previous journal".to_owned()));
    }
    let folder = format!("{BEFORE_RESTORE_PREFIX}{stamp}");
    crate::detach_as(
        journal,
        &PreviousJournal,
        Unit::PreviousJournal,
        Action::PreviousJournalRemove,
        surface,
        vec![folder],
        Some(format!("previous-{stamp}")),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]
mod tests {
    use super::*;
    use solstone_core_removals::Page;
    use std::fs;

    const SURFACE: Surface = Surface::Web { page: Page::Backup };

    struct Root(std::path::PathBuf);

    impl Root {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn with_kept(name: &str) -> Root {
        let root = std::env::temp_dir().join(format!(
            "solstone-previous-journal-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".before-restore-1/notes")).unwrap();
        fs::write(root.join(".before-restore-1/notes/a.md"), b"12345").unwrap();
        Root(root)
    }

    #[test]
    fn a_kept_journal_is_listed_with_its_size() {
        let root = with_kept("listed");
        fs::write(root.path().join(".before-restore-file"), b"not a folder").unwrap();
        let kept = kept(root.path()).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].stamp, "1");
        assert_eq!(kept[0].bytes, 5);
    }

    #[test]
    fn removing_one_moves_it_whole_recoverably_and_recorded() {
        let root = with_kept("removed");
        let item = remove(root.path(), "1", SURFACE).unwrap();
        assert_eq!(item.id, "previous-1");
        assert!(!root.path().join(".before-restore-1").exists());
        assert!(kept(root.path()).unwrap().is_empty());
        let history = solstone_core_removals::history(root.path(), 10).unwrap();
        assert!(
            history
                .entries
                .iter()
                .any(|entry| entry.unit == Unit::PreviousJournal)
        );
        crate::attach(
            root.path(),
            &[&PreviousJournal],
            &item.id,
            Action::Recover,
            SURFACE,
        )
        .unwrap();
        assert_eq!(
            fs::read(root.path().join(".before-restore-1/notes/a.md")).unwrap(),
            b"12345"
        );
    }

    #[test]
    fn a_name_that_is_not_a_kept_journal_is_refused() {
        let root = with_kept("refused");
        fs::create_dir(root.path().join("notes")).unwrap();
        let refused = crate::detach_as(
            root.path(),
            &PreviousJournal,
            Unit::PreviousJournal,
            Action::PreviousJournalRemove,
            SURFACE,
            vec!["notes".to_owned()],
            None,
        );
        assert!(refused.is_err());
        assert!(root.path().join("notes").exists());
    }

    #[test]
    fn a_stamp_cannot_name_a_path() {
        let root = with_kept("traversal");
        fs::create_dir(root.path().join("notes")).unwrap();
        assert!(remove(root.path(), "../notes", SURFACE).is_err());
        assert!(root.path().join("notes").exists());
    }

    #[test]
    fn its_trash_item_is_kept_out_of_backups() {
        assert!(solstone_core_journal_io::copy_excludes::copy_excluded(
            ".trash/previous-1/item.json"
        ));
    }
}
