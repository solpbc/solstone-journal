// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a hide keeps out of search, and what it leaves there.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::db::{db_path, reset_index};
use crate::generation::read_last_scan;
use crate::scan::{RescanFileStatus, ScanReport, rescan_file, scan_journal};

pub(crate) struct Journal(pub(crate) PathBuf);

impl Journal {
    pub(crate) fn new(name: &str) -> Self {
        let root = crate::test_support::reserve_temp_path(&format!("solstone-hide-{name}"));
        let journal = Self(root);
        journal.write("config/journal.json", r#"{"identity":{"timezone":"UTC"}}"#);
        journal
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn write(&self, rel: &str, text: &str) {
        self.write_bytes(rel, text.as_bytes());
    }

    pub(crate) fn write_bytes(&self, rel: &str, bytes: &[u8]) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// Change a file's bytes and keep its modification time, as a store
    /// written in the same second, or an index older than its rules, sees it.
    pub(crate) fn rewrite_keeping_time(&self, rel: &str, text: &str) {
        let path = self.0.join(rel);
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, text).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    pub(crate) fn json(&self, rel: &str, value: Value) {
        self.write(rel, &value.to_string());
    }

    pub(crate) fn lines(&self, rel: &str, rows: &[Value]) {
        let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
        self.write(rel, &text);
    }

    pub(crate) fn hide_names(&self, facet: &str, names: &[&str]) {
        let names: Vec<Value> = names
            .iter()
            .map(|name| json!({"name": name, "hidden_at": "2026-11-11T07:00:00Z"}))
            .collect();
        self.json(
            &format!("facets/{facet}/hidden-names.json"),
            json!({ "names": names }),
        );
    }

    pub(crate) fn entity(&self, id: &str, name: &str, blocked: bool) {
        self.json(
            &format!("entities/{id}/entity.json"),
            json!({"id": id, "name": name, "blocked": blocked}),
        );
    }

    pub(crate) fn link(&self, facet: &str, folder: &str, link: Value) {
        self.json(
            &format!("facets/{facet}/entities/{folder}/entity.json"),
            link,
        );
    }

    pub(crate) fn notes(&self, facet: &str, folder: &str, text: &str) -> String {
        let rel = format!("facets/{facet}/entities/{folder}/observations.jsonl");
        self.lines(
            &rel,
            &[json!({"content": text, "observed_at": 1772658000000_i64, "source_day": "20260304"})],
        );
        rel
    }

    pub(crate) fn detected(&self, facet: &str, day: &str, names: &[&str]) -> String {
        let rel = format!("facets/{facet}/entities/{day}.jsonl");
        let rows: Vec<Value> = names
            .iter()
            .map(|name| json!({"type": "Person", "name": name, "description": format!("{name} was here")}))
            .collect();
        self.lines(&rel, &rows);
        rel
    }

    pub(crate) fn scan(&self, full: bool) -> ScanReport {
        scan_journal(&self.0, full).unwrap()
    }

    pub(crate) fn rescan(&self, rel: &str) -> RescanFileStatus {
        rescan_file(&self.0, Path::new(rel)).unwrap()
    }

    pub(crate) fn conn(&self) -> Connection {
        Connection::open(db_path(&self.0)).unwrap()
    }

    /// The indexed entries of a file, by position.
    pub(crate) fn rows(&self, rel: &str) -> Vec<(i64, String)> {
        let conn = self.conn();
        let mut statement = conn
            .prepare("SELECT idx, content FROM chunks WHERE path=? ORDER BY idx")
            .unwrap();
        statement
            .query_map([rel], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    pub(crate) fn has(&self, rel: &str, word: &str) -> bool {
        self.rows(rel)
            .iter()
            .any(|(_, content)| content.contains(word))
    }

    /// Every indexed entry of a journal file that holds `word`. The entity
    /// search's own rows aren't files and aren't counted.
    pub(crate) fn search(&self, word: &str) -> Vec<(String, i64)> {
        let conn = self.conn();
        let mut statement = conn
            .prepare(
                "SELECT path, idx FROM chunks WHERE chunks MATCH ? \
                 AND path NOT LIKE 'entity_search:%' ORDER BY path, idx",
            )
            .unwrap();
        statement
            .query_map([word], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    pub(crate) fn stored_mtime(&self, rel: &str) -> Option<i64> {
        use rusqlite::OptionalExtension;
        self.conn()
            .query_row("SELECT mtime FROM files WHERE path=?", [rel], |row| {
                row.get(0)
            })
            .optional()
            .unwrap()
    }

    pub(crate) fn failures_named(&self) -> Vec<String> {
        read_last_scan(&self.conn())
            .unwrap()
            .map(|receipt| {
                receipt
                    .failures
                    .into_iter()
                    .filter_map(|f| f.path)
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn activity(id: &str, title: &str, hidden: bool) -> Value {
    json!({"id": id, "title": title, "created_at": 1772658000000_i64, "hidden": hidden})
}

fn body_record(id: &str, title: &str) -> Value {
    json!({
        "id": id,
        "source": "body",
        "activity": "walk",
        "title": title,
        "created_at": 1772658000000_i64,
        "edits": [{"actor": "owner:page", "fields": ["title"], "timestamp": "2026-03-04T10:00:00Z"}],
    })
}

const ACTIVITIES: &str = "facets/work/activities/20260304.jsonl";

#[test]
fn a_hidden_activity_leaves_search_and_every_other_entry_keeps_its_position() {
    let journal = Journal::new("positions");
    journal.lines(
        ACTIVITIES,
        &[
            activity("a", "alphaneedle", false),
            activity("b", "bravoneedle", true),
            activity("c", "charlieneedle", false),
            body_record("d", "deltaneedle"),
        ],
    );
    let report = journal.scan(true);
    assert_eq!(report.failed, 0, "{report:?}");
    let positions: Vec<i64> = journal.rows(ACTIVITIES).iter().map(|row| row.0).collect();
    assert_eq!(positions, [0, 2, 3]);
    assert!(journal.search("bravoneedle").is_empty());
    assert_eq!(
        journal.search("charlieneedle"),
        [(ACTIVITIES.to_owned(), 2)]
    );
    assert_eq!(journal.search("deltaneedle"), [(ACTIVITIES.to_owned(), 3)]);

    // Unhidden, it is back at its own position and nothing else moved.
    journal.lines(
        ACTIVITIES,
        &[
            activity("a", "alphaneedle", false),
            activity("b", "bravoneedle", false),
            activity("c", "charlieneedle", false),
            body_record("d", "deltaneedle"),
        ],
    );
    journal.rescan(ACTIVITIES);
    assert_eq!(journal.search("bravoneedle"), [(ACTIVITIES.to_owned(), 1)]);
    assert_eq!(journal.search("deltaneedle"), [(ACTIVITIES.to_owned(), 3)]);
}

#[test]
fn a_hidden_body_entry_leaves_search() {
    let journal = Journal::new("hidden-body");
    let mut hidden = body_record("d", "deltaneedle");
    hidden["hidden"] = json!(true);
    journal.lines(
        ACTIVITIES,
        &[
            activity("a", "alphaneedle", false),
            hidden,
            body_record("e", "echoneedle"),
        ],
    );
    journal.scan(true);
    assert!(journal.search("deltaneedle").is_empty());
    assert_eq!(journal.search("echoneedle"), [(ACTIVITIES.to_owned(), 2)]);
}

#[test]
fn a_day_of_only_hidden_rows_is_current_on_the_next_scan() {
    let journal = Journal::new("all-hidden");
    journal.lines(
        ACTIVITIES,
        &[
            activity("a", "alphaneedle", true),
            activity("b", "bravoneedle", true),
        ],
    );
    let retired = "facets/work/activities/20260305.jsonl";
    journal.lines(
        retired,
        &[
            json!({"id": "p", "title": "plannedneedle", "source": "anticipated", "created_at": 1}),
            activity("q", "quietneedle", true),
        ],
    );
    let first = journal.scan(true);
    assert_eq!(first.failed, 0, "{first:?}");
    assert!(journal.rows(ACTIVITIES).is_empty());
    assert!(journal.rows(retired).is_empty());
    let second = journal.scan(false);
    assert_eq!(second.indexed, 0, "{second:?}");
    let third = journal.scan(true);
    assert_eq!(third.indexed, 0, "{third:?}");
}

#[test]
fn a_name_hidden_after_indexing_leaves_search_and_its_neighbours_keep_their_positions() {
    let journal = Journal::new("name-after");
    let day = journal.detected("work", "20260304", &["Bob Smith", "Alice Jones"]);
    journal.scan(true);
    assert_eq!(journal.search("Bob").len(), 1);

    journal.hide_names("work", &["bob smith"]);
    // The hide's hint re-renders the day: a forced rescan of the same bytes.
    journal.rescan(&day);
    assert!(journal.search("Bob").is_empty());
    // Alice keeps the position the Entities search reads her name back from.
    assert_eq!(journal.search("Alice"), [(day.clone(), 1)]);

    // A new day that names Bob lands hidden.
    let later = journal.detected("work", "20260310", &["Bob Smith", "Carol"]);
    journal.scan(false);
    assert!(journal.search("Bob").is_empty());
    assert_eq!(journal.search("Carol"), [(later.clone(), 1)]);
    // Another facet has hidden nothing.
    let elsewhere = journal.detected("home", "20260304", &["Bob Smith"]);
    journal.scan(false);
    assert_eq!(journal.search("Bob"), [(elsewhere, 0)]);

    // Unhidden, Bob is back where he was.
    journal.hide_names("work", &[]);
    journal.rescan(&day);
    journal.rescan(&later);
    assert!(journal.search("Bob").contains(&(day, 0)));
    assert!(journal.search("Bob").contains(&(later, 0)));
}

/// A hidden entity, linked in a facet: its notes and the detections that name
/// it leave that facet's search; a visible person beside it stays.
fn hidden_entity_journal(name: &str, link: Value, blocked: bool) -> (Journal, String, String) {
    let journal = Journal::new(name);
    journal.entity("bob", "Bob Smith", blocked);
    journal.link("work", "bob", link);
    let bob_notes = journal.notes("work", "bob", "bobnoteneedle");
    journal.entity("ann", "Ann Lee", false);
    journal.link("work", "ann", json!({"entity_id": "ann"}));
    let ann_notes = journal.notes("work", "ann", "annnoteneedle");
    journal.detected("work", "20260304", &["Bob Smith", "Alice Jones"]);
    (journal, bob_notes, ann_notes)
}

fn assert_bob_hidden(journal: &Journal, ann_notes: &str) {
    assert!(journal.search("bobnoteneedle").is_empty());
    assert!(journal.search("Bob").is_empty());
    assert_eq!(journal.search("annnoteneedle"), [(ann_notes.to_owned(), 0)]);
    assert_eq!(
        journal.search("Alice"),
        [("facets/work/entities/20260304.jsonl".to_owned(), 1)]
    );
}

#[test]
fn a_link_the_hide_marked_hides_its_notes_and_detections() {
    let (journal, _, ann_notes) = hidden_entity_journal(
        "marked",
        json!({"entity_id": "bob", "detached": true, "detached_by": "hide"}),
        true,
    );
    let report = journal.scan(true);
    assert_eq!(report.failed, 0, "{report:?}");
    assert_bob_hidden(&journal, &ann_notes);
}

#[test]
fn an_owner_detached_link_of_a_hidden_entity_hides_like_a_marked_one() {
    // Detached by the owner first and hidden after, or hidden before the hide
    // marked links: the link carries no mark, the entity says blocked.
    let (journal, _, ann_notes) = hidden_entity_journal(
        "owner-detached",
        json!({"entity_id": "bob", "detached": true}),
        true,
    );
    journal.scan(true);
    assert_bob_hidden(&journal, &ann_notes);
}

#[test]
fn an_owner_detached_link_of_a_visible_entity_stays_in_search() {
    let (journal, bob_notes, _) = hidden_entity_journal(
        "owner-detached-visible",
        json!({"entity_id": "bob", "detached": true}),
        false,
    );
    journal.scan(true);
    assert_eq!(journal.search("bobnoteneedle"), [(bob_notes, 0)]);
    assert_eq!(journal.search("Bob").len(), 1);
}

#[test]
fn a_damaged_entity_of_an_attached_link_decides_nothing_and_indexes() {
    let (journal, bob_notes, _) =
        hidden_entity_journal("attached-damaged", json!({"entity_id": "bob"}), false);
    journal.write("entities/bob/entity.json", "{ damaged");
    let report = journal.scan(true);
    assert_eq!(report.failed, 0, "{report:?}");
    assert!(journal.failures_named().is_empty());
    assert_eq!(journal.search("bobnoteneedle"), [(bob_notes, 0)]);
    assert_eq!(journal.search("Bob").len(), 1);
}

#[test]
fn a_damaged_entity_of_an_owner_detached_link_holds_its_notes_and_the_facets_detections() {
    let (journal, bob_notes, ann_notes) = hidden_entity_journal(
        "detached-damaged",
        json!({"entity_id": "bob", "detached": true}),
        false,
    );
    journal.write("entities/bob/entity.json", "{ damaged");
    let report = journal.scan(true);
    assert_eq!(report.failed, 0, "{report:?}");
    let day = "facets/work/entities/20260304.jsonl";
    assert!(journal.search("Alice").is_empty());
    assert!(journal.search("bobnoteneedle").is_empty());
    assert_eq!(journal.stored_mtime(day), Some(-1));
    assert_eq!(journal.stored_mtime(&bob_notes), Some(-1));
    let named = journal.failures_named();
    assert!(named.contains(&day.to_owned()), "{named:?}");
    assert!(named.contains(&bob_notes), "{named:?}");
    // Another folder's notes are decided on their own.
    assert_eq!(journal.search("annnoteneedle"), [(ann_notes, 0)]);
}

#[test]
fn a_damaged_entity_of_a_marked_link_hides_its_notes_and_holds_the_facets_detections() {
    let (journal, bob_notes, ann_notes) = hidden_entity_journal(
        "marked-damaged",
        json!({"entity_id": "bob", "detached": true, "detached_by": "hide"}),
        true,
    );
    journal.write("entities/bob/entity.json", "{ damaged");
    journal.scan(true);
    let day = "facets/work/entities/20260304.jsonl";
    assert!(journal.search("Alice").is_empty());
    assert_eq!(journal.stored_mtime(day), Some(-1));
    assert!(journal.search("bobnoteneedle").is_empty());
    assert!(
        journal
            .stored_mtime(&bob_notes)
            .is_some_and(|mtime| mtime > 0),
        "the notes are decided: hidden"
    );
    assert_eq!(journal.search("annnoteneedle"), [(ann_notes, 0)]);
}

#[test]
fn an_unreadable_link_holds_its_notes_and_the_facets_detections_only() {
    let (journal, _, ann_notes) =
        hidden_entity_journal("link-damaged", json!({"entity_id": "bob"}), false);
    journal.write("facets/work/entities/bob/entity.json", "{ damaged");
    journal.scan(true);
    let day = "facets/work/entities/20260304.jsonl";
    assert_eq!(journal.stored_mtime(day), Some(-1));
    assert_eq!(
        journal.stored_mtime("facets/work/entities/bob/observations.jsonl"),
        Some(-1)
    );
    assert_eq!(journal.search("annnoteneedle"), [(ann_notes, 0)]);
}

#[test]
fn a_damaged_hidden_names_file_holds_the_facets_detections_until_it_reads_again() {
    let journal = Journal::new("names-damaged");
    let day = journal.detected("work", "20260304", &["Bob Smith", "Alice Jones"]);
    let notes = journal.notes("work", "ann", "annnoteneedle");
    let other = journal.detected("home", "20260304", &["Dana"]);
    journal.write("facets/work/hidden-names.json", "{ damaged");
    let report = journal.scan(true);
    assert_eq!(report.failed, 0, "{report:?}");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("hidden-names.json")),
        "{report:?}"
    );
    assert!(journal.search("Alice").is_empty());
    assert_eq!(journal.stored_mtime(&day), Some(-1));
    assert_eq!(journal.failures_named(), [day.clone()]);
    // The build still completes: an undecided file is named, not counted.
    let state = crate::db::read_index_build_state(&journal.conn())
        .unwrap()
        .unwrap();
    assert_eq!(state.state, crate::db::IndexBuildLifecycle::Complete);
    // The notes and another facet's detections don't depend on it.
    assert_eq!(journal.search("annnoteneedle"), [(notes, 0)]);
    assert_eq!(journal.search("Dana"), [(other, 0)]);

    // Every scan tries again; once it reads, the day indexes without being
    // written.
    let day_bytes = fs::read(journal.path().join(&day)).unwrap();
    journal.scan(false);
    assert_eq!(journal.stored_mtime(&day), Some(-1));
    journal.hide_names("work", &["Bob Smith"]);
    let repaired = journal.scan(false);
    assert_eq!(repaired.failed, 0, "{repaired:?}");
    assert!(journal.stored_mtime(&day).is_some_and(|mtime| mtime > 0));
    assert_eq!(journal.search("Alice"), [(day.clone(), 1)]);
    assert!(journal.search("Bob").is_empty());
    assert_eq!(fs::read(journal.path().join(&day)).unwrap(), day_bytes);
    assert!(journal.failures_named().is_empty());
}

#[test]
fn a_hidden_day_made_undecidable_before_its_hint_keeps_nothing_and_is_retried() {
    let journal = Journal::new("hidden-then-damaged");
    let day = journal.detected("work", "20260304", &["Bob Smith", "Alice Jones"]);
    journal.scan(true);
    journal.hide_names("work", &["Bob Smith"]);
    journal.write("facets/work/hidden-names.json", "{ damaged");
    let RescanFileStatus::Indexed { warnings } = journal.rescan(&day) else {
        panic!("declined");
    };
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("hidden-names.json")),
        "{warnings:?}"
    );
    assert!(journal.rows(&day).is_empty());
    assert_eq!(journal.stored_mtime(&day), Some(-1));
    // The next scan retries it.
    journal.hide_names("work", &["Bob Smith"]);
    journal.scan(false);
    assert_eq!(journal.search("Alice"), [(day, 1)]);
    assert!(journal.search("Bob").is_empty());
}

#[test]
fn the_outputs_of_a_hidden_activity_leave_search() {
    let journal = Journal::new("outputs");
    let mut cancelled = activity("b", "bravoneedle", true);
    cancelled["hidden_reason"] = json!("cancelled");
    journal.lines(
        ACTIVITIES,
        &[activity("a", "alphaneedle", false), cancelled],
    );
    let shown = "facets/work/activities/20260304/a/story.md";
    let muted = "facets/work/activities/20260304/b/story.md";
    let orphan = "facets/work/activities/20260304/z/story.md";
    journal.write(shown, "# Story\n\nshownoutputneedle\n");
    journal.write(muted, "# Story\n\nmutedoutputneedle\n");
    journal.write(orphan, "# Story\n\norphanoutputneedle\n");
    let report = journal.scan(true);
    assert_eq!(report.failed, 0, "{report:?}");
    assert!(journal.search("mutedoutputneedle").is_empty());
    assert_eq!(journal.search("shownoutputneedle").len(), 1);
    assert_eq!(journal.search("orphanoutputneedle").len(), 1);

    // Unhidden, its outputs come back when its folder's hint is served.
    journal.lines(
        ACTIVITIES,
        &[
            activity("a", "alphaneedle", false),
            activity("b", "bravoneedle", false),
        ],
    );
    journal.rescan(muted);
    assert_eq!(journal.search("mutedoutputneedle").len(), 1);
}

#[test]
fn an_unreadable_day_holds_the_outputs_it_might_hide() {
    let journal = Journal::new("outputs-unreadable");
    let output = "facets/work/activities/20260304/b/story.md";
    journal.write(output, "# Story\n\nmutedoutputneedle\n");
    journal.write_bytes(ACTIVITIES, b"{\"id\":\"b\",\"hidden\":tr\xff}\n");
    journal.scan(true);
    assert!(journal.search("mutedoutputneedle").is_empty());
    assert_eq!(journal.stored_mtime(output), Some(-1));
    assert!(journal.failures_named().contains(&output.to_owned()));

    // A line that isn't a record could be any id's row; a readable row decides.
    journal.write(
        ACTIVITIES,
        &format!("not a record\n{}\n", activity("a", "alphaneedle", false)),
    );
    let shown = "facets/work/activities/20260304/a/story.md";
    journal.write(shown, "# Story\n\nshownoutputneedle\n");
    journal.scan(false);
    assert_eq!(journal.stored_mtime(output), Some(-1));
    assert_eq!(journal.search("shownoutputneedle").len(), 1);

    journal.lines(ACTIVITIES, &[activity("b", "bravoneedle", false)]);
    journal.scan(false);
    assert_eq!(journal.search("mutedoutputneedle").len(), 1);
}

const WEEK: &str = "reflections/weekly/20260301.md";

fn week_journal(name: &str) -> Journal {
    let journal = Journal::new(name);
    journal.write(WEEK, "# Your week\n\nkestrelmemory came back on monday.\n");
    journal.json(
        "reflections/weekly/20260301.json",
        json!({
            "memories": [{"key": "m1", "day": "20260302", "text": "kestrelmemory came back"}],
            "said": [{"key": "s1", "quote": "a quote"}],
        }),
    );
    journal
}

#[test]
fn a_week_page_with_anything_left_out_leaves_search_whole() {
    let journal = week_journal("week");
    journal.scan(true);
    assert_eq!(journal.search("kestrelmemory").len(), 1);

    // A key of another week leaves this page as it is.
    journal.json("config/week-left-out.json", json!({"keys": ["elsewhere"]}));
    journal.rescan(WEEK);
    assert_eq!(journal.search("kestrelmemory").len(), 1);

    journal.json("config/week-left-out.json", json!({"keys": ["s1"]}));
    journal.rescan(WEEK);
    assert!(journal.rows(WEEK).is_empty());
    assert!(journal.stored_mtime(WEEK).is_some_and(|mtime| mtime > 0));

    // Undo puts the page back.
    journal.json("config/week-left-out.json", json!({"keys": []}));
    journal.rescan(WEEK);
    assert_eq!(journal.search("kestrelmemory").len(), 1);
}

#[test]
fn a_week_page_waits_while_what_is_left_out_of_it_cant_be_read() {
    let journal = week_journal("week-undecided");
    journal.write("config/week-left-out.json", "{ damaged");
    journal.scan(true);
    assert!(journal.rows(WEEK).is_empty());
    assert_eq!(journal.stored_mtime(WEEK), Some(-1));

    // Something is left out and the page's own keys can't be read.
    journal.json("config/week-left-out.json", json!({"keys": ["m1"]}));
    journal.write("reflections/weekly/20260301.json", "{ damaged");
    journal.scan(false);
    assert_eq!(journal.stored_mtime(WEEK), Some(-1));
    fs::remove_file(journal.path().join("reflections/weekly/20260301.json")).unwrap();
    journal.scan(false);
    assert_eq!(journal.stored_mtime(WEEK), Some(-1));

    // Nothing left out: the page needs no keys.
    fs::remove_file(journal.path().join("config/week-left-out.json")).unwrap();
    journal.scan(false);
    assert_eq!(journal.search("kestrelmemory").len(), 1);
}

/// An index from before hides were left out of search: the hidden things are
/// in it, under the times of files that will not change again.
fn plant_an_older_index(journal: &Journal) {
    let conn = journal.conn();
    // Each indexed activity file's proof names the bytes now on disk, as an
    // older build that indexed hidden records left it.
    let digest = crate::activity_provenance::digest_bytes(
        &fs::read(journal.path().join(ACTIVITIES)).unwrap(),
    );
    conn.execute(
        "UPDATE activity_provenance SET digest=? WHERE rowid IN \
         (SELECT rowid FROM chunk_sources WHERE path=?)",
        rusqlite::params![digest.as_slice(), ACTIVITIES],
    )
    .unwrap();
    conn.execute("DELETE FROM index_meta WHERE key='hides_left_out'", [])
        .unwrap();
}

fn upgrade_journal(name: &str) -> Journal {
    let journal = Journal::new(name);
    let people = json!([
        {"role": "attendee", "entity_id": "edge_zoe"},
        {"role": "attendee", "entity_id": "edge_ada"}
    ]);
    let mut a = activity("a", "alphaneedle", false);
    a["participation"] = people.clone();
    let mut b = activity("b", "bravoneedle", false);
    b["participation"] = people;
    journal.lines(ACTIVITIES, &[a, b]);
    journal.write(
        "facets/work/activities/20260304/a/story.md",
        "# Story\n\nshownoutputneedle\n",
    );
    journal.write(
        "facets/work/activities/20260304/b/story.md",
        "# Story\n\nmutedoutputneedle\n",
    );
    journal.entity("bob", "Bob Smith", false);
    journal.link("work", "bob", json!({"entity_id": "bob"}));
    journal.notes("work", "bob", "bobnoteneedle");
    journal.entity("ann", "Ann Lee", false);
    journal.link("work", "ann", json!({"entity_id": "ann"}));
    journal.notes("work", "ann", "annnoteneedle");
    journal.detected("work", "20260304", &["Bob Smith", "Alice Jones"]);
    let report = journal.scan(true);
    assert_eq!(report.failed, 0, "{report:?}");
    crate::scan::rebuild_edges(journal.path()).unwrap();
    assert_eq!(journal.search("bravoneedle").len(), 1);
    journal
}

/// Hide the activity and the person the way an older build left them: the
/// stores say hidden, and no file the index has a time for changed.
fn hide_under_an_older_index(journal: &Journal) {
    let mut a = activity("a", "alphaneedle", false);
    let mut b = activity("b", "bravoneedle", true);
    let people = json!([
        {"role": "attendee", "entity_id": "edge_zoe"},
        {"role": "attendee", "entity_id": "edge_ada"}
    ]);
    a["participation"] = people.clone();
    b["participation"] = people;
    let text = format!("{a}\n{b}\n");
    journal.rewrite_keeping_time(ACTIVITIES, &text);
    journal.entity("bob", "Bob Smith", true);
    journal.link(
        "work",
        "bob",
        json!({"entity_id": "bob", "detached": true, "detached_by": "hide"}),
    );
    plant_an_older_index(journal);
}

fn edge_anchors(journal: &Journal) -> Vec<String> {
    let conn = journal.conn();
    let mut statement = conn
        .prepare("SELECT DISTINCT anchor FROM edges WHERE path=? ORDER BY anchor")
        .unwrap();
    statement
        .query_map([ACTIVITIES], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn the_first_scan_after_upgrade_drops_what_older_hides_left_in_search() {
    let journal = upgrade_journal("upgrade");
    assert_eq!(edge_anchors(&journal), ["a", "b"]);
    hide_under_an_older_index(&journal);

    let report = journal.scan(false);
    assert_eq!(report.failed, 0, "{report:?}");
    crate::scan::rebuild_edges(journal.path()).unwrap();
    assert!(journal.search("bravoneedle").is_empty());
    assert!(journal.search("mutedoutputneedle").is_empty());
    assert!(journal.search("bobnoteneedle").is_empty());
    assert!(journal.search("Bob").is_empty());
    assert_eq!(edge_anchors(&journal), ["a"]);
    // Visible siblings stay.
    assert_eq!(journal.search("alphaneedle").len(), 1);
    assert_eq!(journal.search("shownoutputneedle").len(), 1);
    assert_eq!(journal.search("annnoteneedle").len(), 1);
    assert_eq!(journal.search("Alice").len(), 1);

    // Once only: a later scan reindexes nothing.
    assert_eq!(journal.scan(false).indexed, 0);
}

#[test]
fn without_the_upgrade_mark_an_older_index_keeps_what_it_held() {
    // The positive control: the mark, not the scan, is what drops them.
    let journal = upgrade_journal("upgrade-control");
    hide_under_an_older_index(&journal);
    journal
        .conn()
        .execute(
            "REPLACE INTO index_meta(key, value) VALUES ('hides_left_out', '1')",
            [],
        )
        .unwrap();
    journal.scan(false);
    assert_eq!(journal.search("mutedoutputneedle").len(), 1);
    assert_eq!(journal.search("bobnoteneedle").len(), 1);
}

/// A journal holding one of each hide.
fn hide_everything(journal: &Journal) {
    journal.lines(
        ACTIVITIES,
        &[
            activity("a", "alphaneedle", false),
            activity("b", "bravoneedle", true),
        ],
    );
    journal.entity("bob", "Bob Smith", true);
    journal.link(
        "work",
        "bob",
        json!({"entity_id": "bob", "detached": true, "detached_by": "hide"}),
    );
    journal.hide_names("work", &["Carol"]);
    journal.json("config/week-left-out.json", json!({"keys": ["m1"]}));
}

fn restore_journal(name: &str) -> Journal {
    let journal = week_journal(name);
    journal.lines(
        ACTIVITIES,
        &[
            activity("a", "alphaneedle", false),
            activity("b", "bravoneedle", false),
        ],
    );
    journal.entity("bob", "Bob Smith", false);
    journal.link("work", "bob", json!({"entity_id": "bob"}));
    journal.notes("work", "bob", "bobnoteneedle");
    journal.detected("work", "20260304", &["Bob Smith", "Carol", "Alice Jones"]);
    journal
}

fn assert_every_hide_out_of_search(journal: &Journal) {
    for hidden in [
        "bravoneedle",
        "bobnoteneedle",
        "Bob",
        "Carol",
        "kestrelmemory",
    ] {
        assert!(journal.search(hidden).is_empty(), "{hidden}");
    }
    for visible in ["alphaneedle", "Alice"] {
        assert_eq!(journal.search(visible).len(), 1, "{visible}");
    }
}

#[test]
fn an_index_rebuilt_from_a_journal_with_every_hide_leaves_each_out() {
    let journal = restore_journal("restore");
    hide_everything(&journal);
    reset_index(journal.path()).unwrap();
    let report = journal.scan(true);
    assert_eq!(report.failed, 0, "{report:?}");
    assert_every_hide_out_of_search(&journal);
}

#[test]
fn an_index_rebuilt_after_hides_made_since_it_was_built_leaves_each_out() {
    let journal = restore_journal("restore-after");
    journal.scan(true);
    for visible in ["bravoneedle", "bobnoteneedle", "Carol", "kestrelmemory"] {
        assert_eq!(journal.search(visible).len(), 1, "{visible}");
    }
    hide_everything(&journal);
    // A whole-journal restore starts the index from empty.
    reset_index(journal.path()).unwrap();
    journal.scan(true);
    assert_every_hide_out_of_search(&journal);
}
