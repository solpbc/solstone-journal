// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    CalendarVersion, ConnectionBoundary, ConnectionScope, ConnectionSearchRequest, IndexedEntry,
    Order, OwnerBoundary, QueryBoundary, SearchHit, SearchRequest, coverage, hit_at,
    read_indexed_entry, search, search_connection,
};

const GOLDEN: &str = include_str!("../../../fixtures/calendar_notes/golden.jsonl");

struct Journal(PathBuf);

impl Journal {
    fn new() -> Self {
        let root = crate::test_support::reserve_temp_path("query-calendar");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            json!({"identity": {"timezone": "America/Denver"}}).to_string(),
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
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn save(&self, day: &str, segment: &str, lines: &[&str]) -> String {
        let dir = self
            .0
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
        let name = format!("calendar_{:x}.jsonl", Sha256::digest(&bytes));
        fs::write(dir.join(&name), bytes).unwrap();
        format!("{day}/phone_calendar/{segment}/{name}")
    }

    fn capture(&self, day: &str, segment: &str, text: &str) {
        let dir = self
            .0
            .join("chronicle")
            .join(day)
            .join("desk")
            .join(segment)
            .join("talents");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("summary.md"), text).unwrap();
    }

    fn scan(&self) {
        solstone_core_indexer_store::scan::scan_journal(self.path(), true).unwrap();
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn golden(index: usize) -> &'static str {
    GOLDEN.lines().nth(index).unwrap()
}

fn reference() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()
}

fn owner(
    journal: &Journal,
    query: &str,
    day: Option<&str>,
    range: Option<(&str, &str)>,
) -> Vec<SearchHit> {
    search(
        journal.path(),
        OwnerBoundary,
        &SearchRequest {
            query: query.to_owned(),
            limit: 50,
            day: day.map(str::to_owned),
            day_from: range.map(|(from, _)| from.to_owned()),
            day_to: range.map(|(_, to)| to.to_owned()),
            ..SearchRequest::new("", Order::Recency)
        },
        reference(),
    )
    .unwrap()
    .results
}

fn calendar_hits(hits: &[SearchHit]) -> Vec<(String, CalendarVersion, String)> {
    hits.iter()
        .filter_map(|hit| {
            hit.metadata.calendar.as_ref().map(|calendar| {
                (
                    hit.metadata.day.clone(),
                    calendar.version,
                    calendar.changed_at.clone(),
                )
            })
        })
        .collect()
}

fn journal_with_golden_lines() -> Journal {
    let journal = Journal::new();
    journal.save("20261019", "094000_300", &[golden(0)]);
    journal.save("20261020", "163000_300", &[golden(1), golden(2), golden(3)]);
    journal.capture("20261027", "090000_300", "design work on the slides");
    journal.capture("20261021", "120000_300", "lunch notes");
    journal.scan();
    journal
}

#[test]
fn a_day_search_finds_the_event_listed_under_the_day_it_happens() {
    let journal = journal_with_golden_lines();
    for (query, day) in [
        ("", "20261027"),
        ("design", "20261027"),
        ("", "20261021"),
        ("standup", "20261021"),
    ] {
        let hits = owner(&journal, query, Some(day), None);
        let calendar = calendar_hits(&hits);
        assert_eq!(calendar.len(), 1, "{query:?} {day}: {calendar:?}");
        assert_eq!(calendar[0].1, CalendarVersion::Latest);
        assert_eq!(calendar[0].0, day);
        if query.is_empty() {
            assert!(
                hits.iter().any(|hit| hit.metadata.calendar.is_none()),
                "the day holds other content"
            );
        }
    }
    let meeting = owner(&journal, "", Some("20261027"), None);
    assert_eq!(calendar_hits(&meeting)[0].2, "2026-10-19T09:42:00-06:00");
    let standup = owner(&journal, "", Some("20261021"), None);
    assert_eq!(calendar_hits(&standup)[0].2, "2026-10-20T16:30:00-06:00");
    // A range of days finds them too.
    let range = owner(&journal, "", None, Some(("20261021", "20261031")));
    let days = calendar_hits(&range)
        .into_iter()
        .filter(|(_, version, _)| *version == CalendarVersion::Latest)
        .map(|(day, _, _)| day)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        days,
        BTreeSet::from(["20261021".into(), "20261027".into(), "20261031".into()])
    );
}

#[test]
fn a_stored_day_search_finds_each_change_on_the_day_it_was_recorded() {
    let journal = journal_with_golden_lines();
    let booked = calendar_hits(&owner(&journal, "", Some("20261019"), None));
    assert_eq!(
        booked,
        [(
            "20261019".into(),
            CalendarVersion::Recorded,
            "2026-10-19T09:42:00-06:00".into()
        )]
    );
    let changed = calendar_hits(&owner(&journal, "", Some("20261020"), None));
    assert_eq!(changed.len(), 3);
    assert!(
        changed
            .iter()
            .all(|(_, version, _)| *version == CalendarVersion::Recorded)
    );
    assert!(
        changed
            .iter()
            .any(|(_, _, at)| at == "2026-10-20T16:31:00-06:00")
    );
    // The marker and the time reach the wire, and ordinary hits carry neither.
    let hits = owner(&journal, "", Some("20261019"), None);
    let wire = serde_json::to_value(&hits).unwrap();
    assert_eq!(wire[0]["metadata"]["calendar"]["version"], "recorded");
    assert_eq!(
        wire[0]["metadata"]["calendar"]["changed_at"],
        "2026-10-19T09:42:00-06:00"
    );
    let other = owner(&journal, "", Some("20261021"), None);
    let plain = other
        .iter()
        .find(|hit| hit.metadata.calendar.is_none())
        .unwrap();
    assert!(
        serde_json::to_value(plain).unwrap()["metadata"]
            .get("calendar")
            .is_none()
    );
}

#[test]
fn whole_journal_connections_see_both_kinds_of_hit_and_facet_connections_neither() {
    let journal = journal_with_golden_lines();
    let request = |day: &str| ConnectionSearchRequest {
        query: String::new(),
        limit: 50,
        day: Some(day.to_owned()),
        ..ConnectionSearchRequest::default()
    };
    let whole =
        ConnectionBoundary::from_category_tokens(["transcripts"], ConnectionScope::WholeJournal)
            .unwrap();
    let facets = ConnectionBoundary::from_category_tokens(
        ["transcripts"],
        ConnectionScope::ChosenFacets {
            ids: BTreeSet::from(["123e4567-e89b-42d3-a456-426614174000".to_owned()]),
        },
    )
    .unwrap();
    for (day, version) in [
        ("20261027", CalendarVersion::Latest),
        ("20261019", CalendarVersion::Recorded),
    ] {
        let seen = search_connection(journal.path(), &whole, &request(day), reference()).unwrap();
        let calendar = seen
            .results
            .iter()
            .filter_map(|hit| hit.metadata.calendar.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(calendar.len(), 1, "{day}");
        assert_eq!(calendar[0].version, version);
        let unseen =
            search_connection(journal.path(), &facets, &request(day), reference()).unwrap();
        assert!(
            unseen
                .results
                .iter()
                .all(|hit| hit.metadata.calendar.is_none()),
            "{day}"
        );
    }
}

fn set_zone(journal: &Journal, zone: &str) {
    fs::write(
        journal.path().join("config/journal.json"),
        json!({"identity": {"timezone": zone}}).to_string(),
    )
    .unwrap();
}

fn all_day_note(title: &str, event: &str, on: NaiveDate, entered: NaiveDate) -> String {
    let mut value: serde_json::Value = serde_json::from_str(golden(2)).unwrap();
    value["title"] = json!(title);
    value["event"] = json!(event);
    value["calendar"] = json!("Work");
    value["start"] = json!(on.format("%Y-%m-%d").to_string());
    value["end"] = json!((on + chrono::Days::new(1)).format("%Y-%m-%d").to_string());
    value["changed_at"] = json!(format!("{}T09:00:00Z", entered.format("%Y-%m-%d")));
    value.to_string()
}

fn at(instant: &str) {
    let now = chrono::DateTime::parse_from_rfc3339(instant)
        .unwrap()
        .with_timezone(&chrono::Utc);
    crate::execute::TEST_NOW.with(|cell| cell.set(Some(now)));
}

fn date(day: &str) -> NaiveDate {
    NaiveDate::parse_from_str(day, "%Y%m%d").unwrap()
}

#[test]
fn today_is_the_latest_day_begun_anywhere() {
    for (instant, day) in [
        ("2030-06-10T09:59:59Z", "20300610"),
        ("2030-06-10T10:00:00Z", "20300611"),
        ("2030-06-10T23:59:59Z", "20300611"),
        ("2030-12-31T12:00:00-06:00", "20310101"),
    ] {
        let now = chrono::DateTime::parse_from_rfc3339(instant).unwrap();
        assert_eq!(
            crate::execute::today_anywhere(now.with_timezone(&chrono::Utc)),
            day
        );
    }
}

#[test]
fn nothing_after_today_is_returned_until_that_day_arrives() {
    let journal = Journal::new();
    // 08:00 in Denver on the 10th: the 11th has begun at UTC+14, the 12th
    // has begun nowhere.
    at("2030-06-10T14:00:00Z");
    let lines = [
        all_day_note(
            "offsite review",
            "11111111-2222-4333-8444-555555555553",
            date("20300610"),
            date("20300610"),
        ),
        all_day_note(
            "offsite checkin",
            "11111111-2222-4333-8444-555555555552",
            date("20300612"),
            date("20300610"),
        ),
        all_day_note(
            "offsite planning",
            "11111111-2222-4333-8444-555555555551",
            date("20300620"),
            date("20300610"),
        ),
    ];
    journal.save("20300610", "090000_300", &[&lines[0], &lines[1], &lines[2]]);
    journal.scan();

    // Every listing is stored: the rule is applied when the journal is read.
    let index =
        rusqlite::Connection::open(solstone_core_indexer_store::db::db_path(journal.path()))
            .unwrap();
    let later_rows = index
        .prepare("SELECT rowid, path, idx, day FROM chunks WHERE day > '20300611' ORDER BY day")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        later_rows
            .iter()
            .map(|row| row.3.as_str())
            .collect::<Vec<_>>(),
        ["20300612", "20300620"]
    );

    let read = |journal: &Journal| {
        let hits = owner(journal, "offsite", None, None);
        assert!(
            hits.iter()
                .all(|hit| hit.metadata.day.as_str() <= "20300611"),
            "{hits:?}"
        );
        calendar_hits(&hits)
    };
    // Each note on the day it entered the journal, today's event on today,
    // and nothing after today, however the days are asked for.
    let listed = read(&journal);
    assert_eq!(
        listed
            .iter()
            .filter(|(_, version, _)| *version == CalendarVersion::Recorded)
            .count(),
        3
    );
    assert_eq!(
        listed
            .iter()
            .filter(|(_, version, _)| *version == CalendarVersion::Latest)
            .map(|(on, _, _)| on.as_str())
            .collect::<Vec<_>>(),
        ["20300610"]
    );
    assert!(owner(&journal, "", Some("20300612"), None).is_empty());
    let range = owner(&journal, "", None, Some(("20300610", "20300630")));
    assert!(
        range
            .iter()
            .all(|hit| hit.metadata.day.as_str() <= "20300611"),
        "{range:?}"
    );
    let whole =
        ConnectionBoundary::from_category_tokens(["transcripts"], ConnectionScope::WholeJournal)
            .unwrap();
    let agent = search_connection(
        journal.path(),
        &whole,
        &ConnectionSearchRequest {
            query: "offsite".into(),
            limit: 50,
            ..ConnectionSearchRequest::default()
        },
        reference(),
    )
    .unwrap();
    assert!(!agent.results.is_empty());
    assert!(
        agent
            .results
            .iter()
            .all(|hit| hit.metadata.day.as_str() <= "20300611")
    );
    for (row_id, path, idx, _) in &later_rows {
        assert_eq!(
            read_indexed_entry(
                journal.path(),
                QueryBoundary::Owner,
                path,
                *idx,
                *row_id,
                1 << 20
            )
            .unwrap(),
            IndexedEntry::NotFound
        );
        assert!(!hit_at(journal.path(), QueryBoundary::Owner, path, *idx).unwrap());
    }
    assert_eq!(
        coverage(journal.path(), QueryBoundary::Owner)
            .unwrap()
            .end
            .as_deref(),
        Some("20300610")
    );

    // The journal's own zone setting, or a change to it, moves nothing.
    for zone in ["Pacific/Pago_Pago", "Pacific/Kiritimati", "Europe/Madrid"] {
        set_zone(&journal, zone);
        assert_eq!(read(&journal), listed, "{zone}");
    }

    // The 12th arrives: the same index, no scan in between.
    at("2030-06-11T14:00:00Z");
    let arrived = calendar_hits(&owner(&journal, "offsite", Some("20300612"), None));
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    assert_eq!(arrived[0].1, CalendarVersion::Latest);
    assert!(owner(&journal, "", Some("20300620"), None).is_empty());
    assert_eq!(
        coverage(journal.path(), QueryBoundary::Owner)
            .unwrap()
            .end
            .as_deref(),
        Some("20300612")
    );
}
