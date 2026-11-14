// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]

use super::*;
use serde_json::{Value, json};
use solstone_core_removals::Page;
use solstone_core_retention::NoIndex;
use std::fs;
use tempfile::TempDir;

const ID: &str = "20260801_090000";
const OTHER: &str = "20260801_100000";
const FIRST: &str = "chronicle/20260801/import.audio/090000_300";
const LAST: &str = "chronicle/20260801/import.audio/090500_300";
const UPLOAD: &str = "imports/20260801_090000/talk.m4a";

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// An import of `source` that kept its upload and lists `segments`.
fn import(root: &Path, id: &str, source: &str, segments: &[&str]) {
    let listed: Vec<Value> = segments
        .iter()
        .map(|path| {
            let parts: Vec<&str> = path.split('/').collect();
            json!({"day": parts[1], "stream": parts[2], "segment": parts[3],
                "outcome": {"status": "bound", "prev_day": null, "prev_segment": null, "seq": 1}})
        })
        .collect();
    write(
        root,
        &format!("imports/{id}/imported.json"),
        json!({"schema": "solstone.import.publication.v2", "status": "success",
            "source_type": source, "segments": listed,
            "indexing": {"queued": [], "declined": [], "errored": []}, "day_markers": []})
        .to_string()
        .as_bytes(),
    );
    write(
        root,
        &format!("imports/{id}/import.json"),
        json!({"import_id": id, "original_filename": "talk.m4a",
            "file_path": format!("imports/{id}/talk.m4a")})
        .to_string()
        .as_bytes(),
    );
    write(root, &format!("imports/{id}/talk.m4a"), b"the upload");
    for path in segments {
        write(root, &format!("{path}/audio.flac"), b"sound");
    }
}

fn journal(segments: &[&str]) -> TempDir {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "config/journal.json",
        br#"{"setup":{"completed_at":1700000000000}}"#,
    );
    import(root.path(), ID, "audio", segments);
    root
}

fn surface() -> Surface {
    Surface::Web {
        page: Page::Transcripts,
    }
}

fn unit(path: &str) -> Unit {
    let parts: Vec<&str> = path.split('/').collect();
    Unit::Segment {
        day: parts[1].into(),
        stream: parts[2].into(),
        segment: parts[3].into(),
    }
}

fn delete(root: &Path, path: &str) -> (Item, Option<String>) {
    trash_segment(
        root,
        &NoIndex,
        &solstone_core_retention::made_from::NothingMade,
        unit(path),
        path.to_owned(),
        Action::SegmentDelete,
        surface(),
    )
    .unwrap()
}

fn recover(root: &Path, id: &str) -> Result<Item, TrashError> {
    crate::trash_kinds::with_kinds(root, |kinds| {
        solstone_core_trash::attach(root, kinds, id, Action::Recover, surface())
    })
}

fn erase(root: &Path, id: &str) {
    solstone_core_trash::erase(root, id, Action::Erase, surface()).unwrap();
}

fn code(error: &TrashError) -> String {
    let TrashError::Held(text) = error else {
        panic!("not a refusal: {error}");
    };
    serde_json::from_str::<Value>(text).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn deleting_an_imports_only_segment_takes_the_import_whole() {
    let root = journal(&[FIRST]);
    let found = preview(root.path(), &[FIRST.to_owned()]).remove(0);
    assert!(found.takes);
    assert_eq!(found.originals, 1);
    assert_eq!(found.kept, 1);
    assert_eq!(found.label.as_deref(), Some("talk.m4a"));
    let (item, took) = delete(root.path(), FIRST);
    assert_eq!(took.as_deref(), Some(ID));
    assert!(matches!(&item.unit, Unit::Import { id, .. } if id == ID));
    assert_eq!(item.action, Action::SegmentDelete);
    assert!(!root.path().join(UPLOAD).exists());
    assert!(!root.path().join(FIRST).join("audio.flac").exists());
    recover(root.path(), &item.id).unwrap();
    assert_eq!(fs::read(root.path().join(UPLOAD)).unwrap(), b"the upload");
    assert!(root.path().join(FIRST).join("audio.flac").is_file());
    let (item, _) = delete(root.path(), FIRST);
    erase(root.path(), &item.id);
    assert!(!root.path().join(UPLOAD).exists());
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
}

#[test]
fn a_segment_that_isnt_the_last_leaves_the_upload_and_names_it() {
    let root = journal(&[FIRST, LAST]);
    assert!(!preview(root.path(), &[FIRST.to_owned()])[0].takes);
    let (item, took) = delete(root.path(), FIRST);
    assert_eq!(took, None);
    assert!(matches!(item.unit, Unit::Segment { .. }));
    assert_eq!(item.needs, vec![format!("imports/{ID}")]);
    assert!(root.path().join(UPLOAD).is_file());
    let last = preview(root.path(), &[LAST.to_owned()]).remove(0);
    assert!(last.takes);
    assert_eq!(last.waiting, 1);
}

#[test]
fn the_last_segment_takes_the_upload_after_the_first_was_erased() {
    let root = journal(&[FIRST, LAST]);
    let (first, _) = delete(root.path(), FIRST);
    erase(root.path(), &first.id);
    assert!(root.path().join(UPLOAD).is_file());
    let (item, took) = delete(root.path(), LAST);
    assert_eq!(took.as_deref(), Some(ID));
    assert!(!root.path().join(UPLOAD).exists());
    erase(root.path(), &item.id);
    assert!(!root.path().join(format!("imports/{ID}")).exists());
}

#[test]
fn the_first_segments_erase_leaves_the_upload_and_the_second_finds_it() {
    let root = journal(&[FIRST, LAST]);
    let (first, _) = delete(root.path(), FIRST);
    let (import, _) = delete(root.path(), LAST);
    assert!(import.erase_at().unwrap() >= first.erase_at().unwrap());
    erase(root.path(), &first.id);
    recover(root.path(), &import.id).unwrap();
    assert_eq!(fs::read(root.path().join(UPLOAD)).unwrap(), b"the upload");
    assert!(root.path().join(LAST).join("audio.flac").is_file());
}

#[test]
fn an_earlier_segment_waits_for_its_import_to_come_back_first() {
    let root = journal(&[FIRST, LAST]);
    let (first, _) = delete(root.path(), FIRST);
    let (import, _) = delete(root.path(), LAST);
    let refused = recover(root.path(), &first.id).unwrap_err();
    assert_eq!(code(&refused), IMPORT_IN_TRASH);
    assert!(!root.path().join(FIRST).join("audio.flac").exists());
    recover(root.path(), &import.id).unwrap();
    recover(root.path(), &first.id).unwrap();
    assert!(root.path().join(FIRST).join("audio.flac").is_file());
    assert!(root.path().join(UPLOAD).is_file());
}

#[test]
fn erasing_the_import_erases_its_earlier_segment_too() {
    let root = journal(&[FIRST, LAST]);
    let (first, _) = delete(root.path(), FIRST);
    let (import, _) = delete(root.path(), LAST);
    erase(root.path(), &import.id);
    assert!(
        solstone_core_trash::item(root.path(), &first.id)
            .unwrap()
            .is_none()
    );
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
}

#[test]
fn a_segment_whose_import_was_erased_without_it_refuses_to_come_back() {
    let root = journal(&[FIRST, LAST]);
    let (first, _) = delete(root.path(), FIRST);
    // A stop between the import's erase and its segments': the record is gone.
    fs::remove_dir_all(root.path().join(format!("imports/{ID}"))).unwrap();
    let refused = recover(root.path(), &first.id).unwrap_err();
    assert_eq!(code(&refused), IMPORT_GONE);
}

#[test]
fn choosing_every_remaining_segment_takes_the_import_as_one() {
    let root = journal(&[FIRST, LAST]);
    let chosen = |path: &str| Chosen {
        unit: unit(path),
        paths: vec![path.to_owned()],
        label: None,
    };
    let choices = choose(root.path(), vec![chosen(FIRST), chosen(LAST)]);
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].covers, vec![0, 1]);
    assert_eq!(choices[0].import.as_deref(), Some(ID));
    let choices = choose(root.path(), vec![chosen(FIRST)]);
    assert_eq!(choices[0].import, None);
}

#[test]
fn a_segment_two_imports_list_never_takes_either() {
    let root = journal(&[FIRST]);
    import(root.path(), OTHER, "audio", &[FIRST]);
    assert!(preview(root.path(), &[FIRST.to_owned()]).is_empty());
    let (item, took) = delete(root.path(), FIRST);
    assert_eq!(took, None);
    assert!(item.needs.is_empty());
    assert!(root.path().join(UPLOAD).is_file());
}

#[test]
fn an_archive_merges_segments_delete_as_segments() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "config/journal.json",
        br#"{"setup":{"completed_at":1700000000000}}"#,
    );
    import(root.path(), ID, "journal_archive", &[FIRST]);
    assert!(preview(root.path(), &[FIRST.to_owned()]).is_empty());
    let (item, took) = delete(root.path(), FIRST);
    assert_eq!(took, None);
    assert!(item.needs.is_empty());
}

#[test]
fn a_segment_no_import_made_is_unchanged() {
    let root = journal(&[FIRST]);
    let lone = "chronicle/20260801/phone/120000_300";
    write(root.path(), &format!("{lone}/audio.flac"), b"sound");
    let (item, took) = delete(root.path(), lone);
    assert_eq!(took, None);
    assert!(item.needs.is_empty());
    recover(root.path(), &item.id).unwrap();
}

#[test]
fn a_recover_a_stop_left_part_way_is_finished_even_with_its_import_in_the_trash() {
    let root = journal(&[FIRST, LAST]);
    let (first, _) = delete(root.path(), FIRST);
    delete(root.path(), LAST);
    // A recover of the first stopped after it was marked, before anything moved.
    let mut stopped = solstone_core_trash::item(root.path(), &first.id)
        .unwrap()
        .unwrap();
    stopped.state = solstone_core_trash::State::Attaching;
    stopped.pending = Some(solstone_core_trash::Pending {
        action: Action::Recover,
        surface: surface(),
        at: chrono::Utc::now().to_rfc3339(),
        operation: None,
    });
    let record = root
        .path()
        .join(solstone_core_trash::TRASH_DIR)
        .join(&first.id)
        .join("item.json");
    fs::write(&record, serde_json::to_vec(&stopped).unwrap()).unwrap();
    let swept = crate::trash_kinds::sweep(root.path(), chrono::Utc::now());
    assert_eq!(swept.failed, Vec::<String>::new());
    assert!(
        solstone_core_trash::item(root.path(), &first.id)
            .unwrap()
            .is_none()
    );
    assert!(root.path().join(FIRST).join("audio.flac").is_file());
}
