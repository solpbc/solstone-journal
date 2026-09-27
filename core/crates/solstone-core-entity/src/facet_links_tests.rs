// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};

use crate::facet_links::{
    FolderState, LinkDirs, LinkFieldPolicy, LinkFolderError, fold_observation_rows,
    merge_link_fields,
};
use crate::{ObservationParseSource, ObservationRow, parse_observation_file};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Journal(PathBuf);

impl Journal {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-links-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("facets/work/entities")).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn dirs(&self) -> LinkDirs {
        LinkDirs::for_facet(&self.0, "work")
    }

    fn folder(&self, dir: &str) -> PathBuf {
        self.0.join("facets/work/entities").join(dir)
    }

    fn link(&self, dir: &str, link: Value) {
        fs::create_dir_all(self.folder(dir)).unwrap();
        fs::write(
            self.folder(dir).join("entity.json"),
            serde_json::to_string_pretty(&link).unwrap(),
        )
        .unwrap();
    }

    fn notes(&self, dir: &str, rows: &[Value]) {
        fs::create_dir_all(self.folder(dir)).unwrap();
        let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
        fs::write(self.folder(dir).join("observations.jsonl"), text).unwrap();
    }

    fn read_notes(&self, dir: &str) -> Vec<ObservationRow> {
        let path = self.folder(dir).join("observations.jsonl");
        let text = fs::read_to_string(&path).unwrap();
        parse_observation_file(&text, ObservationParseSource::Path(&path))
            .unwrap()
            .full_rows
    }

    fn read_link(&self, dir: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(self.folder(dir).join("entity.json")).unwrap())
            .unwrap()
    }

    fn folders(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.0.join("facets/work/entities"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn tree(&self) -> Vec<(String, Vec<u8>)> {
        let mut files = Vec::new();
        collect(&self.0.join("facets"), &self.0, &mut files);
        files.sort();
        files
    }
}

fn collect(dir: &Path, root: &Path, files: &mut Vec<(String, Vec<u8>)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, root, files);
        } else {
            files.push((
                path.strip_prefix(root).unwrap().display().to_string(),
                fs::read(&path).unwrap(),
            ));
        }
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn no_hook() -> impl FnMut(&str) -> Result<(), LinkFolderError> {
    |_| Ok(())
}

fn row(id: u64, content: &str, observed_at: i64) -> Value {
    json!({"id": id, "content": content, "observed_at": observed_at})
}

fn rows(values: &[Value]) -> Vec<ObservationRow> {
    let text: String = values.iter().map(|row| format!("{row}\n")).collect();
    parse_observation_file(&text, ObservationParseSource::CapturedSnapshot)
        .unwrap()
        .full_rows
}

fn contents(rows: &[ObservationRow]) -> Vec<(u64, String, bool)> {
    rows.iter()
        .map(|row| (row.id, row.content.clone(), row.retired.is_some()))
        .collect()
}

#[test]
fn a_fold_keeps_every_note_and_drops_only_exact_copies() {
    let into = rows(&[row(1, "met at the lab", 10), row(2, "prefers tea", 20)]);
    let from = rows(&[
        row(1, "met at the lab", 10),
        row(2, "prefers tea", 99),
        row(7, "moved to Lisbon", 30),
    ]);
    let (folded, report) = fold_observation_rows(into, from).unwrap();
    assert_eq!(
        contents(&folded),
        vec![
            (1, "met at the lab".into(), false),
            (2, "prefers tea".into(), false),
            (3, "prefers tea".into(), false),
            (7, "moved to Lisbon".into(), false),
        ]
    );
    assert_eq!(report.added, 2);
    assert_eq!(report.renumbered, 1);
    assert_eq!(report.copies_dropped, 1);
}

#[test]
fn a_live_copy_of_a_retired_note_does_not_bring_it_back() {
    let into = rows(&[json!({"id": 1, "content": "old job", "observed_at": 5,
        "retired": {"at": 9, "by": "owner"}})]);
    let from = rows(&[row(4, "old job", 5)]);
    let (folded, report) = fold_observation_rows(into, from).unwrap();
    assert_eq!(contents(&folded), vec![(1, "old job".into(), true)]);
    assert_eq!(report.copies_dropped, 1);
    assert_eq!(report.added, 0);
}

#[test]
fn a_fold_preserves_retired_history_and_author() {
    let into = rows(&[row(3, "a", 1)]);
    let from = rows(&[
        json!({"id": 1, "content": "b", "observed_at": 2, "by": "owner",
        "history": [{"content": "b0", "observed_at": 1}],
        "retired": {"at": 3, "by": "owner"}, "extra": "kept"}),
    ]);
    let (folded, _) = fold_observation_rows(into, from).unwrap();
    let moved = &folded[1];
    assert_eq!(moved.id, 4);
    assert_eq!(moved.by.as_deref(), Some("owner"));
    assert_eq!(moved.history.len(), 1);
    assert_eq!(moved.retired.as_ref().unwrap().by, "owner");
    let text = crate::serialize_observation_rows(&folded);
    assert!(text.contains("\"extra\":\"kept\""));
    let ids: Vec<u64> = folded.iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![3, 4]);
}

#[test]
fn link_fields_combine_by_one_rule_and_a_link_in_the_facet_wins() {
    let mut into =
        json!({"entity_id": "ada", "attached_at": "2026-02-01", "updated_at": "2026-02-01",
        "detached": true, "description": ""})
        .as_object()
        .unwrap()
        .clone();
    let from = json!({"entity_id": "x", "attached_at": "2026-01-01", "updated_at": "2026-03-01",
        "description": "mathematician", "tags": ["a"]})
    .as_object()
    .unwrap()
    .clone();
    merge_link_fields(&mut into, &from);
    let expected: Map<String, Value> = json!({"entity_id": "ada", "attached_at": "2026-01-01",
        "updated_at": "2026-03-01", "description": "mathematician", "tags": ["a"]})
    .as_object()
    .unwrap()
    .clone();
    assert_eq!(into, expected);

    let mut both = json!({"detached": true}).as_object().unwrap().clone();
    merge_link_fields(&mut both, json!({"detached": true}).as_object().unwrap());
    assert_eq!(both.get("detached"), Some(&Value::Bool(true)));
}

#[test]
fn find_prefers_the_folder_named_by_the_id() {
    let journal = Journal::new();
    journal.link("aaa_legacy", json!({"entity_id": "ada"}));
    journal.link("ada", json!({"entity_id": "ada"}));
    let found = journal.dirs().find("ada").unwrap().unwrap();
    assert_eq!(found.dir, "ada");
    let all: Vec<String> = journal
        .dirs()
        .folders_for("ada")
        .unwrap()
        .into_iter()
        .map(|link| link.dir)
        .collect();
    assert_eq!(all, vec!["ada", "aaa_legacy"]);
}

#[test]
fn settle_leaves_a_canonical_link_alone() {
    let journal = Journal::new();
    journal.link("ada", json!({"entity_id": "ada"}));
    journal.notes("ada", &[row(1, "a", 1)]);
    let before = journal.tree();
    let (dir, _) = journal.dirs().settle("ada", &mut no_hook()).unwrap();
    assert_eq!(dir.as_deref(), Some("ada"));
    assert_eq!(journal.tree(), before);
}

#[test]
fn settle_renames_a_legacy_folder_and_folds_its_duplicates() {
    let journal = Journal::new();
    journal.link(
        "ada_lovelace",
        json!({"entity_id": "ada", "attached_at": "2026-01-02"}),
    );
    journal.notes("ada_lovelace", &[row(1, "wrote the notes", 1)]);
    journal.link(
        "countess",
        json!({"entity_id": "ada", "detached": true, "attached_at": "2026-01-01"}),
    );
    journal.notes(
        "countess",
        &[
            row(1, "translated Menabrea", 2),
            row(2, "wrote the notes", 1),
        ],
    );
    let (dir, report) = journal.dirs().settle("ada", &mut no_hook()).unwrap();
    assert_eq!(dir.as_deref(), Some("ada"));
    assert_eq!(journal.folders(), vec!["ada"]);
    assert_eq!(
        contents(&journal.read_notes("ada")),
        vec![
            (1, "wrote the notes".into(), false),
            (2, "translated Menabrea".into(), false),
        ]
    );
    assert_eq!(report.copies_dropped, 1);
    let link = journal.read_link("ada");
    assert_eq!(link["entity_id"], "ada");
    assert_eq!(link.get("detached"), None);
    assert_eq!(link["attached_at"], "2026-01-01");
}

#[test]
fn settle_adopts_an_orphan_folder_named_by_the_id() {
    let journal = Journal::new();
    journal.notes("ada", &[row(1, "unlinked note", 1)]);
    journal.link("ada_lovelace", json!({"entity_id": "ada"}));
    journal.notes("ada_lovelace", &[row(1, "linked note", 2)]);
    journal.dirs().settle("ada", &mut no_hook()).unwrap();
    assert_eq!(journal.folders(), vec!["ada"]);
    assert_eq!(
        contents(&journal.read_notes("ada")),
        // The link's own notes keep their ids; the unlinked ones follow.
        vec![
            (1, "linked note".into(), false),
            (2, "unlinked note".into(), false),
        ]
    );
    assert_eq!(journal.read_link("ada")["entity_id"], "ada");
}

#[test]
fn settle_never_moves_another_entitys_link() {
    let journal = Journal::new();
    journal.link("ada", json!({"entity_id": "babbage"}));
    journal.notes("ada", &[row(1, "babbage's note", 1)]);
    journal.link("ada_lovelace", json!({"entity_id": "ada"}));
    journal.notes("ada_lovelace", &[row(1, "ada's note", 1)]);
    let before = journal.tree();
    let error = journal.dirs().settle("ada", &mut no_hook()).unwrap_err();
    assert!(matches!(error, LinkFolderError::NeedsRepair { .. }));
    assert_eq!(journal.tree(), before);
}

#[test]
fn settle_refuses_when_the_id_folder_cannot_be_read_and_ignores_unrelated_damage() {
    let journal = Journal::new();
    fs::create_dir_all(journal.folder("ada")).unwrap();
    fs::write(journal.folder("ada").join("entity.json"), "{not json").unwrap();
    journal.link("ada_lovelace", json!({"entity_id": "ada"}));
    let before = journal.tree();
    assert!(matches!(
        journal.dirs().settle("ada", &mut no_hook()),
        Err(LinkFolderError::NeedsRepair { .. })
    ));
    assert_eq!(journal.tree(), before);

    journal.link("bob_smith", json!({"entity_id": "bob"}));
    let (dir, _) = journal.dirs().settle("bob", &mut no_hook()).unwrap();
    assert_eq!(dir.as_deref(), Some("bob"));
}

#[test]
fn a_conflicting_extra_file_refuses_before_any_write() {
    let journal = Journal::new();
    journal.link("ada", json!({"entity_id": "ada"}));
    fs::write(journal.folder("ada").join("notes.md"), "one").unwrap();
    journal.link("ada_2x", json!({"entity_id": "ada"}));
    journal.notes("ada_2x", &[row(1, "x", 1)]);
    fs::write(journal.folder("ada_2x").join("notes.md"), "two").unwrap();
    let before = journal.tree();
    assert!(matches!(
        journal.dirs().settle("ada", &mut no_hook()),
        Err(LinkFolderError::Conflict { .. })
    ));
    assert_eq!(journal.tree(), before);
}

#[test]
fn place_names_a_new_link_by_its_id_and_adopts_the_name_orphan() {
    let journal = Journal::new();
    journal.notes("ada_lovelace", &[row(1, "note before linking", 1)]);
    journal.notes("ada", &[row(1, "note by id", 2)]);
    let link = json!({"attached_at": "2026-01-01"})
        .as_object()
        .unwrap()
        .clone();
    let (dir, adopted) = journal
        .dirs()
        .place("ada", Some("ada_lovelace"), &link, &mut no_hook())
        .unwrap();
    assert_eq!(dir, "ada");
    assert_eq!(adopted.added, 1);
    assert_eq!(journal.folders(), vec!["ada"]);
    assert_eq!(
        contents(&journal.read_notes("ada")),
        vec![
            (1, "note by id".into(), false),
            (2, "note before linking".into(), false),
        ]
    );
    assert_eq!(journal.read_link("ada")["entity_id"], "ada");
}

#[test]
fn place_refuses_a_folder_held_by_another_entity() {
    let journal = Journal::new();
    journal.link("ada", json!({"entity_id": "babbage"}));
    let before = journal.tree();
    let link = Map::new();
    assert!(matches!(
        journal.dirs().place("ada", None, &link, &mut no_hook()),
        Err(LinkFolderError::NeedsRepair { .. })
    ));
    assert_eq!(journal.tree(), before);
    assert_eq!(
        journal.dirs().state("ada").unwrap(),
        FolderState::Link(journal.dirs().read_link("ada").unwrap().unwrap())
    );
}

#[test]
fn take_in_folds_a_link_from_elsewhere_into_the_entitys_folder() {
    let journal = Journal::new();
    journal.link("ada", json!({"entity_id": "ada", "detached": true}));
    journal.notes("ada", &[row(1, "here", 1)]);
    let other = Journal::new();
    other.link("countess", json!({"entity_id": "ada"}));
    other.notes("countess", &[row(1, "there", 2)]);
    let (dir, report) = journal
        .dirs()
        .take_in(
            "ada",
            &other.dirs(),
            "countess",
            LinkFieldPolicy::Merge,
            &mut no_hook(),
        )
        .unwrap();
    assert_eq!(dir, "ada");
    assert_eq!(report.renumbered, 1);
    assert_eq!(
        contents(&journal.read_notes("ada")),
        vec![(1, "here".into(), false), (2, "there".into(), false)]
    );
    assert_eq!(journal.read_link("ada").get("detached"), None);
    assert!(other.folder("countess").exists());

    let detached_target = Journal::new();
    detached_target.link("ada", json!({"entity_id": "ada", "detached": true}));
    detached_target
        .dirs()
        .take_in(
            "ada",
            &other.dirs(),
            "countess",
            LinkFieldPolicy::TargetWins,
            &mut no_hook(),
        )
        .unwrap();
    assert_eq!(detached_target.read_link("ada")["detached"], true);
}

#[test]
fn take_in_places_an_unlinked_entity_in_the_folder_named_by_its_id() {
    let journal = Journal::new();
    let other = Journal::new();
    other.link(
        "ada_lovelace",
        json!({"entity_id": "ada", "description": "d"}),
    );
    other.notes("ada_lovelace", &[row(5, "kept id", 1)]);
    journal
        .dirs()
        .take_in(
            "ada",
            &other.dirs(),
            "ada_lovelace",
            LinkFieldPolicy::Merge,
            &mut no_hook(),
        )
        .unwrap();
    assert_eq!(journal.folders(), vec!["ada"]);
    assert_eq!(journal.read_link("ada")["description"], "d");
    assert_eq!(
        contents(&journal.read_notes("ada")),
        vec![(5, "kept id".into(), false)]
    );
}

#[test]
fn every_write_is_announced_to_the_hook_first() {
    let journal = Journal::new();
    journal.link("ada_lovelace", json!({"entity_id": "ada"}));
    journal.link("countess", json!({"entity_id": "ada"}));
    let mut seen = Vec::new();
    journal
        .dirs()
        .settle("ada", &mut |rel: &str| {
            seen.push(rel.to_owned());
            Ok(())
        })
        .unwrap();
    for folder in ["ada_lovelace", "ada", "countess"] {
        assert!(
            seen.contains(&format!("facets/work/entities/{folder}")),
            "{folder} not announced: {seen:?}"
        );
    }
}

#[test]
fn an_owner_note_follows_a_link_that_moved_and_is_refused_when_it_is_gone() {
    let journal = Journal::new();
    journal.link("ada_lovelace", json!({"entity_id": "ada"}));
    // The folder is settled to its id after the name was resolved.
    journal.dirs().settle("ada", &mut no_hook()).unwrap();
    let written = crate::add_observation_for_entity(&journal.0, "work", "ada", "met", None, None)
        .unwrap()
        .expect("the entity is still linked");
    assert_eq!(written.1, 1);
    assert_eq!(journal.folders(), vec!["ada"]);

    // Merged away between resolving the name and writing: nothing is written.
    fs::remove_dir_all(journal.folder("ada")).unwrap();
    let refused =
        crate::add_observation_for_entity(&journal.0, "work", "ada", "late", None, None).unwrap();
    assert!(refused.is_none());
    assert!(journal.folders().is_empty());
}

#[test]
fn a_fold_gives_duplicate_ids_already_in_the_receiving_file_their_own() {
    let into = rows(&[row(1, "a", 1), row(1, "b", 2)]);
    let (folded, report) = fold_observation_rows(into, rows(&[row(9, "c", 3)])).unwrap();
    let ids: Vec<u64> = folded.iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![1, 2, 9]);
    assert_eq!(report.renumbered, 1);
}

#[test]
fn a_name_folder_is_adopted_only_when_no_other_entity_answers_to_it() {
    use crate::facet_links::adoptable_name_folder;
    let journal = Journal::new();
    crate::save_entity_identity(
        &journal.0,
        "ada",
        &json!({"id": "ada", "name": "Ada Lovelace"}),
        None,
    )
    .unwrap();
    crate::save_entity_identity(
        &journal.0,
        "acme_2",
        &json!({"id": "acme_2", "name": "Acme"}),
        None,
    )
    .unwrap();
    // A live entity whose id is `acme`, filed under a different directory.
    fs::create_dir_all(journal.0.join("entities/acme_legacy")).unwrap();
    fs::write(
        journal.0.join("entities/acme_legacy/entity.json"),
        serde_json::to_vec(&json!({"id": "acme", "name": "Acme Inc"})).unwrap(),
    )
    .unwrap();
    // No folder under the name: nothing to adopt.
    assert_eq!(
        adoptable_name_folder(&journal.0, "work", "ada", "Ada Lovelace").unwrap(),
        None
    );
    journal.notes("ada_lovelace", &[row(1, "before linking", 1)]);
    journal.notes("acme", &[row(1, "about acme inc", 1)]);
    assert_eq!(
        adoptable_name_folder(&journal.0, "work", "ada", "Ada Lovelace").unwrap(),
        Some("ada_lovelace".to_owned())
    );
    // Another live entity answers to the slug: its notes are its own.
    assert_eq!(
        adoptable_name_folder(&journal.0, "work", "acme_2", "Acme").unwrap(),
        None
    );
    assert_eq!(
        adoptable_name_folder(&journal.0, "work", "ada", "ada").unwrap(),
        None
    );
}

#[test]
fn an_id_that_cant_name_a_folder_is_never_used_as_one() {
    let journal = Journal::new();
    journal.link("x", json!({"entity_id": "."}));
    journal.link("y", json!({"entity_id": "a/b"}));
    let (links, unreadable) = journal.dirs().scan().unwrap();
    assert!(links.is_empty());
    assert_eq!(unreadable, vec!["x", "y"]);
    let before = journal.tree();
    for bad in [".", "..", "a/b", "", ".hidden"] {
        assert!(matches!(
            journal.dirs().settle(bad, &mut no_hook()),
            Err(LinkFolderError::InvalidName { .. })
        ));
        assert!(matches!(
            journal.dirs().place(bad, None, &Map::new(), &mut no_hook()),
            Err(LinkFolderError::InvalidName { .. })
        ));
    }
    assert_eq!(journal.tree(), before);
}

#[test]
fn settle_refuses_two_duplicates_whose_extra_files_differ_before_writing() {
    let journal = Journal::new();
    journal.link("a_one", json!({"entity_id": "ada"}));
    fs::write(journal.folder("a_one").join("x.txt"), "one").unwrap();
    journal.link("a_two", json!({"entity_id": "ada"}));
    fs::write(journal.folder("a_two").join("x.txt"), "two").unwrap();
    let before = journal.tree();
    assert!(matches!(
        journal.dirs().settle("ada", &mut no_hook()),
        Err(LinkFolderError::Conflict { .. })
    ));
    assert_eq!(journal.tree(), before);
}

fn identity(journal: &Journal, id: &str) {
    crate::save_entity_identity(&journal.0, id, &json!({"id": id, "name": id}), None).unwrap();
}

#[test]
fn the_doctor_reports_then_repairs_what_needs_no_judgment_and_leaves_the_rest() {
    use crate::facet_links::{LinkIssueKind, check_journal_links, repair_journal_links};
    let journal = Journal::new();
    for id in ["ada", "bob", "carol", "fred", "xavier", "gone"] {
        identity(&journal, id);
    }
    // A duplicate, a misnamed folder, and a chain: folder `xavier` holds
    // fred's link while xavier's own sits in `xx`.
    journal.link("ada", json!({"entity_id": "ada"}));
    journal.notes("ada", &[row(1, "one", 1)]);
    journal.link("ada_copy", json!({"entity_id": "ada"}));
    journal.notes("ada_copy", &[row(1, "two", 2)]);
    journal.link("bobby", json!({"entity_id": "bob"}));
    journal.link("xavier", json!({"entity_id": "fred"}));
    journal.notes("xavier", &[row(1, "fred's note", 1)]);
    journal.link("xx", json!({"entity_id": "xavier"}));
    journal.notes("xx", &[row(1, "xavier's note", 1)]);
    // A link to a merged-away id, and one to a deleted id.
    journal.link("merged_one", json!({"entity_id": "merged_one"}));
    journal.notes("merged_one", &[row(1, "kept by carol", 1)]);
    journal.link("gone_link", json!({"entity_id": "deleted_one"}));
    fs::write(
        journal.0.join("entities/retired.json"),
        serde_json::to_vec(&json!({"ids": {
            "merged_one": {"state": "merged", "dir": "merged_one", "successor": "carol"},
            "deleted_one": {"state": "deleted", "dir": "deleted_one"},
        }}))
        .unwrap(),
    )
    .unwrap();
    let before = journal.tree();

    let found = check_journal_links(&journal.0).unwrap();
    assert_eq!(journal.tree(), before, "checking changes nothing");
    let mut kinds: Vec<(String, LinkIssueKind)> = found
        .iter()
        .map(|issue| (issue.entity_id.clone(), issue.kind.clone()))
        .collect();
    kinds.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        kinds,
        vec![
            ("ada".to_owned(), LinkIssueKind::Duplicate),
            ("bob".to_owned(), LinkIssueKind::Misnamed),
            ("deleted_one".to_owned(), LinkIssueKind::Deleted),
            ("fred".to_owned(), LinkIssueKind::Misnamed),
            (
                "merged_one".to_owned(),
                LinkIssueKind::Merged {
                    successor: "carol".to_owned()
                }
            ),
            ("xavier".to_owned(), LinkIssueKind::Misnamed),
        ]
    );

    let (repaired, left) = repair_journal_links(&journal.0).unwrap();
    assert_eq!(repaired.len(), 5, "{repaired:?}");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].kind, LinkIssueKind::Deleted);
    assert_eq!(
        journal.folders(),
        vec!["ada", "bob", "carol", "fred", "gone_link", "xavier"]
    );
    assert_eq!(journal.read_notes("fred")[0].content, "fred's note");
    assert_eq!(journal.read_notes("xavier")[0].content, "xavier's note");
    assert_eq!(journal.read_notes("carol")[0].content, "kept by carol");
    assert_eq!(journal.read_link("carol")["entity_id"], "carol");
    assert_eq!(journal.read_notes("ada").len(), 2);

    let (again, still) = repair_journal_links(&journal.0).unwrap();
    assert!(again.is_empty());
    assert_eq!(still, left);
}

#[test]
fn desktop_litter_never_blocks_a_fold_and_one_notes_file_moves_as_it_is() {
    let journal = Journal::new();
    journal.link("a_one", json!({"entity_id": "ada"}));
    fs::write(journal.folder("a_one").join(".DS_Store"), "one").unwrap();
    journal.link("a_two", json!({"entity_id": "ada"}));
    fs::write(journal.folder("a_two").join(".DS_Store"), "two").unwrap();
    // Notes that don't parse, with nothing to combine them with.
    fs::write(
        journal.folder("a_two").join("observations.jsonl"),
        "{not json\n",
    )
    .unwrap();
    let (dir, _) = journal.dirs().settle("ada", &mut no_hook()).unwrap();
    assert_eq!(dir.as_deref(), Some("ada"));
    assert_eq!(journal.folders(), vec!["ada"]);
    assert_eq!(
        fs::read_to_string(journal.folder("ada").join("observations.jsonl")).unwrap(),
        "{not json\n"
    );
}

#[test]
fn the_targets_own_link_sets_the_fields_and_keeps_its_note_ids() {
    let journal = Journal::new();
    // bob_2's link sits in folder `bob`; bob's own link is in `bob_x`.
    journal.link(
        "bob",
        json!({"entity_id": "bob_2", "description": "source desc"}),
    );
    journal.notes("bob", &[row(1, "from bob_2", 1)]);
    journal.link(
        "bob_x",
        json!({"entity_id": "bob", "description": "target desc"}),
    );
    journal.notes("bob_x", &[row(1, "bob one", 2), row(2, "bob two", 3)]);
    let (dir, report) = journal
        .dirs()
        .take_in(
            "bob",
            &journal.dirs(),
            "bob",
            LinkFieldPolicy::Merge,
            &mut no_hook(),
        )
        .unwrap();
    assert_eq!(dir, "bob");
    assert_eq!(journal.folders(), vec!["bob"]);
    let link = journal.read_link("bob");
    assert_eq!(link["entity_id"], "bob");
    assert_eq!(link["description"], "target desc");
    assert_eq!(
        contents(&journal.read_notes("bob")),
        vec![
            (1, "bob one".into(), false),
            (2, "bob two".into(), false),
            (3, "from bob_2".into(), false),
        ]
    );
    assert_eq!(report.added, 1);
}

#[test]
fn added_counts_only_notes_that_came_in() {
    let journal = Journal::new();
    journal.link("ada_old", json!({"entity_id": "ada"}));
    journal.notes("ada_old", &[row(1, "a", 1), row(2, "b", 2), row(3, "c", 3)]);
    // Renaming the entity's own folder brings nothing in.
    let (_, settled) = journal.dirs().settle("ada", &mut no_hook()).unwrap();
    assert_eq!(settled.added, 0);
    let other = Journal::new();
    other.link("one", json!({"entity_id": "zed"}));
    other.notes("one", &[row(1, "x", 1), row(2, "y", 2)]);
    other.link("two", json!({"entity_id": "zed"}));
    other.notes("two", &[row(1, "z", 3), row(2, "w", 4)]);
    let target = Journal::new();
    let (_, taken) = target
        .dirs()
        .take_in_all(
            "zed",
            &[(&other.dirs(), "one"), (&other.dirs(), "two")],
            LinkFieldPolicy::Merge,
            &mut no_hook(),
        )
        .unwrap();
    assert_eq!(taken.added, 4);
}

#[test]
fn a_fold_interrupted_after_writing_its_result_heals_on_the_next_run() {
    let journal = Journal::new();
    // The result was written and the old folder not yet removed; the notes
    // don't parse, so only their bytes can be compared.
    for dir in ["ada", "a_two"] {
        journal.link(dir, json!({"entity_id": "ada"}));
        fs::write(
            journal.folder(dir).join("observations.jsonl"),
            "{not json\n",
        )
        .unwrap();
    }
    journal.dirs().settle("ada", &mut no_hook()).unwrap();
    assert_eq!(journal.folders(), vec!["ada"]);
}

#[test]
fn the_doctor_folds_a_merged_ids_folders_into_its_successor_in_place() {
    use crate::facet_links::repair_journal_links;
    let journal = Journal::new();
    identity(&journal, "bob");
    journal.link("a_b", json!({"entity_id": "bob_2"}));
    journal.notes("a_b", &[row(1, "first", 1)]);
    journal.link("bob", json!({"entity_id": "bob_2"}));
    journal.notes("bob", &[row(1, "second", 2)]);
    fs::write(
        journal.0.join("entities/retired.json"),
        serde_json::to_vec(&json!({"ids": {
            "bob_2": {"state": "merged", "dir": "bob_2", "successor": "bob"},
        }}))
        .unwrap(),
    )
    .unwrap();
    let (repaired, left) = repair_journal_links(&journal.0).unwrap();
    assert_eq!(repaired.len(), 1, "{left:?}");
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(journal.folders(), vec!["bob"]);
    assert_eq!(journal.read_link("bob")["entity_id"], "bob");
    assert_eq!(journal.read_notes("bob").len(), 2);
}

#[test]
fn an_unlinked_folder_a_link_cant_go_into_is_refused_before_anything_is_created() {
    let journal = Journal::new();
    fs::create_dir_all(journal.folder("zed").join("stray")).unwrap();
    assert!(matches!(
        journal.dirs().check_placeable("zed"),
        Err(LinkFolderError::NotAFile { .. })
    ));
}
