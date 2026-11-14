// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An item that needs a path another item holds: the holder never erases first,
//! and erasing it erases what needs it.

use super::*;
use crate::tests::tempfile_dir;
use chrono::Duration;
use solstone_core_removals::Page;
use std::fs;

const RECORD: &str = "imports/20261101_090000";
const FIRST: &str = "chronicle/20261101/import.document/090000_300";
const LAST: &str = "chronicle/20261101/import.document/093000_300";

struct Open;
impl Hold for Open {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

/// A segment needs its import's record; an import needs nothing.
struct Needing;
impl Kind for Needing {
    fn handles(&self, _: &Unit) -> bool {
        true
    }
    fn hold<'a>(&'a self, _: &Path, _: &Item) -> Result<Box<dyn Hold + 'a>, String> {
        Ok(Box::new(Open))
    }
    fn present(&self, journal: &Path, path: &str) -> bool {
        journal.join(path).exists()
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
    fn references(&self, _: &Path, _: &Item, _: Change, _: &[String]) -> Result<(), String> {
        Ok(())
    }
}

fn journal() -> (tempfile_dir::Dir, PathBuf) {
    let dir = tempfile_dir::Dir::new();
    let root = dir.path().to_path_buf();
    for path in [RECORD, FIRST, LAST] {
        fs::create_dir_all(root.join(path)).unwrap();
    }
    fs::write(root.join(RECORD).join("talk.m4a"), b"upload").unwrap();
    fs::write(root.join(FIRST).join("audio.flac"), b"one").unwrap();
    fs::write(root.join(LAST).join("audio.flac"), b"two").unwrap();
    (dir, root)
}

fn web() -> Surface {
    Surface::Web {
        page: Page::Transcripts,
    }
}

fn segment(path: &str) -> Unit {
    let (day, stream, segment) = {
        let parts: Vec<&str> = path.split('/').collect();
        (parts[1], parts[2], parts[3])
    };
    Unit::Segment {
        day: day.into(),
        stream: stream.into(),
        segment: segment.into(),
    }
}

fn import() -> Unit {
    Unit::Import {
        source: "audio".into(),
        id: "20261101_090000".into(),
    }
}

fn trash_first(root: &Path) -> Item {
    detach(
        root,
        &Needing,
        segment(FIRST),
        Action::SegmentDelete,
        web(),
        vec![FIRST.into()],
    )
    .unwrap()
}

fn trash_import(root: &Path) -> Item {
    detach(
        root,
        &Needing,
        import(),
        Action::SegmentDelete,
        web(),
        vec![RECORD.into(), LAST.into()],
    )
    .unwrap()
}

/// Rewrite an item's erase date, as a setting changed between two deletes.
fn redate(root: &Path, id: &str, at: DateTime<Utc>) {
    let mut item = read_item(root, id).unwrap().unwrap();
    item.erase_at = at.to_rfc3339_opts(SecondsFormat::Millis, true);
    publish(root, &item).unwrap();
}

#[test]
fn a_segment_names_what_it_needs_and_its_import_never_erases_first() {
    let (_dir, root) = journal();
    let first = trash_first(&root);
    assert_eq!(first.needs, vec![RECORD.to_owned()]);
    let later = Utc::now() + Duration::days(30);
    redate(&root, &first.id, later);
    let import = trash_import(&root);
    assert!(import.needs.is_empty());
    assert_eq!(import.erase_at().unwrap(), first_erase_at(&root, &first.id));
    assert!(import.erase_at().unwrap() > Utc::now() + Duration::days(29));
}

fn first_erase_at(root: &Path, id: &str) -> DateTime<Utc> {
    read_item(root, id).unwrap().unwrap().erase_at().unwrap()
}

#[test]
fn an_import_waiting_longer_keeps_its_own_date() {
    let (_dir, root) = journal();
    let first = trash_first(&root);
    redate(&root, &first.id, Utc::now() + Duration::hours(1));
    let import = trash_import(&root);
    assert!(import.erase_at().unwrap() > Utc::now() + Duration::days(6));
}

#[test]
fn erasing_the_segment_first_leaves_the_import_and_its_upload() {
    let (_dir, root) = journal();
    let first = trash_first(&root);
    let import = trash_import(&root);
    erase(&root, &first.id, Action::Erase, web()).unwrap();
    let held = held_file(&root, &import.id, &format!("{RECORD}/talk.m4a")).unwrap();
    assert_eq!(fs::read(held).unwrap(), b"upload");
    assert_eq!(list(&root).unwrap().len(), 1);
}

#[test]
fn erasing_the_import_erases_the_segments_that_need_it() {
    let (_dir, root) = journal();
    let first = trash_first(&root);
    let import = trash_import(&root);
    assert_eq!(needing(&root, &import).unwrap().len(), 1);
    erase(&root, &import.id, Action::Erase, web()).unwrap();
    assert!(list(&root).unwrap().is_empty());
    assert!(read_item(&root, &first.id).unwrap().is_none());
    let erased = solstone_core_removals::history(&root, 100)
        .unwrap()
        .entries
        .into_iter()
        .filter(|entry| entry.event == Event::Erased)
        .count();
    assert_eq!(erased, 2);
}

#[test]
fn the_timer_erases_the_segment_before_the_import() {
    let (_dir, root) = journal();
    let first = trash_first(&root);
    let import = trash_import(&root);
    // Dated apart, as two deletes a day apart are.
    let segment_due = first.erase_at().unwrap();
    redate(&root, &import.id, segment_due + Duration::days(1));
    let swept = sweep(&root, &[&Needing], segment_due + Duration::hours(1));
    assert_eq!(swept.failed, Vec::<String>::new());
    assert!(read_item(&root, &first.id).unwrap().is_none());
    assert!(read_item(&root, &import.id).unwrap().is_some());
}

#[test]
fn a_recovered_import_leaves_the_segment_waiting() {
    let (_dir, root) = journal();
    let first = trash_first(&root);
    let import = trash_import(&root);
    attach(&root, &[&Needing], &import.id, Action::Recover, web()).unwrap();
    assert!(root.join(RECORD).join("talk.m4a").is_file());
    assert!(read_item(&root, &first.id).unwrap().is_some());
    assert!(needing(&root, &first).unwrap().is_empty());
}

#[test]
fn erasing_beside_the_record_takes_nothing_that_needs_it() {
    let (_dir, root) = journal();
    let first = trash_first(&root);
    let lone = detach(
        &root,
        &Needing,
        import(),
        Action::SegmentDelete,
        web(),
        vec![LAST.into()],
    )
    .unwrap();
    erase(&root, &lone.id, Action::Erase, web()).unwrap();
    assert!(read_item(&root, &first.id).unwrap().is_some());
}

#[test]
fn the_import_falling_due_first_erases_its_segment_and_the_sweep_reports_nothing_failed() {
    let (_dir, root) = journal();
    let first = trash_first(&root);
    let import = trash_import(&root);
    // Both due, the import first: its erase takes the segment, which the sweep
    // then meets already gone.
    let due = first.erase_at().unwrap();
    redate(&root, &import.id, due - Duration::hours(1));
    let swept = sweep(&root, &[&Needing], due);
    assert_eq!(swept.failed, Vec::<String>::new());
    assert_eq!(
        swept.erased, 1,
        "the import by its timer; the segment with it"
    );
    assert!(list(&root).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn an_erase_now_stopped_part_way_leaves_what_needs_it_erasing_for_the_sweep() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, root) = journal();
    let first = trash_first(&root);
    let import = trash_import(&root);
    // The import's files can't be unlinked: the real erase stops part way.
    let held = root
        .join(TRASH_DIR)
        .join(&import.id)
        .join("files")
        .join(RECORD);
    fs::set_permissions(&held, fs::Permissions::from_mode(0o500)).unwrap();
    assert!(erase(&root, &import.id, Action::Erase, web()).is_err());
    let waiting = read_item(&root, &first.id).unwrap().unwrap();
    assert_eq!(
        waiting.state,
        State::Erasing,
        "marked with the import, not left waiting"
    );
    assert_eq!(waiting.pending.unwrap().action, Action::Erase);
    fs::set_permissions(&held, fs::Permissions::from_mode(0o700)).unwrap();
    let swept = sweep(&root, &[&Needing], Utc::now());
    assert_eq!(swept.failed, Vec::<String>::new());
    assert!(read_item(&root, &first.id).unwrap().is_none());
    assert!(read_item(&root, &import.id).unwrap().is_none());
}

#[test]
fn erasing_the_import_erases_what_was_released_from_its_waiting_segment() {
    let (_dir, root) = journal();
    let released = format!("{FIRST}/original.flac");
    fs::write(root.join(&released), b"original").unwrap();
    // Originals released from the segment while it stayed: a linked item.
    let mut originals = detach(
        &root,
        &Needing,
        Unit::Speakers,
        Action::RetentionTool,
        web(),
        vec![released],
    )
    .unwrap();
    originals.linked = true;
    publish(&root, &originals).unwrap();
    trash_first(&root);
    let import = trash_import(&root);
    erase(&root, &import.id, Action::Erase, web()).unwrap();
    assert!(read_item(&root, &originals.id).unwrap().is_none());
    assert!(list(&root).unwrap().is_empty());
}

/// A hold that passes its first check and refuses every one after.
struct Once(std::cell::Cell<u32>);
impl Hold for &Once {
    fn check(&self) -> Result<(), String> {
        let seen = self.0.get();
        self.0.set(seen + 1);
        if seen == 0 {
            Ok(())
        } else {
            Err("moved into the trash just now".into())
        }
    }
}
struct OnceKind(Once);
impl Kind for OnceKind {
    fn handles(&self, _: &Unit) -> bool {
        true
    }
    fn hold<'a>(&'a self, _: &Path, _: &Item) -> Result<Box<dyn Hold + 'a>, String> {
        Ok(Box::new(&self.0))
    }
    fn present(&self, journal: &Path, path: &str) -> bool {
        journal.join(path).exists()
    }
    fn references(&self, _: &Path, _: &Item, _: Change, _: &[String]) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn a_recover_asks_its_hold_again_under_the_lock_and_a_refusal_leaves_it_waiting() {
    let (_dir, root) = journal();
    let first = trash_first(&root);
    let kind = OnceKind(Once(std::cell::Cell::new(0)));
    let refused = attach(&root, &[&kind], &first.id, Action::Recover, web());
    assert!(matches!(refused, Err(TrashError::Held(_))), "{refused:?}");
    let waiting = read_item(&root, &first.id).unwrap().unwrap();
    assert_eq!(waiting.state, State::Trashed);
    assert!(waiting.pending.is_none());
}
