// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::Path;

use solstone_core_removals::{Action, Event, Surface, Unit};
use solstone_core_retention::notify::{IndexNotify, NotifyError, PruneCounts};
use solstone_core_retention::receipt::RemovedPath;
use solstone_core_trash::{Kind, State, TrashError};
use tempfile::TempDir;

use super::*;
use crate::{RetiredFacet, create_facet, record_retired_facet};

/// Search, as the kinds tell it, failing on request.
#[derive(Default)]
struct Search {
    removed: RefCell<Vec<String>>,
    returned: RefCell<Vec<String>>,
    fail: Cell<bool>,
}

impl IndexNotify for Search {
    fn paths_removed(&self, removed: &[RemovedPath]) -> Result<PruneCounts, NotifyError> {
        if self.fail.get() {
            return Err(NotifyError {
                reason: "down".into(),
            });
        }
        self.removed
            .borrow_mut()
            .extend(removed.iter().map(|path| path.as_str().to_owned()));
        Ok(PruneCounts::default())
    }

    fn paths_returned(&self, returned: &[String]) -> Result<(), NotifyError> {
        if self.fail.get() {
            return Err(NotifyError {
                reason: "down".into(),
            });
        }
        self.returned.borrow_mut().extend(returned.iter().cloned());
        Ok(())
    }
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn journal() -> TempDir {
    let root = TempDir::new().unwrap();
    write(root.path(), "config/journal.json", b"{}");
    create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
    create_facet(root.path(), "home", "Home", "", "", "", None).unwrap();
    write(root.path(), "facets/work/news/20260801.md", b"work news");
    root
}

fn id_of(root: &Path, name: &str) -> String {
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(format!("facets/{name}/facet.json"))).unwrap())
            .unwrap();
    value["id"].as_str().unwrap().to_owned()
}

/// What the history recorded, in a fixed order: entries written in the same
/// moment come back in an order the history doesn't promise.
fn events(root: &Path) -> Vec<(Event, Unit)> {
    sorted(
        solstone_core_removals::history(root, 100)
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| (entry.event, entry.unit))
            .collect(),
    )
}

fn sorted(mut events: Vec<(Event, Unit)>) -> Vec<(Event, Unit)> {
    events.sort_by_key(|event| format!("{event:?}"));
    events
}

fn recover(
    root: &Path,
    kind: &dyn Kind,
    id: &str,
) -> Result<solstone_core_trash::Item, TrashError> {
    solstone_core_trash::attach(root, &[kind], id, Action::Recover, Surface::Cli)
}

fn sweep(root: &Path, kind: &dyn Kind) -> solstone_core_trash::Swept {
    solstone_core_trash::sweep(root, &[kind], chrono::Utc::now())
}

fn state(root: &Path, id: &str) -> Option<State> {
    solstone_core_trash::item(root, id)
        .unwrap()
        .map(|item| item.state)
}

#[test]
fn a_facet_moves_whole_comes_back_exactly_and_is_erased() {
    let journal = journal();
    let root = journal.path();
    let search = Search::default();
    let facets = Facets::new(&search);
    let id = id_of(root, "work");

    let item = facets
        .trash(root, "work", Action::FacetDelete, Surface::Cli)
        .unwrap();
    assert_eq!(item.label.as_deref(), Some("Work"));
    assert!(!root.join("facets/work").exists());
    assert!(!root.join("facets/retired.json").exists());
    assert!(
        search
            .removed
            .borrow()
            .contains(&"facets/work/news/20260801.md".to_owned())
    );

    recover(root, &facets, &item.id).unwrap();
    assert_eq!(id_of(root, "work"), id);
    assert_eq!(
        fs::read(root.join("facets/work/news/20260801.md")).unwrap(),
        b"work news"
    );
    assert!(
        search
            .returned
            .borrow()
            .contains(&"facets/work/news/20260801.md".to_owned())
    );
    // What names the facet comes back with it: search rescans and reclassifies.
    for token in [
        solstone_core_indexer_store::todo::Token::ScanFull,
        solstone_core_indexer_store::todo::Token::Reclassify,
    ] {
        assert!(
            solstone_core_indexer_store::todo::listed_attempts(root, token)
                .unwrap()
                .is_some()
        );
    }

    let again = facets
        .trash(root, "work", Action::FacetDelete, Surface::Cli)
        .unwrap();
    solstone_core_trash::erase(root, &again.id, Action::Erase, Surface::Cli).unwrap();
    assert!(!root.join("facets/work").exists());
    assert!(solstone_core_trash::list(root).unwrap().is_empty());
    assert_eq!(
        events(root),
        sorted(vec![
            (Event::Trashed, Unit::Facet),
            (Event::Recovered, Unit::Facet),
            (Event::Trashed, Unit::Facet),
            (Event::Erased, Unit::Facet),
        ])
    );
}

#[test]
fn a_facet_move_that_stopped_either_way_is_finished_by_the_sweep() {
    let journal = journal();
    let root = journal.path();
    let search = Search::default();
    let facets = Facets::new(&search);

    search.fail.set(true);
    let Err(TrashError::Unfinished(id)) =
        facets.trash(root, "work", Action::FacetDelete, Surface::Cli)
    else {
        panic!("the detach should stop after the move");
    };
    assert_eq!(state(root, &id), Some(State::Detaching));
    search.fail.set(false);
    assert_eq!(sweep(root, &facets).resumed, 1);
    assert_eq!(state(root, &id), Some(State::Trashed));

    search.fail.set(true);
    assert!(recover(root, &facets, &id).is_err());
    assert_eq!(state(root, &id), Some(State::Attaching));
    search.fail.set(false);
    assert_eq!(sweep(root, &facets).resumed, 1);
    assert_eq!(state(root, &id), None);
    assert_eq!(
        fs::read(root.join("facets/work/news/20260801.md")).unwrap(),
        b"work news"
    );

    // Erased after a stop, too.
    search.fail.set(true);
    let Err(TrashError::Unfinished(id)) =
        facets.trash(root, "work", Action::FacetDelete, Surface::Cli)
    else {
        panic!("the detach should stop after the move");
    };
    search.fail.set(false);
    sweep(root, &facets);
    solstone_core_trash::erase(root, &id, Action::Erase, Surface::Cli).unwrap();
    assert!(!root.join("facets/work").exists());
    assert_eq!(state(root, &id), None);
}

#[test]
fn recovering_over_a_new_facet_of_the_same_name_replaces_it_and_says_so() {
    let journal = journal();
    let root = journal.path();
    let search = Search::default();
    let facets = Facets::new(&search);
    let old = id_of(root, "work");
    let item = facets
        .trash(root, "work", Action::FacetDelete, Surface::Cli)
        .unwrap();

    // The name is free: no `-2`, no retired entry.
    create_facet(root, "work", "Work again", "", "", "", None).unwrap();
    write(root, "facets/work/news/20260802.md", b"new news");
    assert_ne!(id_of(root, "work"), old);

    recover(root, &facets, &item.id).unwrap();
    assert_eq!(id_of(root, "work"), old);
    assert!(!root.join("facets/work/news/20260802.md").exists());
    assert_eq!(
        events(root),
        sorted(vec![
            (Event::Trashed, Unit::Facet),
            (Event::Erased, Unit::Facet),
            (Event::Recovered, Unit::Facet),
        ])
    );
}

#[test]
fn the_last_enabled_facet_stays() {
    let root = TempDir::new().unwrap();
    create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
    let error = Facets::new(&Search::default())
        .trash(root.path(), "work", Action::FacetDelete, Surface::Cli)
        .unwrap_err();
    assert!(matches!(&error, TrashError::Held(reason) if reason == LAST_FACET));
    assert_eq!(failure_key(&error), "last_facet");
    assert!(root.path().join("facets/work/facet.json").is_file());
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
}

#[test]
fn a_name_that_leads_to_another_facet_now_refuses_the_recover() {
    let journal = journal();
    let root = journal.path();
    let search = Search::default();
    let facets = Facets::new(&search);
    let item = facets
        .trash(root, "work", Action::FacetDelete, Surface::Cli)
        .unwrap();
    // A new `work` is made and then merged into `home`.
    create_facet(root, "work", "Work again", "", "", "", None).unwrap();
    let newer = id_of(root, "work");
    fs::remove_dir_all(root.join("facets/work")).unwrap();
    record_retired_facet(
        root,
        "work",
        RetiredFacet::merged(Some(newer), id_of(root, "home"), None),
    )
    .unwrap();

    let error = recover(root, &facets, &item.id).unwrap_err();
    assert!(matches!(&error, TrashError::Held(reason) if reason == NAME_REDIRECTED));
    assert_eq!(failure_key(&error), "name_redirected");
    assert_eq!(state(root, &item.id), Some(State::Trashed));
    assert!(!root.join("facets/work").exists());
}

#[test]
fn a_deleted_entry_from_an_earlier_build_reserves_nothing() {
    let root = TempDir::new().unwrap();
    let retired = serde_json::json!({"names": {
        "personal": {"state": "deleted", "id": null},
        "body": {"state": "deleted", "id": null},
        "work": {"state": "deleted", "id": null},
        "old": {"state": "merged", "id": null, "successor": "0190a3c5-6f9b-4c4e-9d5c-0123456789ab"},
    }});
    write(
        root.path(),
        "facets/retired.json",
        retired.to_string().as_bytes(),
    );
    assert!(crate::ensure_default_facet(root.path()).unwrap());
    assert!(root.path().join("facets/personal/facet.json").is_file());
    create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
    assert!(crate::facet_name_is_free(root.path(), "body").unwrap());
    assert_eq!(
        crate::first_free_facet_name(root.path(), "body").unwrap(),
        "body"
    );
    // A merged name still leads to its facet and is never given out.
    assert!(matches!(
        create_facet(root.path(), "old", "Old", "", "", "", None),
        Err(crate::FacetWriteError::NameRetired { .. })
    ));
    assert_eq!(
        crate::first_free_facet_name(root.path(), "old").unwrap(),
        "old-2"
    );
    // A leftover deleted entry gives way to a real redirect.
    let home = {
        create_facet(root.path(), "home", "Home", "", "", "", None).unwrap();
        id_of(root.path(), "home")
    };
    record_retired_facet(
        root.path(),
        "body",
        RetiredFacet::merged(None, home.clone(), None),
    )
    .unwrap();
    assert!(
        crate::retired_facet_entry(root.path(), "body")
            .unwrap()
            .is_some_and(|entry| entry.redirects())
    );
}

// ---- what a merge dropped ----

const DROPPED: &str = ".facet-merge-tx1.dropped";

/// `home` kept its own `a.txt` and `notes/b.txt` in a merge of `work`, whose
/// copies wait in the merge's staging folder.
fn staged(root: &Path) {
    write(root, "facets/home/a.txt", b"home a");
    write(root, "facets/home/notes/b.txt", b"home b");
    write(root, "facets/home/keep.txt", b"home keep");
    let manifest = DropManifest {
        dest_id: id_of(root, "home"),
        dest_name: "home".into(),
        files: vec!["a.txt".into(), "notes/b.txt".into()],
    };
    write(
        root,
        &format!("facets/{DROPPED}/{MANIFEST}"),
        &serde_json::to_vec(&manifest).unwrap(),
    );
    write(root, &format!("facets/{DROPPED}/files/a.txt"), b"work a");
    write(
        root,
        &format!("facets/{DROPPED}/files/notes/b.txt"),
        b"work b",
    );
}

fn drop_item(root: &Path, drops: &FacetDrops<'_>) -> solstone_core_trash::Item {
    drops
        .trash(root, DROPPED, Action::FacetMerge, Surface::Cli)
        .unwrap()
}

#[test]
fn a_merges_dropped_files_come_back_over_the_facet_they_were_merged_into() {
    let journal = journal();
    let root = journal.path();
    staged(root);
    let search = Search::default();
    let drops = FacetDrops::new(&search);
    let item = drop_item(root, &drops);
    assert!(!root.join(format!("facets/{DROPPED}")).exists());
    assert_eq!(fs::read(root.join("facets/home/a.txt")).unwrap(), b"home a");

    recover(root, &drops, &item.id).unwrap();
    assert_eq!(fs::read(root.join("facets/home/a.txt")).unwrap(), b"work a");
    assert_eq!(
        fs::read(root.join("facets/home/notes/b.txt")).unwrap(),
        b"work b"
    );
    // What the drop doesn't touch stays exactly as it was.
    assert_eq!(
        fs::read(root.join("facets/home/keep.txt")).unwrap(),
        b"home keep"
    );
    assert!(!root.join(format!("facets/{DROPPED}")).exists());
    // Search reads the facet's files again.
    let returned = search.returned.borrow();
    assert!(returned.contains(&"facets/home/a.txt".to_owned()));
    assert!(returned.contains(&"facets/home/notes/b.txt".to_owned()));
    // The facet's own copies went with the item, and the history says so.
    assert_eq!(
        events(root),
        sorted(vec![
            (Event::Trashed, Unit::FacetFile),
            (Event::Erased, Unit::FacetFile),
            (Event::Recovered, Unit::FacetFile),
        ])
    );
}

#[test]
fn a_put_back_that_stopped_part_way_finishes_with_a_copy_deleted_since() {
    let journal = journal();
    let root = journal.path();
    staged(root);
    let search = Search::default();
    let drops = FacetDrops::new(&search);
    let item = drop_item(root, &drops);

    // `notes` is a file for now, so the second file can't go in: the recover
    // stops after the first.
    fs::remove_dir_all(root.join("facets/home/notes")).unwrap();
    write(root, "facets/home/notes", b"in the way");
    assert!(recover(root, &drops, &item.id).is_err());
    assert_eq!(state(root, &item.id), Some(State::Attaching));
    assert_eq!(fs::read(root.join("facets/home/a.txt")).unwrap(), b"work a");

    // The owner removes it: the facet's own `notes/b.txt` is gone since.
    fs::remove_file(root.join("facets/home/notes")).unwrap();
    assert_eq!(sweep(root, &drops).resumed, 1);
    assert_eq!(state(root, &item.id), None);
    assert_eq!(fs::read(root.join("facets/home/a.txt")).unwrap(), b"work a");
    assert_eq!(
        fs::read(root.join("facets/home/notes/b.txt")).unwrap(),
        b"work b"
    );
    assert!(!root.join(format!("facets/{DROPPED}")).exists());
}

#[test]
fn every_file_is_told_to_search_before_the_staging_folder_goes() {
    let journal = journal();
    let root = journal.path();
    staged(root);
    let search = Search::default();
    let drops = FacetDrops::new(&search);
    let item = drop_item(root, &drops);

    search.fail.set(true);
    assert!(recover(root, &drops, &item.id).is_err());
    // Every file is in place, and the folder waits with its manifest.
    assert_eq!(fs::read(root.join("facets/home/a.txt")).unwrap(), b"work a");
    assert!(root.join(format!("facets/{DROPPED}/{MANIFEST}")).is_file());
    assert!(search.returned.borrow().is_empty());

    search.fail.set(false);
    assert_eq!(sweep(root, &drops).resumed, 1);
    let returned = search.returned.borrow();
    assert!(returned.contains(&"facets/home/a.txt".to_owned()));
    assert!(returned.contains(&"facets/home/notes/b.txt".to_owned()));
    assert!(!root.join(format!("facets/{DROPPED}")).exists());
}

#[test]
fn a_resume_after_the_staging_folder_went_finishes() {
    let journal = journal();
    let root = journal.path();
    staged(root);
    let search = Search::default();
    let drops = FacetDrops::new(&search);
    let item = drop_item(root, &drops);
    search.fail.set(true);
    assert!(recover(root, &drops, &item.id).is_err());
    search.fail.set(false);
    // The put-back finishes and its folder goes, then the step stops.
    let held = solstone_core_trash::item(root, &item.id).unwrap().unwrap();
    drops
        .references(root, &held, solstone_core_trash::Change::Attached, &[])
        .unwrap();
    assert!(!root.join(format!("facets/{DROPPED}")).exists());

    assert_eq!(sweep(root, &drops).resumed, 1);
    assert_eq!(state(root, &item.id), None);
    assert_eq!(fs::read(root.join("facets/home/a.txt")).unwrap(), b"work a");
}

#[test]
fn the_facet_they_were_merged_into_erased_refuses_and_records_nothing() {
    let journal = journal();
    let root = journal.path();
    staged(root);
    let search = Search::default();
    let drops = FacetDrops::new(&search);
    let item = drop_item(root, &drops);
    let home = Facets::new(&search)
        .trash(root, "home", Action::FacetDelete, Surface::Cli)
        .unwrap();
    solstone_core_trash::erase(root, &home.id, Action::Erase, Surface::Cli).unwrap();
    let before = events(root).len();

    let error = recover(root, &drops, &item.id).unwrap_err();
    assert!(matches!(&error, TrashError::Held(reason) if reason == MERGE_DEST_MISSING));
    assert_eq!(failure_key(&error), "merge_dest_missing");
    assert_eq!(state(root, &item.id), Some(State::Trashed));
    assert_eq!(events(root).len(), before);
}

#[test]
fn the_facet_they_were_merged_into_in_the_trash_is_named_to_recover_first() {
    let journal = journal();
    let root = journal.path();
    staged(root);
    let search = Search::default();
    let drops = FacetDrops::new(&search);
    let item = drop_item(root, &drops);
    let facets = Facets::new(&search);
    let home = facets
        .trash(root, "home", Action::FacetDelete, Surface::Cli)
        .unwrap();

    let error = recover(root, &drops, &item.id).unwrap_err();
    assert!(matches!(&error, TrashError::Held(reason) if *reason == waits_dest("Home")));
    assert_eq!(failure_key(&error), "waits_dest");
    let TrashError::Held(reason) = &error else {
        unreachable!()
    };
    let refusal: serde_json::Value = serde_json::from_str(reason).unwrap();
    assert_eq!(refusal["title"], "Home");
    assert_eq!(state(root, &item.id), Some(State::Trashed));

    recover(root, &facets, &home.id).unwrap();
    recover(root, &drops, &item.id).unwrap();
    assert_eq!(fs::read(root.join("facets/home/a.txt")).unwrap(), b"work a");
}

#[test]
fn a_leftover_with_no_manifest_comes_back_where_it_was_and_touches_nothing() {
    let journal = journal();
    let root = journal.path();
    write(root, "facets/home/a.txt", b"home a");
    write(root, "facets/.facet-merge-tx2.source/a.txt", b"work a");
    write(root, "facets/.facet-merge-tx2.source/facet.json", b"{}");
    let search = Search::default();
    let drops = FacetDrops::new(&search);
    let item = drops
        .trash(
            root,
            ".facet-merge-tx2.source",
            Action::FacetMerge,
            Surface::Cli,
        )
        .unwrap();
    assert!(!root.join("facets/.facet-merge-tx2.source").exists());

    recover(root, &drops, &item.id).unwrap();
    assert_eq!(
        fs::read(root.join("facets/.facet-merge-tx2.source/a.txt")).unwrap(),
        b"work a"
    );
    assert_eq!(fs::read(root.join("facets/home/a.txt")).unwrap(), b"home a");
}
