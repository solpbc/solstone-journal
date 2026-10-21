// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

//! Calendar notes as an agent connection sees them: both kinds of hit from
//! search, each fetchable and each a place the search cursor can stand.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::dispatch::run_mcp_probe;
use crate::permissions::{PermissionStore, ReadPermission, ReadScope};

const CONNECTION: &str = "bearer:calendar-test";
const FACET: &str = "123e4567-e89b-42d3-a456-426614174000";
const GOLDEN: &str = include_str!("../../../fixtures/calendar_notes/golden.jsonl");
const DAY: &str = "20261027";

fn golden(index: usize) -> &'static str {
    GOLDEN.lines().nth(index).unwrap()
}

fn journal() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("solstone-mcp-calendar-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let root = journal.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        json!({"identity": {"timezone": "America/Denver"}}).to_string(),
    )
    .unwrap();
    let facet = root.join("facets/work");
    fs::create_dir_all(&facet).unwrap();
    fs::write(
        facet.join("facet.json"),
        json!({"id": FACET, "title": "Work", "description": "work"}).to_string(),
    )
    .unwrap();
    fs::create_dir_all(root.join("streams")).unwrap();
    fs::write(
        root.join("streams/phone_calendar.json"),
        json!({
            "name": "phone_calendar", "kind": "observer", "host": null, "platform": null,
            "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
            "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "source": "calendar",
        })
        .to_string(),
    )
    .unwrap();
    journal
}

fn grant(journal: &Path, scope: ReadScope) {
    PermissionStore::open(journal)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope,
            },
        )
        .unwrap();
}

fn calendar_file(journal: &Path, day: &str, segment: &str, lines: &[&str]) {
    let dir = journal
        .join("chronicle")
        .join(day)
        .join("phone_calendar")
        .join(segment);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("stream.json"),
        r#"{"stream":"phone_calendar","prev_day":null,"prev_segment":null,"seq":1}"#,
    )
    .unwrap();
    let mut bytes = lines.join("\n").into_bytes();
    bytes.push(b'\n');
    fs::write(
        dir.join(format!("calendar_{:x}.jsonl", Sha256::digest(&bytes))),
        bytes,
    )
    .unwrap();
}

/// Ordinary capture on a day: a summary in the work facet.
fn capture(journal: &Path, day: &str, segment: &str, text: &str) {
    let dir = journal
        .join("chronicle")
        .join(day)
        .join("desk")
        .join(segment);
    fs::create_dir_all(dir.join("talents")).unwrap();
    fs::write(dir.join("talents/facets.json"), r#"[{"facet":"work"}]"#).unwrap();
    fs::write(dir.join("talents/brief.md"), format!("{text}\n")).unwrap();
}

fn index(journal: &Path) {
    let report = solstone_core_indexer_store::scan::scan_journal(journal, true).unwrap();
    assert_eq!(report.failed, 0, "{report:?}");
}

fn search(journal: &Path, arguments: Value) -> Value {
    run_mcp_probe(journal, CONNECTION, "search", &arguments).unwrap()
}

/// Page through a day's search with `limit: 1`, by the cursor alone.
fn every_page(journal: &Path, day: &str) -> Vec<Value> {
    let mut results = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..50 {
        let mut arguments = json!({"query": "", "day": day, "limit": 1});
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = search(journal, arguments);
        results.extend(page["results"].as_array().unwrap().iter().cloned());
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return results,
        }
    }
    panic!("paging did not end");
}

fn calendar_results(results: &[Value]) -> Vec<(String, String)> {
    let mut found = results
        .iter()
        .filter(|result| result.get("calendar").is_some())
        .map(|result| {
            let title = ["design review", "Halloween", "standup"]
                .into_iter()
                .find(|title| result["snippet"].as_str().unwrap().contains(title))
                .unwrap_or("other");
            (
                result["calendar"]["version"].as_str().unwrap().to_owned(),
                title.to_owned(),
            )
        })
        .collect::<Vec<_>>();
    found.sort();
    found
}

#[test]
fn paging_a_busy_day_by_cursor_returns_each_calendar_hit_once() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    // A meeting booked and held the same day, and a second event from the
    // same file that happens on another day.
    let booked = golden(0).replace("2026-10-19T09:42:00-06:00", "2026-10-27T06:41:00-06:00");
    let other = golden(2).replace("2026-10-20T16:31:00-06:00", "2026-10-27T06:42:00-06:00");
    calendar_file(root, DAY, "064000_300", &[&booked, &other]);
    for (segment, text) in [
        ("080000_300", "planning the quarterly roadmap"),
        ("090000_300", "reviewing the budget sheet"),
        ("100000_300", "drafting the launch post"),
    ] {
        capture(root, DAY, segment, text);
    }
    index(root);
    let results = every_page(root, "2026-10-27");
    assert!(results.len() >= 6, "{results:?}");
    assert_eq!(
        calendar_results(&results),
        [
            ("latest".to_owned(), "design review".to_owned()),
            ("recorded".to_owned(), "Halloween".to_owned()),
            ("recorded".to_owned(), "design review".to_owned()),
        ]
    );
    // Each hit says when its version was recorded.
    let times = results
        .iter()
        .filter_map(|result| result["calendar"]["changed_at"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        times,
        ["2026-10-27T06:41:00-06:00", "2026-10-27T06:42:00-06:00"].into()
    );
    // The second event is listed on the day it happens, not here.
    let halloween = every_page(root, "20261031");
    assert_eq!(
        calendar_results(&halloween),
        [("latest".to_owned(), "Halloween".to_owned())]
    );
}

#[test]
fn a_whole_journal_connection_fetches_both_hits_and_a_facet_connection_sees_neither() {
    let journal = journal();
    let root = journal.path();
    calendar_file(root, "20261019", "094000_300", &[golden(0)]);
    capture(root, DAY, "080000_300", "design notes for the review");
    capture(root, "20261019", "080000_300", "a quiet morning");
    index(root);
    grant(root, ReadScope::WholeJournal);
    for (day, version) in [(DAY, "latest"), ("20261019", "recorded")] {
        let page = search(root, json!({"query": "", "day": day, "limit": 10}));
        let hit = page["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|result| result.get("calendar").is_some())
            .unwrap_or_else(|| panic!("calendar hit on {day}: {page}"));
        assert_eq!(hit["calendar"]["version"], version);
        assert_eq!(hit["calendar"]["changed_at"], "2026-10-19T09:42:00-06:00");
        assert_eq!(hit["date"], day);
        let fetched = run_mcp_probe(
            root,
            CONNECTION,
            "fetch",
            &json!({"reference": hit["reference"]}),
        )
        .unwrap();
        assert!(fetched["text"].as_str().unwrap().contains("design review"));
    }
    grant(
        root,
        ReadScope::Facets {
            ids: vec![FACET.to_owned()],
        },
    );
    for day in [DAY, "20261019"] {
        let page = search(root, json!({"query": "", "day": day, "limit": 10}));
        let results = page["results"].as_array().unwrap();
        assert!(
            !results.is_empty(),
            "the facet's own content is still found"
        );
        assert!(
            results
                .iter()
                .all(|result| result.get("calendar").is_none()),
            "{page}"
        );
    }
}

#[test]
fn a_note_with_long_notes_is_found_fetched_and_anchors_a_cursor() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    let long = "the agenda runs long ".repeat(6_000);
    let line = golden(0)
        .replace("2026-10-19T09:42:00-06:00", "2026-10-27T06:41:00-06:00")
        .replace(
            r#""notes":"bring the mocks""#,
            &format!(r#""notes":"{long}""#),
        );
    calendar_file(root, DAY, "064000_300", &[&line]);
    capture(root, DAY, "080000_300", "planning the quarterly roadmap");
    capture(root, DAY, "090000_300", "reviewing the budget sheet");
    index(root);
    let results = every_page(root, DAY);
    let calendar = results
        .iter()
        .filter(|result| result.get("calendar").is_some())
        .collect::<Vec<_>>();
    assert_eq!(calendar.len(), 2, "{results:?}");
    // Every hit anchored the next page, and every hit can be read back.
    assert!(results.len() >= 4);
    for hit in calendar {
        let fetched = run_mcp_probe(
            root,
            CONNECTION,
            "fetch",
            &json!({"reference": hit["reference"]}),
        )
        .unwrap();
        let text = fetched["text"].as_str().unwrap();
        assert!(text.contains("the agenda runs long"));
        assert!(text.len() < long.len());
    }
}

#[test]
fn calendar_segments_are_not_listed_as_transcripts_of_the_day() {
    let journal = journal();
    let root = journal.path();
    grant(root, ReadScope::WholeJournal);
    calendar_file(root, "20261019", "094000_300", &[golden(0)]);
    capture(root, "20261019", "100000_300", "a working session");
    index(root);
    let listed = run_mcp_probe(
        root,
        CONNECTION,
        "list_transcripts",
        &json!({"day": "20261019"}),
    )
    .unwrap();
    let transcripts = listed["transcripts"].as_array().unwrap();
    // The capture is listed; the calendar segment is not.
    let times = transcripts
        .iter()
        .map(|transcript| transcript["time"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(times, ["10:00-10:05"], "{listed}");
}

fn grant_categories(journal: &Path, categories: &[&str], scope: ReadScope) {
    PermissionStore::open(journal)
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: categories
                    .iter()
                    .map(|category| (*category).to_owned())
                    .collect(),
                scope,
            },
        )
        .unwrap();
}

fn scheduled_beside_activities(journal: &Path) -> Vec<Value> {
    let listed =
        run_mcp_probe(journal, CONNECTION, "list_activities", &json!({"day": DAY})).unwrap();
    listed["activities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|activity| activity.get("scheduled").cloned().unwrap_or(Value::Null))
        .collect()
}

#[test]
fn an_activity_lists_the_meeting_it_ran_during_only_to_a_connection_that_reads_the_calendar() {
    let journal = journal();
    let root = journal.path();
    // Golden line 0: "design review", 10:00–11:00 Denver, organized by Alex.
    calendar_file(root, "20261019", "094000_300", &[golden(0)]);
    let alex = root.join("entities/alex_rivera");
    fs::create_dir_all(&alex).unwrap();
    fs::write(
        alex.join("entity.json"),
        json!({"id": "alex_rivera", "name": "Alex Rivera", "type": "Person", "emails": ["alex@example.com"]})
            .to_string(),
    )
    .unwrap();
    fs::create_dir_all(root.join("facets/work/activities")).unwrap();
    let during = json!({"id": "meeting_100200_300", "activity": "meeting", "segments": ["100200_1800", "103200_540"], "description": "talked through the mocks", "created_at": 1});
    let after = json!({"id": "coding_120000_300", "activity": "coding", "segments": ["120000_1800"], "description": "wrote code", "created_at": 2});
    fs::write(
        root.join(format!("facets/work/activities/{DAY}.jsonl")),
        format!("{during}\n{after}\n"),
    )
    .unwrap();
    index(root);

    grant_categories(
        root,
        &["facets", "transcripts", "entities"],
        ReadScope::WholeJournal,
    );
    assert_eq!(
        scheduled_beside_activities(root),
        [
            json!([{"title": "design review", "start": "10:00", "end": "11:00", "people": ["Alex Rivera"]}]),
            Value::Null,
        ]
    );
    // Without the people grant, the meeting comes without names.
    grant_categories(root, &["facets", "transcripts"], ReadScope::WholeJournal);
    assert_eq!(
        scheduled_beside_activities(root),
        [
            json!([{"title": "design review", "start": "10:00", "end": "11:00"}]),
            Value::Null,
        ]
    );

    // The calendar belongs to no facet, and without transcripts a connection
    // doesn't read it at all.
    grant_categories(
        root,
        &["facets", "transcripts"],
        ReadScope::Facets {
            ids: vec![FACET.to_owned()],
        },
    );
    assert_eq!(
        scheduled_beside_activities(root),
        [Value::Null, Value::Null]
    );
    grant_categories(root, &["facets"], ReadScope::WholeJournal);
    assert_eq!(
        scheduled_beside_activities(root),
        [Value::Null, Value::Null]
    );
}
