// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's choice to also remove what the trash erases from their backups:
//! which erases leave an entry for the backup, and what it holds.

use super::*;
use crate::tests::tempfile_dir;
use chrono::Duration;
use solstone_core_removals::Page;
use std::fs;

const RECORD: &str = "imports/20261101_090000";
const SEGMENT: &str = "chronicle/20261101/import.document/090000_300";

struct Open;
impl Hold for Open {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

/// A segment that needs its import's record, and leaves a tombstone where it was.
struct Kind1;
impl Kind for Kind1 {
    fn handles(&self, _: &Unit) -> bool {
        true
    }
    fn hold<'a>(&'a self, _: &Path, _: &Item) -> Result<Box<dyn Hold + 'a>, String> {
        Ok(Box::new(Open))
    }
    fn present(&self, journal: &Path, path: &str) -> bool {
        let place = journal.join(path);
        place.exists() && !place.join("tombstone.json").exists()
    }
    fn may_be_needed(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Import { .. })
    }
    fn needs(&self, _: &Path, unit: &Unit) -> Vec<String> {
        match unit {
            Unit::Segment { .. } => vec![RECORD.to_owned()],
            _ => Vec::new(),
        }
    }
    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        _: &[String],
    ) -> Result<(), String> {
        if change == Change::Detached && matches!(item.unit, Unit::Segment { .. }) {
            let place = journal.join(SEGMENT);
            fs::create_dir_all(&place).unwrap();
            fs::write(
                place.join("tombstone.json"),
                format!("{{\"cid\":\"{}\"}}", item.id),
            )
            .unwrap();
        }
        Ok(())
    }
}

fn journal(backup_on: bool, setting: Option<bool>) -> (tempfile_dir::Dir, PathBuf) {
    let dir = tempfile_dir::Dir::new();
    let root = dir.path().to_path_buf();
    for path in [RECORD, SEGMENT] {
        fs::create_dir_all(root.join(path)).unwrap();
    }
    fs::write(root.join(RECORD).join("talk.m4a"), b"upload").unwrap();
    fs::write(root.join(SEGMENT).join("audio.flac"), b"one").unwrap();
    let mut config = serde_json::json!({"backup": {"enabled": backup_on}});
    if let Some(on) = setting {
        config["trash"] = serde_json::json!({ ERASE_FROM_BACKUPS_KEY: on });
    }
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/journal.json"), config.to_string()).unwrap();
    (dir, root)
}

fn web() -> Surface {
    Surface::Web {
        page: Page::Settings,
    }
}

fn trash_segment(root: &Path) -> Item {
    detach(
        root,
        &Kind1,
        Unit::Segment {
            day: "20261101".into(),
            stream: "import.document".into(),
            segment: "090000_300".into(),
        },
        Action::SegmentDelete,
        web(),
        vec![SEGMENT.into()],
    )
    .unwrap()
}

fn trash_import(root: &Path) -> Item {
    detach(
        root,
        &Kind1,
        Unit::Import {
            source: "audio".into(),
            id: "20261101_090000".into(),
        },
        Action::ImportDelete,
        web(),
        vec![RECORD.into()],
    )
    .unwrap()
}

fn pending_ids(root: &Path) -> Vec<String> {
    backup_erase::pending(root)
        .0
        .into_iter()
        .map(|entry| entry.id)
        .collect()
}

#[test]
fn erase_with_the_choice_leaves_an_entry_before_the_files_go() {
    let (_dir, root) = journal(true, None);
    let item = trash_segment(&root);
    assert!(item.settled_at.is_some(), "a settled item says when");
    erase_with(&root, &[&Kind1], &item.id, Action::Erase, web(), None, true).unwrap();
    assert!(
        !trash_root(&root).join(&item.id).exists(),
        "the local erase ran at once"
    );
    let (entries, unreadable) = backup_erase::pending(&root);
    assert!(unreadable.is_empty());
    let entry = &entries[0];
    assert_eq!(entry.id, item.id);
    assert_eq!(entry.paths, vec![SEGMENT.to_owned()]);
    assert_eq!(entry.trash_path, format!("{TRASH_DIR}/{}", item.id));
    assert_eq!(entry.settled(), item.settled_at.as_deref().unwrap());
    assert_eq!(entry.surface, web());
    // Never copied or restored with the journal.
    assert!(solstone_core_journal_io::copy_excludes::copy_excluded(
        &format!("{}/{}.json", backup_erase::DIR, item.id)
    ));
}

#[test]
fn unticked_or_with_backup_off_it_leaves_nothing() {
    let (_dir, root) = journal(true, Some(true));
    let item = trash_segment(&root);
    erase_with(
        &root,
        &[&Kind1],
        &item.id,
        Action::Erase,
        web(),
        None,
        false,
    )
    .unwrap();
    assert!(
        pending_ids(&root).is_empty(),
        "unticked, even with the setting on"
    );

    let (_dir, root) = journal(false, None);
    let item = trash_segment(&root);
    erase_with(&root, &[&Kind1], &item.id, Action::Erase, web(), None, true).unwrap();
    assert!(
        pending_ids(&root).is_empty(),
        "no backup set up, nothing to remove it from"
    );
}

#[test]
fn what_an_erase_takes_with_it_carries_the_choice() {
    let (_dir, root) = journal(true, None);
    let segment = trash_segment(&root);
    let import = trash_import(&root);
    // Erasing the import erases the segment that needs it, under the same act.
    erase_with(
        &root,
        &[&Kind1],
        &import.id,
        Action::Erase,
        web(),
        None,
        true,
    )
    .unwrap();
    let mut ids = pending_ids(&root);
    ids.sort();
    let mut want = vec![segment.id, import.id];
    want.sort();
    assert_eq!(ids, want);
}

#[test]
fn the_timer_follows_the_setting() {
    for (setting, want) in [(Some(true), 1), (Some(false), 0), (None, 0)] {
        let (_dir, root) = journal(true, setting);
        trash_segment(&root);
        let later = Utc::now() + Duration::days(i64::from(MAX_ERASE_AFTER_DAYS) + 1);
        let swept = sweep(&root, &[&Kind1], later);
        assert_eq!(swept.erased, 1);
        assert_eq!(pending_ids(&root).len(), want, "setting {setting:?}");
    }
}

#[test]
fn a_resumed_erase_keeps_the_owners_choice() {
    let (_dir, root) = journal(true, Some(false));
    let item = trash_segment(&root);
    // A stop after the item was marked erasing with the choice ticked.
    let mut marked = read_item(&root, &item.id).unwrap().unwrap();
    marked.state = State::Erasing;
    marked.pending = Some(Pending {
        action: Action::Erase,
        surface: web(),
        at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        operation: None,
        backups: true,
    });
    publish(&root, &marked).unwrap();
    let swept = sweep(&root, &[&Kind1], Utc::now());
    assert_eq!(swept.resumed, 1);
    assert_eq!(pending_ids(&root), vec![item.id.clone()]);
    // Retiring is idempotent.
    backup_erase::retire(&root, &item.id).unwrap();
    backup_erase::retire(&root, &item.id).unwrap();
    assert!(pending_ids(&root).is_empty());
}

#[test]
fn the_setting_reads_off_unless_set_on() {
    let (_dir, root) = journal(true, None);
    assert!(!erase_from_backups(&root));
    fs::write(root.join("config/journal.json"), "not json").unwrap();
    assert!(!erase_from_backups(&root));
    let (_dir, root) = journal(true, Some(true));
    assert!(erase_from_backups(&root));
    assert!(backup_erase::backups_on(&root));
}
