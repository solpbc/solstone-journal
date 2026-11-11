// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test code"
)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_entity::{AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation};
use solstone_core_journal_io::MalformedPolicy;
use solstone_core_removals::{Action, Event, Surface, Unit};
use solstone_core_retention::notify::{IndexNotify, NotifyError, PruneCounts};
use solstone_core_retention::receipt::RemovedPath;
use solstone_core_trash::{Change, Chosen, Hold, Item, Kind, SetResult, State, TrashError};

use super::*;

/// What search was told.
#[derive(Default)]
struct Told {
    removed: RefCell<Vec<String>>,
    returned: RefCell<Vec<String>>,
}

impl IndexNotify for Told {
    fn paths_removed(&self, removed: &[RemovedPath]) -> Result<PruneCounts, NotifyError> {
        self.removed
            .borrow_mut()
            .extend(removed.iter().map(|path| path.as_str().to_owned()));
        Ok(PruneCounts::default())
    }

    fn paths_returned(&self, returned: &[String]) -> Result<(), NotifyError> {
        self.returned.borrow_mut().extend(returned.iter().cloned());
        Ok(())
    }
}

struct Journal(tempfile::TempDir);

impl Journal {
    fn new() -> Self {
        let journal = Self(tempfile::TempDir::new().unwrap());
        for facet in ["work", "home"] {
            journal.facet(facet);
        }
        journal
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    fn facet(&self, name: &str) {
        crate::create_facet(self.root(), name, name, "", "blue", "💼", None).unwrap();
    }

    fn person(&self, id: &str, name: &str) {
        self.entity_in(id, id, name);
    }

    fn entity_in(&self, dir: &str, id: &str, name: &str) {
        let path = self.root().join("entities").join(dir);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("entity.json"),
            json!({"id": id, "name": name, "type": "Person", "created_at": "2026-01-01T00:00:00Z"})
                .to_string(),
        )
        .unwrap();
        fs::write(path.join("voiceprints.npz"), b"voice").unwrap();
        solstone_core_entity::rewrite_identity_map_cache(self.root()).unwrap();
    }

    fn link(&self, facet: &str, folder: &str, id: &str) {
        let path = self
            .root()
            .join("facets")
            .join(facet)
            .join("entities")
            .join(folder);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("entity.json"),
            json!({"entity_id": id}).to_string(),
        )
        .unwrap();
        fs::write(
            path.join("observations.jsonl"),
            format!("{{\"content\":\"a note about {id}\"}}\n"),
        )
        .unwrap();
    }

    /// Every file under `entities/` and `facets/`, by path.
    fn tree(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            let Ok(listing) = fs::read_dir(dir) else {
                return;
            };
            for entry in listing.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    let rel = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    // Lock files and the map cache come and go with every write.
                    if !rel.ends_with(".lock") && !rel.contains("identity_map") {
                        out.insert(rel, fs::read(&path).unwrap());
                    }
                }
            }
        }
        let mut out = BTreeMap::new();
        for top in ["entities", "facets"] {
            walk(self.root(), &self.root().join(top), &mut out);
        }
        // The record of kept ids only grows, and ambiguity rows are compared
        // by field.
        out.retain(|rel, _| {
            !rel.starts_with("entities/retired.json") && rel != "entities/ambiguities.jsonl"
        });
        out
    }

    fn retired(&self) -> Value {
        serde_json::from_slice(&fs::read(self.root().join("entities/retired.json")).unwrap())
            .unwrap()
    }

    fn events(&self) -> Vec<Event> {
        let mut entries = solstone_core_removals::history(self.root(), 100)
            .unwrap()
            .entries;
        entries.reverse();
        entries
            .into_iter()
            .filter(|entry| entry.unit == Unit::Entity)
            .map(|entry| entry.event)
            .collect()
    }

    fn observe(&self, query: &str, candidates: &[&str]) {
        solstone_core_entity::record_ambiguity_observation(
            self.root(),
            &AmbiguityObservation {
                scope: json!({"kind": "journal"}),
                query: query.to_owned(),
                normalized_query: query.to_owned(),
                observed_tier: 5,
                ranked_candidates: candidates
                    .iter()
                    .map(|id| json!({"id": id, "name": id, "tier": 5, "score": 1.0}))
                    .collect(),
                origin: json!({"lane": "segment", "day": "20260805", "segment_id": query}),
            },
        )
        .unwrap();
    }

    fn choose(&self, query: &str, chosen: &str, candidates: &[&str]) {
        solstone_core_entity::record_ambiguity_choice(
            self.root(),
            &AmbiguityChoiceRequest {
                scope: json!({"kind": "journal"}),
                query: query.to_owned(),
                entity_id: chosen.to_owned(),
                origin: None,
            },
            &candidates
                .iter()
                .map(|id| AmbiguityChoiceEntity {
                    id: (*id).to_owned(),
                    blocked: false,
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }

    /// The row for `query`: its candidates, in order, and what it resolves to.
    fn row(&self, query: &str) -> (Vec<String>, Option<String>) {
        let rows =
            solstone_core_entity::read_ambiguities(self.root(), MalformedPolicy::Raise).unwrap();
        let row = rows
            .iter()
            .find(|row| row["normalized_query"] == query)
            .unwrap();
        (
            row["ranked_candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|candidate| candidate["id"].as_str().unwrap().to_owned())
                .collect(),
            row["resolved_entity_id"].as_str().map(str::to_owned),
        )
    }
}

fn trash(journal: &Journal, told: &Told, id: &str) -> Result<Item, TrashError> {
    Entities::new(told).trash(journal.root(), id, Action::EntityDelete, Surface::Cli)
}

fn recover(journal: &Journal, told: &Told, item: &str) -> Result<Item, TrashError> {
    solstone_core_trash::attach(
        journal.root(),
        &[&Entities::new(told)],
        item,
        Action::Recover,
        Surface::Cli,
    )
}

fn erase(journal: &Journal, item: &str) {
    solstone_core_trash::erase(journal.root(), item, Action::Erase, Surface::Cli).unwrap();
}

/// Bob, linked in two facets, and Ann.
fn bob_and_ann() -> Journal {
    let journal = Journal::new();
    journal.person("bob", "Bob");
    journal.person("ann", "Ann");
    journal.link("work", "bob", "bob");
    journal.link("home", "bob", "bob");
    journal.link("work", "ann", "ann");
    journal
}

#[test]
fn an_entity_goes_to_the_trash_with_its_links_and_comes_back_exactly() {
    let journal = bob_and_ann();
    let told = Told::default();
    let before = journal.tree();

    let item = trash(&journal, &told, "bob").unwrap();
    assert_eq!(item.unit, Unit::Entity);
    assert_eq!(item.label.as_deref(), Some("Bob"));
    let paths: BTreeSet<&str> = item.paths.iter().map(String::as_str).collect();
    assert_eq!(
        paths,
        BTreeSet::from([
            "entities/bob",
            "facets/home/entities/bob",
            "facets/work/entities/bob"
        ])
    );
    assert!(!journal.root().join("entities/bob").exists());
    assert!(!journal.root().join("facets/work/entities/bob").exists());
    assert!(journal.root().join("facets/work/entities/ann").exists());
    assert!(
        solstone_core_entity::read_identity_map(journal.root())
            .unwrap()
            .resolved
            .get("bob")
            .is_none()
    );
    assert!(
        told.removed
            .borrow()
            .contains(&"entities/bob/voiceprints.npz".to_owned())
    );
    // The id is kept, with its folder and never its name.
    let entry = &journal.retired()["ids"]["bob"];
    assert_eq!(entry["state"], "deleted");
    assert_eq!(entry["dir"], "bob");
    assert!(entry.get("name").is_none());

    recover(&journal, &told, &item.id).unwrap();
    assert_eq!(journal.tree(), before);
    assert!(
        told.returned
            .borrow()
            .contains(&"facets/home/entities/bob/observations.jsonl".to_owned())
    );
    assert_eq!(
        solstone_core_entity::read_identity_map(journal.root())
            .unwrap()
            .resolved
            .get("bob"),
        Some(&"bob".to_owned())
    );
    // A recovered entity is live: it can be changed as any other.
    solstone_core_entity::save_entity_identity(
        journal.root(),
        "bob",
        &json!({"id": "bob", "name": "Bob", "type": "Person", "aka": ["Bobby"]}),
        None,
    )
    .unwrap();

    let again = trash(&journal, &told, "bob").unwrap();
    erase(&journal, &again.id);
    assert!(!journal.root().join("entities/bob").exists());
    assert_eq!(
        journal.events(),
        [
            Event::Trashed,
            Event::Recovered,
            Event::Trashed,
            Event::Erased
        ]
    );
    // After erase the record holds the id and its folder, and no name.
    let text = fs::read_to_string(journal.root().join("entities/retired.json")).unwrap();
    assert!(!text.contains("Bob"), "{text}");
}

/// The entity kind, with its first detach's references failing as a stop
/// right after the move would leave them.
struct StopsOnce<'a> {
    inner: Entities<'a>,
    stop: Cell<bool>,
}

impl Kind for StopsOnce<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        self.inner.handles(unit)
    }
    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        self.inner.hold(journal, item)
    }
    fn present(&self, journal: &Path, path: &str) -> bool {
        self.inner.present(journal, path)
    }
    fn current_paths(&self, journal: &Path, unit: &Unit, chosen: &[String]) -> Option<Vec<String>> {
        self.inner.current_paths(journal, unit, chosen)
    }
    fn returns(&self, journal: &Path, item: &Item, path: &str) -> bool {
        self.inner.returns(journal, item, path)
    }
    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        if self.stop.replace(false) {
            return Err("stopped".to_owned());
        }
        self.inner.references(journal, item, change, files)
    }
}

#[test]
fn a_detach_a_stop_interrupted_is_finished_by_the_sweep_and_recovers_exactly() {
    let journal = bob_and_ann();
    let told = Told::default();
    journal.observe("robert", &["ann", "bob"]);
    journal.choose("robert", "bob", &["ann", "bob"]);
    let before = journal.tree();
    let rows_before =
        solstone_core_entity::read_ambiguities(journal.root(), MalformedPolicy::Raise).unwrap();
    let stops = StopsOnce {
        inner: Entities::new(&told),
        stop: Cell::new(true),
    };
    let chosen = chosen_entity(journal.root(), "bob").unwrap().unwrap();
    let error = solstone_core_trash::detach_labelled(
        journal.root(),
        &stops,
        Unit::Entity,
        Action::EntityDelete,
        Surface::Cli,
        chosen.paths,
        chosen.label,
    )
    .unwrap_err();
    let TrashError::Unfinished(id) = error else {
        panic!("{error:?}");
    };
    assert_eq!(
        solstone_core_trash::item(journal.root(), &id)
            .unwrap()
            .unwrap()
            .state,
        State::Detaching
    );

    let swept =
        solstone_core_trash::sweep(journal.root(), &[&Entities::new(&told)], chrono::Utc::now());
    assert_eq!(swept.resumed, 1, "{swept:?}");
    assert_eq!(journal.row("robert"), (vec!["ann".to_owned()], None));

    recover(&journal, &told, &id).unwrap();
    assert_eq!(journal.tree(), before);
    assert_eq!(
        solstone_core_entity::read_ambiguities(journal.root(), MalformedPolicy::Raise).unwrap(),
        rows_before
    );
    assert_eq!(
        journal.row("robert"),
        (
            vec!["ann".to_owned(), "bob".to_owned()],
            Some("bob".to_owned())
        )
    );
    assert_eq!(journal.events(), [Event::Trashed, Event::Recovered]);
}

#[test]
fn a_kept_id_with_no_other_trace_gives_a_new_entity_the_next_id_and_no_old_notes() {
    let journal = Journal::new();
    solstone_core_entity::record_deleted_entity(journal.root(), "bob", "bob").unwrap();
    // Notes written under old Bob's name, in a folder the delete didn't take.
    let orphan = journal.root().join("facets/work/entities/bob");
    fs::create_dir_all(&orphan).unwrap();
    fs::write(orphan.join("observations.jsonl"), "{\"content\":\"old\"}\n").unwrap();

    let created =
        crate::create_facet_entity_if_absent(journal.root(), "work", "Person", "Bob", "").unwrap();
    let crate::FacetEntityCreateOutcome::Created { entity_id, .. } = created else {
        panic!("{created:?}");
    };
    assert_eq!(entity_id, "bob_2");
    assert!(orphan.join("observations.jsonl").is_file());
    assert!(!orphan.join("entity.json").exists());
    assert!(
        !journal
            .root()
            .join("facets/work/entities/bob_2/observations.jsonl")
            .exists()
    );
}

#[test]
fn recovering_an_entity_leaves_the_new_one_of_its_name_beside_it() {
    let journal = bob_and_ann();
    let told = Told::default();
    let item = trash(&journal, &told, "bob").unwrap();
    let created =
        crate::attach_or_reactivate_entity_for_owner(journal.root(), "work", "Person", "Bob", "")
            .unwrap();
    assert_eq!(created.relationship["entity_id"], "bob_2");
    assert!(trash_view(journal.root(), &item).same_name);

    recover(&journal, &told, &item.id).unwrap();
    let map = solstone_core_entity::read_identity_map(journal.root()).unwrap();
    assert_eq!(map.resolved.get("bob"), Some(&"bob".to_owned()));
    assert_eq!(map.resolved.get("bob_2"), Some(&"bob_2".to_owned()));
    assert!(journal.root().join("facets/work/entities/bob").is_dir());
    assert!(journal.root().join("facets/work/entities/bob_2").is_dir());
}

#[test]
fn five_entities_in_one_set_each_hold_only_their_own() {
    let journal = Journal::new();
    let told = Told::default();
    let ids = ["ada", "bea", "cyd", "dov", "eli"];
    for id in ids {
        journal.person(id, id);
        journal.link("work", id, id);
    }
    journal.link("home", "ada", "ada");
    let chosen: Vec<Chosen> = ids
        .iter()
        .map(|id| chosen_entity(journal.root(), id).unwrap().unwrap())
        .collect();
    let results = solstone_core_trash::detach_set(
        journal.root(),
        &[&Entities::new(&told)],
        chosen,
        Action::EntityDelete,
        Surface::Cli,
        false,
    )
    .unwrap();
    assert_eq!(results.len(), 5);
    for (id, result) in ids.iter().zip(results) {
        let SetResult::Moved { item, erased } = result else {
            panic!("{id}: {result:?}");
        };
        assert!(!erased);
        let mut expected = vec![
            format!("entities/{id}"),
            format!("facets/work/entities/{id}"),
        ];
        if *id == "ada" {
            expected.push("facets/home/entities/ada".to_owned());
        }
        let mut paths = item.paths.clone();
        paths.sort();
        expected.sort();
        assert_eq!(paths, expected, "{id}");
    }
    assert!(
        solstone_core_entity::read_identity_map(journal.root())
            .unwrap()
            .resolved
            .is_empty()
    );
}

#[test]
fn an_ambiguity_comes_back_by_field_around_what_happened_meanwhile() {
    let journal = bob_and_ann();
    let told = Told::default();
    journal.person("cat", "Cat");
    // Resolved to Bob, then an observation of the same name during the trash.
    journal.observe("robert", &["ann", "bob"]);
    journal.choose("robert", "bob", &["ann", "bob"]);
    // Resolved to Bob, then the owner resolves it to someone else meanwhile.
    journal.observe("rob", &["ann", "bob", "cat"]);
    journal.choose("rob", "bob", &["ann", "bob", "cat"]);

    let item = trash(&journal, &told, "bob").unwrap();
    assert_eq!(journal.row("robert"), (vec!["ann".to_owned()], None));
    assert_eq!(
        journal.row("rob"),
        (vec!["ann".to_owned(), "cat".to_owned()], None)
    );
    journal.observe("robert", &["ann"]);
    journal.choose("rob", "cat", &["ann", "cat"]);

    recover(&journal, &told, &item.id).unwrap();
    assert_eq!(
        journal.row("robert"),
        (
            vec!["ann".to_owned(), "bob".to_owned()],
            Some("bob".to_owned())
        )
    );
    assert_eq!(
        journal.row("rob"),
        (
            vec!["ann".to_owned(), "cat".to_owned(), "bob".to_owned()],
            Some("cat".to_owned())
        )
    );
}

#[test]
fn two_entities_stripped_from_one_row_come_back_in_either_order() {
    for bob_first in [true, false] {
        let journal = bob_and_ann();
        let told = Told::default();
        journal.person("cat", "Cat");
        journal.observe("robert", &["ann", "bob", "cat"]);
        journal.choose("robert", "bob", &["ann", "bob", "cat"]);
        let bob = trash(&journal, &told, "bob").unwrap();
        let cat = trash(&journal, &told, "cat").unwrap();
        assert_eq!(journal.row("robert"), (vec!["ann".to_owned()], None));
        let order = if bob_first {
            [&bob, &cat]
        } else {
            [&cat, &bob]
        };
        for item in order {
            recover(&journal, &told, &item.id).unwrap();
        }
        let (mut candidates, resolved) = journal.row("robert");
        candidates.sort();
        assert_eq!(candidates, ["ann", "bob", "cat"], "bob first: {bob_first}");
        assert_eq!(resolved.as_deref(), Some("bob"), "bob first: {bob_first}");
    }
}

#[test]
fn an_erased_entity_whose_folder_differs_from_its_id_keeps_both_stepped_past() {
    let journal = Journal::new();
    let told = Told::default();
    journal.entity_in("bob_dir", "bob", "Bob");
    journal.link("work", "bob", "bob");
    let item = trash(&journal, &told, "bob").unwrap();
    erase(&journal, &item.id);
    let entry = &journal.retired()["ids"]["bob"];
    assert_eq!(entry["dir"], "bob_dir");
    assert!(entry.get("name").is_none());
    assert_eq!(
        solstone_core_entity::fresh_entity_id(journal.root(), "bob").unwrap(),
        "bob_2"
    );
    assert_eq!(
        solstone_core_entity::fresh_entity_id(journal.root(), "bob_dir").unwrap(),
        "bob_dir_2"
    );
}

/// A facet in the trash, as its own kind would hold it.
struct FacetStandIn;

struct NoHold;

impl Hold for NoHold {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Kind for FacetStandIn {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Facet)
    }
    fn hold<'h>(&'h self, _journal: &Path, _item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        Ok(Box::new(NoHold))
    }
    fn present(&self, journal: &Path, path: &str) -> bool {
        journal.join(path).join("facet.json").is_file()
    }
    fn references(&self, _: &Path, _: &Item, _: Change, _: &[String]) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn a_link_whose_facet_is_in_the_trash_waits_for_it_then_goes_once_it_is_erased() {
    let journal = bob_and_ann();
    let told = Told::default();
    let bob = trash(&journal, &told, "bob").unwrap();
    let facet = solstone_core_trash::detach_labelled(
        journal.root(),
        &FacetStandIn,
        Unit::Facet,
        Action::FacetDelete,
        Surface::Cli,
        vec!["facets/work".to_owned()],
        Some("Work".to_owned()),
    )
    .unwrap();

    let refused = recover(&journal, &told, &bob.id).unwrap_err();
    let TrashError::Held(words) = &refused else {
        panic!("{refused:?}");
    };
    let words: Value = serde_json::from_str(words).unwrap();
    assert_eq!(
        words,
        json!({"code": "waits_facets", "title": "Work", "others": 0})
    );
    assert_eq!(refusal_code(&refused), "waits_facets");
    // Refused before it started: still in the trash, and nothing moved.
    assert_eq!(
        solstone_core_trash::item(journal.root(), &bob.id)
            .unwrap()
            .unwrap()
            .state,
        State::Trashed
    );
    assert!(!journal.root().join("entities/bob").exists());

    erase(&journal, &facet.id);
    recover(&journal, &told, &bob.id).unwrap();
    assert!(journal.root().join("entities/bob/entity.json").is_file());
    assert!(journal.root().join("facets/home/entities/bob").is_dir());
    // The link to the facet that's gone went with the item, and didn't make
    // a facet folder holding only it.
    assert!(!journal.root().join("facets/work").exists());
    assert_eq!(
        journal.events(),
        [Event::Trashed, Event::Erased, Event::Recovered]
    );
}

#[test]
fn the_owner_and_a_damaged_record_are_refused_and_nothing_moves() {
    let journal = bob_and_ann();
    let told = Told::default();
    journal.entity_in("me", "me", "Me");
    let identity = journal.root().join("entities/me/entity.json");
    fs::write(
        &identity,
        json!({"id": "me", "name": "Me", "type": "Person", "is_principal": true}).to_string(),
    )
    .unwrap();
    let refused = trash(&journal, &told, "me").unwrap_err();
    assert_eq!(refusal_code(&refused), "principal");
    assert!(identity.is_file());
    assert!(!journal.root().join("entities/retired.json").exists());

    fs::write(journal.root().join("entities/retired.json"), "{not json").unwrap();
    let before = journal.tree();
    let refused = trash(&journal, &told, "bob").unwrap_err();
    assert_eq!(refusal_code(&refused), "record_damaged");
    assert_eq!(journal.tree(), before);

    let missing = trash(&journal, &told, "nobody").unwrap_err();
    assert_eq!(refusal_code(&missing), "gone");
}

#[test]
fn a_recover_waits_while_another_entity_has_its_id() {
    let journal = bob_and_ann();
    let told = Told::default();
    let item = trash(&journal, &told, "bob").unwrap();
    // An entity from before the record, holding the id in another folder.
    journal.entity_in("bob_elsewhere", "bob", "Bob");
    let refused = recover(&journal, &told, &item.id).unwrap_err();
    assert_eq!(refusal_code(&refused), "taken");
}

#[test]
fn no_owner_creation_path_reaches_the_backstop_with_a_kept_id() {
    fn kept_bob() -> Journal {
        let journal = bob_and_ann();
        let told = Told::default();
        trash(&journal, &told, "bob").unwrap();
        journal
    }

    // Review promotion picks the next free id.
    let journal = kept_bob();
    let promotion = crate::prepare_review_promotion(
        journal.root(),
        "work",
        "Person",
        "Bob",
        "",
        &[],
        false,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(promotion.identity.unwrap().entity_id, "bob_2");

    // A connected source's create-if-absent.
    let journal = kept_bob();
    let created =
        crate::create_facet_entity_if_absent(journal.root(), "work", "Person", "Bob", "").unwrap();
    assert!(matches!(
        created,
        crate::FacetEntityCreateOutcome::Created { ref entity_id, .. } if entity_id == "bob_2"
    ));

    // The owner's own create, and a model's attach.
    for owner in [true, false] {
        let journal = kept_bob();
        let attached = if owner {
            crate::attach_or_reactivate_entity_for_owner(
                journal.root(),
                "work",
                "Person",
                "Bob",
                "",
            )
        } else {
            crate::attach_or_reactivate_entity(journal.root(), "work", "Person", "Bob", "")
        }
        .unwrap();
        assert_eq!(
            attached.relationship["entity_id"], "bob_2",
            "owner: {owner}"
        );
    }

    // The owner configured as Bob.
    let journal = kept_bob();
    fs::create_dir_all(journal.root().join("config")).unwrap();
    fs::write(
        journal.root().join("config/journal.json"),
        json!({"identity": {"name": "Bob"}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        solstone_core_entity::ensure_configured_principal(journal.root()).unwrap(),
        Some("bob_2".to_owned())
    );

    // The backstop still stands for anything that tries.
    let journal = kept_bob();
    let refused = solstone_core_entity::save_entity_identity(
        journal.root(),
        "bob",
        &json!({"id": "bob", "name": "Bob", "type": "Person"}),
        None,
    )
    .unwrap_err();
    assert!(matches!(
        refused,
        solstone_core_entity::EntityWriteError::IdentityDeleted { .. }
    ));
}

#[test]
fn the_trash_view_and_preview_say_what_the_entity_holds() {
    let journal = bob_and_ann();
    let told = Told::default();
    assert_eq!(
        preview(journal.root(), "bob").unwrap(),
        Some(EntityPreview {
            entity_type: Some("Person".to_owned()),
            facets: 2,
            voice: true,
        })
    );
    assert_eq!(preview(journal.root(), "nobody").unwrap(), None);
    let item = trash(&journal, &told, "bob").unwrap();
    assert_eq!(
        trash_view(journal.root(), &item),
        EntityTrashView {
            entity_type: Some("Person".to_owned()),
            facets: 2,
            same_name: false,
        }
    );
    journal.person("bobby", "BOB");
    assert!(trash_view(journal.root(), &item).same_name);
}
