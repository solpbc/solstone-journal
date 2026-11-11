// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use solstone_core_entity::hidden_names::{HiddenNames, hidden_names_rel, read_hidden_names};

use crate::{
    FacetEntityWriteError, HiddenNamesError, create_facet, hide_detected_name_at,
    load_detected_entities_recent_at, merge_hidden_names_locked, unhide_detected_name,
};

const AT: &str = "2026-11-11T07:00:00Z";

fn journal() -> tempfile::TempDir {
    let journal = tempfile::tempdir().unwrap();
    create_facet(journal.path(), "work", "work", "", "blue", "💼", None).unwrap();
    journal
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 11, 12, 0, 0).unwrap()
}

fn detected(root: &Path, facet: &str, day: &str, names: &[&str]) {
    let path = root.join(format!("facets/{facet}/entities/{day}.jsonl"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let rows: String = names
        .iter()
        .map(|name| {
            json!({"type": "Person", "name": name, "description": "seen"}).to_string() + "\n"
        })
        .collect();
    fs::write(path, rows).unwrap();
}

fn queued(root: &Path) -> Vec<String> {
    solstone_core_indexer_todo::inspect(root).unwrap().keys
}

fn clear_queue(root: &Path) {
    let _ = fs::remove_dir_all(solstone_core_indexer_todo::root(root));
}

fn names(root: &Path, facet: &str) -> Vec<(String, String)> {
    match read_hidden_names(root, facet) {
        HiddenNames::Present(names) => names
            .into_iter()
            .map(|entry| (entry.name, entry.hidden_at))
            .collect(),
        HiddenNames::Absent => Vec::new(),
        damaged @ HiddenNames::Damaged { .. } => panic!("{damaged:?}"),
    }
}

fn listed(root: &Path) -> (Vec<String>, Vec<Value>) {
    let recent = load_detected_entities_recent_at(root, "work", 30, now()).unwrap();
    (
        recent
            .detected
            .iter()
            .map(|row| row["name"].as_str().unwrap().to_owned())
            .collect(),
        recent.hidden,
    )
}

#[test]
fn a_hide_is_written_and_queues_the_facets_detections() {
    let journal = journal();
    let root = journal.path();
    clear_queue(root);
    let write = hide_detected_name_at(root, "work", "  Bob Smith ", AT).unwrap();
    assert!(write.changed);
    assert!(write.search_queued);
    assert_eq!(
        names(root, "work"),
        vec![("Bob Smith".to_owned(), AT.to_owned())]
    );
    assert!(queued(root).iter().any(|key| key == "facets/work/entities"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.join(hidden_names_rel("work")))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn a_repeat_hide_keeps_the_first_entry_and_still_queues() {
    let journal = journal();
    let root = journal.path();
    hide_detected_name_at(root, "work", "Bob Smith", AT).unwrap();
    clear_queue(root);
    let write = hide_detected_name_at(root, "work", "BOB  smith", "2026-11-12T00:00:00Z").unwrap();
    assert!(!write.changed);
    assert!(write.search_queued);
    assert_eq!(
        names(root, "work"),
        vec![("Bob Smith".to_owned(), AT.to_owned())]
    );
    assert!(queued(root).iter().any(|key| key == "facets/work/entities"));
}

#[test]
fn an_unhide_removes_the_name_and_an_unhide_of_nothing_still_queues() {
    let journal = journal();
    let root = journal.path();
    hide_detected_name_at(root, "work", "Bob Smith", AT).unwrap();
    hide_detected_name_at(root, "work", "Alice", AT).unwrap();
    let write = unhide_detected_name(root, "work", "bob smith").unwrap();
    assert!(write.changed);
    assert_eq!(
        names(root, "work"),
        vec![("Alice".to_owned(), AT.to_owned())]
    );
    clear_queue(root);
    let write = unhide_detected_name(root, "work", "Carol").unwrap();
    assert!(!write.changed);
    assert!(write.search_queued);
    assert!(queued(root).iter().any(|key| key == "facets/work/entities"));
}

#[test]
fn a_damaged_store_refuses_hide_and_unhide_and_is_left_as_it_is() {
    let journal = journal();
    let root = journal.path();
    let path = root.join(hidden_names_rel("work"));
    fs::write(&path, b"{\"names\": [oops").unwrap();
    assert!(matches!(
        hide_detected_name_at(root, "work", "Bob", AT),
        Err(HiddenNamesError::Damaged { .. })
    ));
    assert!(matches!(
        unhide_detected_name(root, "work", "Bob"),
        Err(HiddenNamesError::Damaged { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), b"{\"names\": [oops");
    assert!(matches!(
        load_detected_entities_recent_at(root, "work", 30, now()),
        Err(FacetEntityWriteError::HiddenUndecidable(_))
    ));
}

#[test]
fn a_hide_in_an_undeclared_facet_or_of_no_name_refuses() {
    let journal = journal();
    let root = journal.path();
    assert!(matches!(
        hide_detected_name_at(root, "elsewhere", "Bob", AT),
        Err(HiddenNamesError::Facet(_))
    ));
    assert!(!root.join("facets/elsewhere").exists());
    assert!(matches!(
        hide_detected_name_at(root, "work", "  ", AT),
        Err(HiddenNamesError::EmptyName)
    ));
}

#[test]
fn a_hidden_name_leaves_the_list_comes_back_hidden_and_returns_on_unhide() {
    let journal = journal();
    let root = journal.path();
    detected(root, "work", "20261101", &["Bob Smith", "Alice"]);
    hide_detected_name_at(root, "work", "Bob Smith", AT).unwrap();

    let (shown, hidden) = listed(root);
    assert_eq!(shown, vec!["Alice"]);
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0]["name"], "Bob Smith");
    assert_eq!(hidden[0]["count"], 1);
    assert_eq!(hidden[0]["last_seen"], "20261101");

    // Detected again on a new day, under another spelling: it lands hidden.
    detected(root, "work", "20261110", &["bob smith"]);
    let (shown, hidden) = listed(root);
    assert_eq!(shown, vec!["Alice"]);
    assert_eq!(hidden[0]["count"], 2);
    assert_eq!(hidden[0]["last_seen"], "20261110");

    unhide_detected_name(root, "work", "Bob Smith").unwrap();
    let (mut shown, hidden) = listed(root);
    shown.sort();
    assert_eq!(shown, vec!["Alice", "Bob Smith", "bob smith"]);
    assert!(hidden.is_empty());
}

#[test]
fn a_name_hidden_long_ago_is_listed_without_recent_counts() {
    let journal = journal();
    let root = journal.path();
    detected(root, "work", "20260901", &["Bob Smith"]);
    hide_detected_name_at(root, "work", "Bob Smith", "2026-09-02T00:00:00Z").unwrap();
    let (shown, hidden) = listed(root);
    assert!(shown.is_empty());
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0]["name"], "Bob Smith");
    assert_eq!(hidden[0]["count"], Value::Null);
    assert_eq!(hidden[0]["last_seen"], Value::Null);
    assert_eq!(hidden[0]["type"], Value::Null);

    unhide_detected_name(root, "work", "Bob Smith").unwrap();
    assert!(listed(root).1.is_empty());
}

#[test]
fn hidden_entities_take_their_names_out_of_the_list_and_not_into_the_hidden_one() {
    let journal = journal();
    let root = journal.path();
    let entity = |id: &str, name: &str| {
        let dir = root.join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entity.json"),
            json!({"id": id, "name": name, "type": "Person", "blocked": true}).to_string(),
        )
        .unwrap();
    };
    let link = |id: &str, link: Value| {
        let dir = root.join("facets/work/entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("entity.json"), link.to_string()).unwrap();
    };
    entity("bob", "Bob Smith");
    link(
        "bob",
        json!({"entity_id": "bob", "detached": true, "detached_by": "hide"}),
    );
    entity("dana", "Dana Ruiz");
    link("dana", json!({"entity_id": "dana", "detached": true}));
    entity("eve", "Eve Stone");
    link("eve", json!({"entity_id": "eve"}));
    // A visible person detached by the owner doesn't cover their name.
    let dir = root.join("entities/frank");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("entity.json"),
        json!({"id": "frank", "name": "Frank", "type": "Person"}).to_string(),
    )
    .unwrap();
    link("frank", json!({"entity_id": "frank", "detached": true}));
    detected(
        root,
        "work",
        "20261101",
        &["Bob Smith", "Dana Ruiz", "Eve Stone", "Frank", "Alice"],
    );

    let (mut shown, hidden) = listed(root);
    shown.sort();
    assert_eq!(shown, vec!["Alice", "Frank"]);
    assert!(hidden.is_empty());
}

#[test]
fn a_merge_unions_hidden_names_keeping_the_destinations_entry() {
    let journal = journal();
    let root = journal.path();
    create_facet(root, "home", "home", "", "blue", "💼", None).unwrap();
    hide_detected_name_at(root, "work", "Bob Smith", AT).unwrap();
    hide_detected_name_at(root, "work", "Alice", AT).unwrap();
    hide_detected_name_at(root, "home", "bob smith", "2026-01-01T00:00:00Z").unwrap();
    assert!(merge_hidden_names_locked(root, "work", "home").unwrap());
    assert_eq!(
        names(root, "home"),
        vec![
            ("bob smith".to_owned(), "2026-01-01T00:00:00Z".to_owned()),
            ("Alice".to_owned(), AT.to_owned()),
        ]
    );
    assert!(!merge_hidden_names_locked(root, "work", "home").unwrap());

    fs::write(root.join(hidden_names_rel("work")), b"[").unwrap();
    let before = fs::read(root.join(hidden_names_rel("home"))).unwrap();
    assert!(merge_hidden_names_locked(root, "work", "home").is_err());
    assert_eq!(
        fs::read(root.join(hidden_names_rel("home"))).unwrap(),
        before
    );
}
