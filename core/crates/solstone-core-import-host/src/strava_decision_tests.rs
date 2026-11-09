// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

use super::*;

const NOW: &str = "2026-11-09T00:00:00Z";
const LATER: &str = "2026-11-09T01:00:00Z";

fn line(facet: &str, record: Value) -> SenseLine {
    let record = record.as_object().unwrap().clone();
    SenseLine {
        day: "20260101".to_owned(),
        facet: facet.to_owned(),
        id: record["id"].as_str().unwrap().to_owned(),
        record,
    }
}

fn base() -> Record {
    json!({"id": "body_x", "source": "body", "activity": "workout"})
        .as_object()
        .unwrap()
        .clone()
}

fn edit(at: &str, actor: &str, fields: &[&str], set: Value) -> Value {
    json!({"timestamp": at, "actor": actor, "fields": fields, "note": "", "set": set})
}

fn hide(at: &str, actor: &str) -> Value {
    json!({"timestamp": at, "actor": actor, "fields": ["hidden"], "note": "muted"})
}

fn fold(base: Record, lines: &[&SenseLine], primary: &str) -> Record {
    decide(base, lines, &|_| true, primary, NOW)
}

#[test]
fn a_changed_line_adds_whats_new_and_never_brings_back_what_the_owner_removed() {
    let at = "2026-01-02T00:00:00Z";
    let first = line(
        "a",
        json!({"id": "s", "title": "Old", "edits": [
            edit(at, "owner:update", &["title"], json!({"title": "Old"}))]}),
    );
    let mut record = fold(base(), &[&first], &first.key());
    assert_eq!(record["title"], json!("Old"));
    // The owner removes the carried edit.
    let edits = record.get_mut("edits").unwrap().as_array_mut().unwrap();
    edits[0].as_object_mut().unwrap().insert(
        solstone_core_format::content::EDIT_REMOVED.to_owned(),
        json!(LATER),
    );
    let without = fold(record.clone(), &[&first], &first.key());
    assert!(without.get("title").is_none());

    // The line changes without the owner touching it: the store rewrote it.
    let changed = line(
        "a",
        json!({"id": "s", "title": "Old", "rewritten": true, "edits": [
            edit(at, "owner:update", &["title"], json!({"title": "Old"})),
            edit(LATER, "owner:update", &["description"], json!({"description": "New"}))]}),
    );
    let after = fold(without, &[&changed], &changed.key());
    assert!(after.get("title").is_none(), "{after:?}");
    assert_eq!(after["description"], json!("New"));
}

#[test]
fn the_hide_note_the_owner_wrote_is_carried() {
    let hidden = line(
        "a",
        json!({"id": "s", "hidden": true, "edits": [hide("2026-01-02T00:00:00Z", "owner:hide")]}),
    );
    let record = fold(base(), &[&hidden], &hidden.key());
    let decision = record["edits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edit| edit["carried"] == json!("decision"))
        .unwrap();
    assert_eq!(decision["note"], json!("muted"));
    assert_eq!(record["hidden"], json!(true));
}

#[test]
fn a_record_that_was_visible_counts_as_visible_at_every_later_decision() {
    let hidden = line(
        "a",
        json!({"id": "s", "hidden": true, "edits": [hide("2026-01-02T00:00:00Z", "owner:hide")]}),
    );
    let mut seeded = base();
    seeded.insert(CONVERTED.to_owned(), json!({RECORD_VISIBLE: true}));
    let record = fold(seeded, &[&hidden], &hidden.key());
    assert_eq!(record["hidden"], json!(false));
    assert_eq!(record[CONVERTED][RECORD_VISIBLE], json!(true));
    let again = fold(record, &[&hidden], &hidden.key());
    assert_eq!(again["hidden"], json!(false));
}

fn key(facet: &str) -> String {
    format!("20260101|{facet}|exercise_070000_300")
}

const SENSE_ID: &str = "exercise_070000_300";

fn visible_with_title() -> SenseLine {
    line(
        "a",
        json!({"id": SENSE_ID, "title": "Run", "edits": [
            edit("2026-01-02T00:00:00Z", "owner:update", &["title"], json!({"title": "Run"}))]}),
    )
}

fn hidden_by_owner() -> SenseLine {
    line(
        "b",
        json!({"id": SENSE_ID, "hidden": true,
            "edits": [hide("2026-01-03T00:00:00Z", "owner:hide")]}),
    )
}

#[test]
fn an_owner_title_carries_and_is_held_and_removable() {
    let a = visible_with_title();
    let folded = fold(base(), &[&a], &key("a"));
    assert_eq!(folded["title"], "Run");
    assert!(solstone_core_format::content::activity_field_held(
        &folded, "title"
    ));
    let carried = &folded["edits"][0];
    assert_eq!(carried["before"], json!({}));
    assert_eq!(carried["carried"], key("a"));
    assert_eq!(carried["actor"], "owner:update");
}

#[test]
fn a_command_line_edit_carries_and_senses_own_words_do_not() {
    let a = line(
        "a",
        json!({"id": SENSE_ID, "description": "model words", "title": "Trail",
            "edits": [edit("2026-01-02T00:00:00Z", "cli:update", &["title"], json!({"title": "Trail"}))]}),
    );
    let folded = fold(base(), &[&a], &key("a"));
    assert_eq!(folded["title"], "Trail");
    assert!(!folded.contains_key("description"));
}

#[test]
fn an_edit_with_no_recorded_value_carries_what_the_line_says() {
    let a = line(
        "a",
        json!({"id": SENSE_ID, "title": "Older",
            "edits": [{"timestamp": "2026-01-02T00:00:00Z", "actor": "owner:update",
                "fields": ["title"], "note": ""}]}),
    );
    let folded = fold(base(), &[&a], &key("a"));
    assert_eq!(folded["title"], "Older");
    assert_eq!(folded["edits"][0]["set"], json!({"title": "Older"}));
}

#[test]
fn a_removed_edit_carries_nothing() {
    let a = line(
        "a",
        json!({"id": SENSE_ID, "edits": [{"timestamp": "t", "actor": "owner:update",
            "fields": ["title"], "note": "", "set": {"title": "Gone"}, "removed": "x"}]}),
    );
    let folded = fold(base(), &[&a], &key("a"));
    assert!(!folded.contains_key("title"));
}

#[test]
fn the_primary_wins_a_field_and_a_later_edit_elsewhere_stays_in_the_history() {
    let a = visible_with_title();
    let b = line(
        "b",
        json!({"id": SENSE_ID, "title": "Newer", "edits": [
            edit("2026-05-01T00:00:00Z", "owner:update", &["title"], json!({"title": "Newer"}))]}),
    );
    let folded = fold(base(), &[&a, &b], &key("a"));
    assert_eq!(folded["title"], "Run");
    let edits = folded["edits"].as_array().unwrap();
    assert_eq!(edits[0]["carried"], key("b"));
    assert_eq!(edits[1]["carried"], key("a"));
}

#[test]
fn with_no_primary_value_the_newest_carried_value_shows() {
    let a = line("a", json!({"id": SENSE_ID}));
    let b = line(
        "b",
        json!({"id": SENSE_ID, "title": "Old", "edits": [
            edit("2026-01-01T00:00:00Z", "owner:update", &["title"], json!({"title": "Old"}))]}),
    );
    let c = line(
        "c",
        json!({"id": SENSE_ID, "title": "New", "edits": [
            edit("2026-06-01T00:00:00Z", "owner:update", &["title"], json!({"title": "New"}))]}),
    );
    let folded = fold(base(), &[&a, &b, &c], &key("a"));
    assert_eq!(folded["title"], "New");
}

#[test]
fn hidden_in_one_of_two_is_visible_and_hidden_in_both_is_hidden_as_the_owner_hid_it() {
    let a = visible_with_title();
    let b = hidden_by_owner();
    let folded = fold(base(), &[&a, &b], &key("a"));
    assert_eq!(folded["hidden"], false);
    let decision = folded["edits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edit| edit["carried"] == DECISION)
        .unwrap();
    assert_eq!(decision["actor"], "convert");

    let both = line(
        "a",
        json!({"id": SENSE_ID, "hidden": true,
            "edits": [hide("2026-01-05T00:00:00Z", "cli:hide")]}),
    );
    let folded = fold(base(), &[&both, &b], &key("a"));
    assert_eq!(folded["hidden"], true);
    let decision = folded["edits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edit| edit["carried"] == DECISION)
        .unwrap();
    assert_eq!(decision["actor"], "cli:hide");
    assert_eq!(decision["fields"], json!(["hidden"]));
}

#[test]
fn hidden_by_the_system_counts_as_visible() {
    let a = line(
        "a",
        json!({"id": SENSE_ID, "hidden": true,
            "edits": [hide("2026-01-05T00:00:00Z", "system:sweep")]}),
    );
    assert_eq!(fold(base(), &[&a], &key("a"))["hidden"], false);
    let no_history = line("a", json!({"id": SENSE_ID, "hidden": true}));
    assert_eq!(fold(base(), &[&no_history], &key("a"))["hidden"], false);
}

#[test]
fn the_records_own_edits_and_hide_win() {
    let mut existing = base();
    existing.insert("title".to_owned(), json!("Mine"));
    existing.insert("hidden".to_owned(), json!(true));
    existing.insert(
        "edits".to_owned(),
        json!([
            edit(
                "2026-02-01T00:00:00Z",
                "owner:update",
                &["title"],
                json!({"title": "Mine"})
            ),
            hide("2026-02-02T00:00:00Z", "owner:hide")
        ]),
    );
    let a = visible_with_title();
    let folded = fold(existing, &[&a], &key("a"));
    assert_eq!(folded["title"], "Mine");
    assert_eq!(folded["hidden"], true);
    let edits = folded["edits"].as_array().unwrap();
    // Carried first, own after, and no decision entry beside its own history.
    assert_eq!(edits[0]["carried"], key("a"));
    assert!(edits[1].get("carried").is_none());
    assert!(edits.iter().all(|edit| edit["carried"] != DECISION));
}

#[test]
fn folding_twice_changes_nothing() {
    let (a, b) = (visible_with_title(), hidden_by_owner());
    let once = fold(base(), &[&a, &b], &key("a"));
    let twice = decide(once.clone(), &[&a, &b], &|_| true, &key("b"), LATER);
    assert_eq!(once, twice);
}

#[test]
fn a_line_gone_from_disk_counts_as_what_was_stored() {
    let (a, b) = (visible_with_title(), hidden_by_owner());
    let once = fold(base(), &[&a, &b], &key("a"));
    // A's line is removed; the walk finds B only. Hide stays false.
    let later = decide(once.clone(), &[&b], &|_| true, &key("a"), LATER);
    assert_eq!(later["hidden"], false);
    assert_eq!(later["title"], "Run");
    assert_eq!(once, later);
}

#[test]
fn a_changed_line_is_reread_in_place_without_a_duplicate() {
    let (a, b) = (visible_with_title(), hidden_by_owner());
    let once = fold(base(), &[&a, &b], &key("a"));
    let edited = line(
        "b",
        json!({"id": SENSE_ID, "hidden": true, "title": "B title", "edits": [
            hide("2026-01-03T00:00:00Z", "owner:hide"),
            edit("2026-11-09T02:00:00Z", "owner:update", &["title"], json!({"title": "B title"}))]}),
    );
    let later = decide(once, &[&a, &edited], &|_| true, &key("a"), LATER);
    assert_eq!(later["title"], "Run");
    let carried_b = later["edits"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|edit| edit["carried"] == key("b"))
        .count();
    assert_eq!(carried_b, 1);
}

#[test]
fn a_line_brought_back_later_is_added_and_the_primary_stays_pinned() {
    let a = visible_with_title();
    let once = fold(base(), &[&a], &key("a"));
    let back = line(
        "c",
        json!({"id": SENSE_ID, "title": "Back", "edits": [
            edit("2026-07-01T00:00:00Z", "owner:update", &["title"], json!({"title": "Back"}))]}),
    );
    let later = decide(once, &[&back], &|_| true, &key("c"), LATER);
    assert_eq!(later["converted"]["primary"], key("a"));
    assert_eq!(later["title"], "Run");
    assert!(later["converted"]["lines"].get(key("c")).is_some());
}

#[test]
fn a_hide_after_the_first_decision_counts_as_visible() {
    let a = line("a", json!({"id": SENSE_ID}));
    let once = fold(base(), &[&a], &key("a"));
    let hidden_later = line(
        "a",
        json!({"id": SENSE_ID, "hidden": true,
            "edits": [hide("2026-11-09T02:00:00Z", "owner:hide")]}),
    );
    let later = decide(once, &[&hidden_later], &|_| true, &key("a"), LATER);
    assert_eq!(later["hidden"], false);
}

#[test]
fn a_decided_time_is_never_rewritten() {
    let a = visible_with_title();
    let once = fold(base(), &[&a], &key("a"));
    let later = decide(once, &[&a], &|_| true, &key("a"), LATER);
    assert_eq!(later["converted"]["decided_at"], NOW);
}

#[test]
fn a_spanning_line_gives_its_fields_only_where_told_and_its_hide_to_both() {
    let span = line(
        "a",
        json!({"id": SENSE_ID, "title": "Long", "hidden": true, "edits": [
            edit("2026-01-02T00:00:00Z", "owner:update", &["title"], json!({"title": "Long"})),
            hide("2026-01-03T00:00:00Z", "owner:hide")]}),
    );
    let first = decide(base(), &[&span], &|_| true, &key("a"), NOW);
    let second = decide(base(), &[&span], &|_| false, &key("a"), NOW);
    assert_eq!(first["title"], "Long");
    assert!(!second.contains_key("title"));
    assert_eq!(first["hidden"], true);
    assert_eq!(second["hidden"], true);
}

#[test]
fn the_primary_is_the_target_facets_first_tile_then_the_earliest() {
    let a = line(
        "a",
        json!({"id": "x", "created_at": 5, "segments": ["070000_300"]}),
    );
    let b = line(
        "b",
        json!({"id": "y", "created_at": 1, "segments": ["070500_300"]}),
    );
    let c = line(
        "b",
        json!({"id": "z", "created_at": 9, "segments": ["070000_300"]}),
    );
    assert_eq!(
        choose_primary(&[&a, &b, &c], "b", Some(("20260101", "070000_300"))),
        Some(c.key())
    );
    assert_eq!(
        choose_primary(&[&a, &b], "b", Some(("20260101", "070000_300"))),
        Some(b.key())
    );
}
