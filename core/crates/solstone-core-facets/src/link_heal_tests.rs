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

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_removals::{Action, Surface, Unit};

use super::*;

struct Journal(tempfile::TempDir);

impl Journal {
    fn new() -> Self {
        let journal = Self(tempfile::TempDir::new().unwrap());
        journal.write("config/journal.json", b"{}");
        fs::create_dir_all(journal.root().join("entities")).unwrap();
        for facet in ["work", "home"] {
            crate::create_facet(journal.root(), facet, facet, "", "blue", "💼", None).unwrap();
        }
        journal
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn entity(&self, id: &str, fields: Value) {
        let mut value = json!({"id": id, "name": id, "type": "Person"});
        for (key, field) in fields.as_object().unwrap() {
            value[key] = field.clone();
        }
        self.write(
            &format!("entities/{id}/entity.json"),
            value.to_string().as_bytes(),
        );
    }

    fn link(&self, facet: &str, folder: &str, link: Value) {
        self.write(
            &format!("facets/{facet}/entities/{folder}/entity.json"),
            link.to_string().as_bytes(),
        );
    }

    fn note(&self, facet: &str, folder: &str, content: &str) {
        self.write(
            &format!("facets/{facet}/entities/{folder}/observations.jsonl"),
            format!("{}\n", json!({"content": content})).as_bytes(),
        );
    }

    fn read_link(&self, facet: &str, folder: &str) -> Value {
        serde_json::from_slice(
            &fs::read(
                self.root()
                    .join(format!("facets/{facet}/entities/{folder}/entity.json")),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn exists(&self, rel: &str) -> bool {
        self.root().join(rel).exists()
    }

    fn facet_id(&self, facet: &str) -> String {
        let value: Value = serde_json::from_slice(
            &fs::read(self.root().join(format!("facets/{facet}/facet.json"))).unwrap(),
        )
        .unwrap();
        value["id"].as_str().unwrap().to_owned()
    }

    /// A facet merge folder holding a declaration with `id` and one file.
    fn leftover(&self, name: &str, id: &str) {
        self.write(
            &format!("facets/{name}/facet.json"),
            json!({"id": id, "title": "Work"}).to_string().as_bytes(),
        );
        self.write(&format!("facets/{name}/news/20260801.md"), b"only here");
    }

    /// Every file under `facets/` and `entities/`, by path.
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
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        for top in ["facets", "entities"] {
            walk(self.root(), &self.root().join(top), &mut out);
        }
        out
    }

    fn subtree(&self, prefix: &str) -> BTreeMap<String, Vec<u8>> {
        self.tree()
            .into_iter()
            .filter(|(path, _)| path.starts_with(prefix))
            .collect()
    }

    fn heal(&self) -> FacetLinkHealReport {
        let report = self_heal_facet_links(self.root()).unwrap();
        assert!(report.failures.is_empty(), "{report:?}");
        report
    }
}

fn trashed_paths(root: &Path) -> Vec<String> {
    solstone_core_trash::list(root)
        .unwrap()
        .into_iter()
        .flat_map(|item| item.paths)
        .collect()
}

#[test]
fn only_a_live_facets_merge_copy_goes_to_the_trash() {
    let journal = Journal::new();
    let work = journal.facet_id("work");
    journal.leftover(".facet-merge-a.stage", &work);
    journal.leftover(".facet-merge-a.source", &journal.facet_id("home"));
    journal.leftover(".facet-merge-a.dropped", &work);
    // A `.dest` whose facet is no longer live may be its only copy.
    journal.leftover(".facet-merge-b.dest", "f_gone_for_good");
    let source = journal.subtree("facets/.facet-merge-a.source");
    let dropped = journal.subtree("facets/.facet-merge-a.dropped");
    let orphan = journal.subtree("facets/.facet-merge-b.dest");

    let report = journal.heal();

    assert_eq!(report.merge_copies_trashed, 1);
    assert_eq!(report.merge_leftovers_kept, 3);
    assert!(!journal.exists("facets/.facet-merge-a.stage"));
    assert_eq!(
        trashed_paths(journal.root()),
        vec!["facets/.facet-merge-a.stage"]
    );
    assert_eq!(journal.subtree("facets/.facet-merge-a.source"), source);
    assert_eq!(journal.subtree("facets/.facet-merge-a.dropped"), dropped);
    assert_eq!(journal.subtree("facets/.facet-merge-b.dest"), orphan);
}

#[test]
fn links_to_a_merged_entity_join_its_live_successor() {
    let journal = Journal::new();
    journal.entity("carol", json!({}));
    journal.write(
        "entities/retired.json",
        json!({"ids": {"old_carol": {"state": "merged", "dir": "old_carol", "successor": "carol"}}})
            .to_string()
            .as_bytes(),
    );
    journal.link("work", "old_carol", json!({"entity_id": "old_carol"}));
    journal.note("work", "old_carol", "met at the market");

    let report = journal.heal();

    assert_eq!(report.merged_links_folded, 1);
    assert!(!journal.exists("facets/work/entities/old_carol"));
    assert_eq!(journal.read_link("work", "carol")["entity_id"], "carol");
    let notes = fs::read_to_string(
        journal
            .root()
            .join("facets/work/entities/carol/observations.jsonl"),
    )
    .unwrap();
    assert!(notes.contains("met at the market"), "{notes}");
}

#[test]
fn links_to_an_entity_waiting_in_the_trash_stay_for_its_recovery() {
    let journal = Journal::new();
    journal.entity("dana", json!({}));
    solstone_core_entity::rewrite_identity_map_cache(journal.root()).unwrap();
    let index = solstone_core_indexer_store::RetentionIndex::new(journal.root());
    crate::entity_trash::Entities::new(&index)
        .trash(journal.root(), "dana", Action::EntityDelete, Surface::Cli)
        .unwrap();
    assert!(!journal.exists("entities/dana"));
    // A facet that was itself in the trash when dana went comes back with
    // its link: dana is recorded deleted, but can still be recovered.
    journal.link("home", "dana", json!({"entity_id": "dana"}));
    let before = journal.subtree("facets/home/entities/dana");

    let report = journal.heal();

    assert_eq!(report.absent_links_removed, 0);
    assert_eq!(journal.subtree("facets/home/entities/dana"), before);
}

#[test]
fn a_bare_link_to_an_entity_deleted_for_good_goes_and_one_with_notes_stays() {
    let journal = Journal::new();
    solstone_core_entity::record_deleted_entity(journal.root(), "erin", "erin").unwrap();
    journal.link("work", "erin", json!({"entity_id": "erin"}));
    journal.link("home", "erin", json!({"entity_id": "erin"}));
    journal.note("home", "erin", "the owner's own words");
    let with_notes = journal.subtree("facets/home/entities/erin");

    let report = journal.heal();

    assert_eq!(report.absent_links_removed, 1);
    assert_eq!(report.absent_links_kept, 1);
    assert!(!journal.exists("facets/work/entities/erin"));
    assert_eq!(journal.subtree("facets/home/entities/erin"), with_notes);
}

#[test]
fn a_link_to_an_id_never_recorded_deleted_stays() {
    let journal = Journal::new();
    journal.link("work", "frank", json!({"entity_id": "frank"}));
    let before = journal.subtree("facets/work/entities/frank");

    journal.heal();

    assert_eq!(journal.subtree("facets/work/entities/frank"), before);
}

#[test]
fn a_hidden_entity_detaches_only_its_still_attached_links() {
    let journal = Journal::new();
    journal.entity("gail", json!({"blocked": true}));
    // A hide that stopped after the entity was marked hidden.
    journal.link(
        "work",
        "gail",
        json!({"entity_id": "gail", "role": "attached"}),
    );
    // Detached by the owner before the hide, and by an earlier hide.
    journal.link(
        "home",
        "gail",
        json!({"entity_id": "gail", "detached": true, "role": "owner"}),
    );
    let owner_detached = journal.subtree("facets/home/entities/gail");

    let report = journal.heal();

    assert_eq!(report.hidden_links_detached, 1);
    let work = journal.read_link("work", "gail");
    assert_eq!(work["detached"], true);
    assert_eq!(work["detached_by"], "hide");
    assert_eq!(work["role"], "attached");
    assert_eq!(journal.subtree("facets/home/entities/gail"), owner_detached);

    // And unhiding puts back exactly the one hiding took.
    let unhide = crate::unblock_journal_entity(journal.root(), "gail").unwrap();
    assert_eq!(unhide.facets_reattached, vec!["work"]);
    assert_eq!(journal.read_link("home", "gail")["detached"], true);
}

#[test]
fn a_visible_entitys_detached_links_are_never_reattached() {
    let journal = Journal::new();
    journal.entity("hal", json!({}));
    journal.link(
        "work",
        "hal",
        json!({"entity_id": "hal", "detached": true, "detached_by": "hide"}),
    );
    journal.link("home", "hal", json!({"entity_id": "hal", "detached": true}));
    let before = journal.tree();

    let report = journal.heal();

    assert_eq!(report.hidden_links_detached, 0);
    assert_eq!(journal.tree(), before);
}

#[test]
fn an_undeclared_facet_folder_is_left_entirely_alone() {
    let journal = Journal::new();
    journal.entity("carol", json!({}));
    journal.entity("gail", json!({"blocked": true}));
    journal.write(
        "entities/retired.json",
        json!({"ids": {"old_carol": {"state": "merged", "dir": "old_carol", "successor": "carol"}}})
            .to_string()
            .as_bytes(),
    );
    solstone_core_entity::record_deleted_entity(journal.root(), "erin", "erin").unwrap();
    journal.link("personal", "old_carol", json!({"entity_id": "old_carol"}));
    journal.link("personal", "erin", json!({"entity_id": "erin"}));
    journal.link("personal", "gail", json!({"entity_id": "gail"}));
    journal.link("personal", "carol_two", json!({"entity_id": "carol"}));
    assert!(!journal.exists("facets/personal/facet.json"));
    let before = journal.subtree("facets/personal");

    let report = journal.heal();

    assert_eq!(report, FacetLinkHealReport::default());
    assert_eq!(journal.subtree("facets/personal"), before);
}

#[test]
fn a_second_pass_changes_nothing() {
    let journal = Journal::new();
    let work = journal.facet_id("work");
    journal.leftover(".facet-merge-a.preview", &work);
    journal.leftover(".facet-merge-a.source", "f_other");
    journal.entity("carol", json!({}));
    journal.entity("gail", json!({"blocked": true}));
    journal.write(
        "entities/retired.json",
        json!({"ids": {"old_carol": {"state": "merged", "dir": "old_carol", "successor": "carol"}}})
            .to_string()
            .as_bytes(),
    );
    solstone_core_entity::record_deleted_entity(journal.root(), "erin", "erin").unwrap();
    journal.link("work", "old_carol", json!({"entity_id": "old_carol"}));
    journal.link("work", "erin", json!({"entity_id": "erin"}));
    journal.link("home", "erin", json!({"entity_id": "erin"}));
    journal.note("home", "erin", "kept");
    journal.link("work", "gail", json!({"entity_id": "gail"}));

    let first = journal.heal();
    assert_eq!(first.merge_copies_trashed, 1);
    assert_eq!(first.merged_links_folded, 1);
    assert_eq!(first.absent_links_removed, 1);
    assert_eq!(first.hidden_links_detached, 1);
    let after = journal.tree();
    let trashed = trashed_paths(journal.root());

    let second = journal.heal();

    assert_eq!(
        second,
        FacetLinkHealReport {
            merge_leftovers_kept: 1,
            absent_links_kept: 1,
            ..FacetLinkHealReport::default()
        }
    );
    assert_eq!(journal.tree(), after);
    assert_eq!(trashed_paths(journal.root()), trashed);
    assert!(
        solstone_core_trash::list(journal.root())
            .unwrap()
            .iter()
            .all(|item| item.unit == Unit::FacetFile)
    );
}
