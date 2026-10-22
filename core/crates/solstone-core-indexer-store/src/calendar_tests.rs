// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{LISTING_IDX_BASE, interrupted_after_first_write};
use crate::db::{db_path, prune_by_paths, reset_index};
use crate::scan::{rescan_file, scan_journal};

const GOLDEN: &str = include_str!("../../../fixtures/calendar_notes/golden.jsonl");
const CALENDAR: &str = "phone_calendar";
const CAPTURE: &str = "phone";
const MEETING: &str = "6CF25C80-68D2-44D9-B52C-6CAD5BEC8C10";
const STANDUP: &str = "CCBF5C02-6D92-477A-AB25-1A2ED19E1D67";

fn golden(index: usize) -> &'static str {
    GOLDEN.lines().nth(index).unwrap()
}

/// A golden line with some keys replaced.
fn note(index: usize, changes: Value) -> String {
    let mut record: serde_json::Map<String, Value> = serde_json::from_str(golden(index)).unwrap();
    for (key, value) in changes.as_object().unwrap() {
        record.insert(key.clone(), value.clone());
    }
    Value::Object(record).to_string()
}

struct Journal(PathBuf);

impl Journal {
    fn new(zone: Option<&str>) -> Self {
        let root = crate::test_support::reserve_temp_path("index-calendar");
        fs::create_dir_all(root.join("config")).unwrap();
        if let Some(zone) = zone {
            fs::write(
                root.join("config/journal.json"),
                json!({"identity": {"timezone": zone}}).to_string(),
            )
            .unwrap();
        }
        let journal = Self(root);
        journal.stream(CALENDAR, "calendar");
        journal.stream(CAPTURE, "audio");
        journal
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn stream(&self, name: &str, source: &str) {
        fs::create_dir_all(self.0.join("streams")).unwrap();
        fs::write(
            self.0.join("streams").join(format!("{name}.json")),
            json!({
                "name": name, "kind": "observer", "host": null, "platform": null,
                "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
                "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "source": source,
            })
            .to_string(),
        )
        .unwrap();
    }

    fn segment(&self, day: &str, stream: &str, segment: &str) -> PathBuf {
        let dir = self
            .0
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("stream.json"),
            json!({"stream": stream, "prev_day": null, "prev_segment": null, "seq": 1}).to_string(),
        )
        .unwrap();
        dir
    }

    /// Store a calendar file in `stream`, returning its index path.
    fn save_in(&self, stream: &str, day: &str, segment: &str, lines: &[&str]) -> String {
        let mut bytes = lines.join("\n").into_bytes();
        bytes.push(b'\n');
        let name = format!("calendar_{:x}.jsonl", Sha256::digest(&bytes));
        fs::write(self.segment(day, stream, segment).join(&name), bytes).unwrap();
        format!("{day}/{stream}/{segment}/{name}")
    }

    fn save(&self, day: &str, segment: &str, lines: &[&str]) -> String {
        self.save_in(CALENDAR, day, segment, lines)
    }

    /// Ordinary capture content on a day.
    fn capture(&self, day: &str, segment: &str, text: &str) -> String {
        let dir = self.segment(day, CAPTURE, segment).join("talents");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("summary.md"), text).unwrap();
        format!("{day}/{CAPTURE}/{segment}/talents/summary.md")
    }

    fn index(&self, rel: &str) {
        rescan_file(self.path(), Path::new(rel)).unwrap();
    }

    /// Remove a file's segment from disk and tell the index, as an owner's
    /// delete does.
    fn delete_segment_of(&self, rel: &str) {
        let segment = rel.rsplit_once('/').unwrap().0;
        fs::remove_dir_all(self.0.join("chronicle").join(segment)).unwrap();
        prune_by_paths(self.path(), &[segment]).unwrap();
    }

    fn conn(&self) -> Connection {
        Connection::open(db_path(self.path())).unwrap()
    }

    /// Every listing row: (day, path, idx, bucket, content).
    fn listing(&self) -> Vec<(String, String, i64, String, String)> {
        let conn = self.conn();
        let mut statement = conn
            .prepare(
                "SELECT c.day, c.path, c.idx, c.time_bucket, c.content FROM chunks c \
                 JOIN calendar_hits h ON h.rowid=c.rowid WHERE h.version='latest' \
                 ORDER BY c.day, c.path, c.idx",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// The days an event is listed under.
    fn listed(&self, event: &str) -> Vec<String> {
        let conn = self.conn();
        let mut statement = conn
            .prepare(
                "SELECT DISTINCT c.day FROM chunks c JOIN calendar_hits h ON h.rowid=c.rowid \
                 WHERE h.version='latest' AND h.event=? ORDER BY c.day",
            )
            .unwrap();
        statement
            .query_map([event], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// The stored-day rows: (day, path, idx).
    fn stored(&self) -> Vec<(String, String, i64)> {
        let conn = self.conn();
        let mut statement = conn
            .prepare(
                "SELECT c.day, c.path, c.idx FROM chunks c JOIN calendar_hits h ON h.rowid=c.rowid \
                 WHERE h.version='recorded' ORDER BY c.day, c.path, c.idx",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.conn().query_row(sql, [], |row| row.get(0)).unwrap()
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A timed note for an identity, recorded at `changed_at`, happening at
/// `start` for one hour.
fn timed(event: &str, change: &str, changed_at: &str, start: &str, end: &str) -> String {
    note(
        0,
        json!({"event": event, "change": change, "changed_at": changed_at, "start": start, "end": end, "title": format!("{event} {start}")}),
    )
}

#[test]
fn each_golden_line_is_stored_on_its_own_day_and_listed_on_the_days_it_happens() {
    let journal = Journal::new(Some("America/Denver"));
    let files = [
        journal.save("20261019", "094000_300", &[golden(0)]),
        journal.save("20261020", "163000_300", &[golden(1), golden(2), golden(3)]),
    ];
    for file in &files {
        journal.index(file);
    }
    assert_eq!(
        journal.stored(),
        [
            ("20261019".to_owned(), files[0].clone(), 0),
            ("20261020".to_owned(), files[1].clone(), 0),
            ("20261020".to_owned(), files[1].clone(), 1),
            ("20261020".to_owned(), files[1].clone(), 2),
        ]
    );
    let listing = journal.listing();
    let days = listing
        .iter()
        .map(|(day, _, _, bucket, _)| (day.as_str(), bucket.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        days,
        [
            ("20261021", "morning"),
            ("20261027", "morning"),
            ("20261031", ""),
            ("20270120", ""),
        ]
    );
    // Each listing row is filed under the file holding its note.
    assert_eq!(listing[1].1, files[0]);
    assert_eq!(listing[0].1, files[1]);
    assert!(listing[1].4.contains("design review"));
    assert!(listing[3].4.contains("Kate Bell's Birthday"));
    assert!(listing.iter().all(|row| row.2 >= LISTING_IDX_BASE));

    // The removal takes the meeting out of the listing; the history stays.
    let removed = journal.save("20261022", "080500_300", &[golden(4)]);
    journal.index(&removed);
    assert!(journal.listed(MEETING).is_empty());
    assert_eq!(journal.listed(STANDUP), ["20261021"]);
    assert_eq!(journal.stored().len(), 5);
    let removal_text: String = journal
        .conn()
        .query_row(
            "SELECT content FROM chunks WHERE path=? AND idx=0",
            [&removed],
            |row| row.get(0),
        )
        .unwrap();
    assert!(removal_text.contains("design review") && removal_text.contains("Work"));
}

#[test]
fn the_latest_version_wins_in_every_indexing_order() {
    let notes = [
        timed(
            MEETING,
            "added",
            "2026-10-19T09:42:00-06:00",
            "2026-10-27T10:00:00-06:00",
            "2026-10-27T11:00:00-06:00",
        ),
        timed(
            MEETING,
            "changed",
            "2026-10-20T08:00:00-06:00",
            "2026-10-28T14:00:00-06:00",
            "2026-10-28T15:00:00-06:00",
        ),
        timed(
            MEETING,
            "changed",
            "2026-10-21T07:00:00-06:00",
            "2026-10-29T18:00:00-06:00",
            "2026-10-29T19:00:00-06:00",
        ),
    ];
    let segments = [
        ("20261019", "094000_300"),
        ("20261020", "080000_300"),
        ("20261021", "070000_300"),
    ];
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let mut listings = BTreeSet::new();
    for order in orders {
        let journal = Journal::new(Some("America/Denver"));
        let files = segments
            .iter()
            .zip(&notes)
            .map(|((day, segment), line)| journal.save(day, segment, &[line]))
            .collect::<Vec<_>>();
        for index in order {
            journal.index(&files[index]);
        }
        assert_eq!(journal.listed(MEETING), ["20261029"], "order {order:?}");
        let listing = journal.listing();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].3, "evening");
        assert_eq!(listing[0].1, files[2]);
        // The older versions stay findable on the days they were recorded.
        assert_eq!(journal.stored().len(), 3);
        listings.insert(listing);
    }
    assert_eq!(listings.len(), 1, "every order gives one identical listing");
}

#[test]
fn equal_times_break_removed_over_changed_over_added_then_by_file_name() {
    let at = "2026-10-20T16:30:00-06:00";
    let added = timed(
        STANDUP,
        "added",
        at,
        "2026-10-26T10:00:00-06:00",
        "2026-10-26T11:00:00-06:00",
    );
    let changed = timed(
        STANDUP,
        "changed",
        at,
        "2026-10-27T10:00:00-06:00",
        "2026-10-27T11:00:00-06:00",
    );
    let removed = note(4, json!({"event": STANDUP, "changed_at": at}));
    for reverse in [false, true] {
        let journal = Journal::new(Some("America/Denver"));
        let mut files = vec![
            journal.save("20261020", "163000_300", &[&added]),
            journal.save("20261020", "163000_300", &[&changed]),
            journal.save("20261020", "163000_300", &[&removed]),
        ];
        // Without the removal, the change outranks the add in either order.
        let removal = files.pop().unwrap();
        if reverse {
            files.reverse();
        }
        for file in &files {
            journal.index(file);
        }
        assert_eq!(journal.listed(STANDUP), ["20261027"], "reverse {reverse}");
        journal.index(&removal);
        assert!(journal.listed(STANDUP).is_empty(), "reverse {reverse}");
    }
    // Indexed first, the removal still outranks both.
    let journal = Journal::new(Some("America/Denver"));
    let removal = journal.save("20261020", "163000_300", &[&removed]);
    journal.index(&removal);
    for line in [&changed, &added] {
        let file = journal.save("20261020", "163000_300", &[line]);
        journal.index(&file);
    }
    assert!(journal.listed(STANDUP).is_empty());

    // Two changes at the same instant: the greater file name wins, whichever
    // is indexed last.
    let first = timed(
        STANDUP,
        "changed",
        at,
        "2026-11-02T10:00:00-07:00",
        "2026-11-02T11:00:00-07:00",
    );
    let second = timed(
        STANDUP,
        "changed",
        at,
        "2026-11-03T10:00:00-07:00",
        "2026-11-03T11:00:00-07:00",
    );
    let mut winners = BTreeSet::new();
    for reverse in [false, true] {
        let journal = Journal::new(Some("America/Denver"));
        let mut files = vec![
            journal.save("20261020", "163000_300", &[&first]),
            journal.save("20261020", "163000_300", &[&second]),
        ];
        let expected = if files[0] > files[1] {
            "20261102"
        } else {
            "20261103"
        };
        if reverse {
            files.reverse();
        }
        for file in &files {
            journal.index(file);
        }
        assert_eq!(journal.listed(STANDUP), [expected]);
        winners.insert(journal.listing()[0].1.clone());
    }
    assert_eq!(winners.len(), 1);
}

#[test]
fn identities_compare_normalized_occurrences() {
    let journal = Journal::new(Some("America/Denver"));
    let spellings = [
        (
            "2026-10-21T15:00:00Z",
            "2026-10-20T16:30:00-06:00",
            "2026-10-21T09:30:00-06:00",
        ),
        (
            "2026-10-21T15:00:00.000Z",
            "2026-10-20T17:30:00-06:00",
            "2026-10-22T09:30:00-06:00",
        ),
        (
            "2026-10-21T15:00:00+00:00",
            "2026-10-20T18:30:00-06:00",
            "2026-10-23T09:30:00-06:00",
        ),
    ];
    for (occurrence, changed_at, start) in spellings {
        let line = note(
            1,
            json!({"occurrence": occurrence, "changed_at": changed_at, "start": start, "end": start}),
        );
        let segment = format!("{}000_300", changed_at[11..15].replace(':', ""));
        let file = journal.save("20261020", &segment, &[&line]);
        journal.index(&file);
    }
    assert_eq!(journal.listed(STANDUP), ["20261023"]);
    assert_eq!(journal.listing().len(), 1);
    assert_eq!(
        journal.count("SELECT count(DISTINCT occurrence) FROM calendar_notes"),
        1
    );
}

#[test]
fn golden_line_two_after_its_nine_o_clock_version_lists_once_at_half_past() {
    let journal = Journal::new(Some("America/Denver"));
    let nine = note(
        1,
        json!({"change": "added", "changed_at": "2026-10-19T09:43:00-06:00",
               "start": "2026-10-21T09:00:00-06:00", "end": "2026-10-21T09:15:00-06:00"}),
    );
    let earlier = journal.save("20261019", "094000_300", &[&nine]);
    let later = journal.save("20261020", "163000_300", &[golden(1)]);
    journal.index(&earlier);
    assert_eq!(journal.listed(STANDUP), ["20261021"]);
    assert!(journal.listing()[0].4.contains("9:00"));
    journal.index(&later);
    let listing = journal.listing();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].0, "20261021");
    assert_eq!(listing[0].1, later);
    assert!(listing[0].4.contains("9:30"), "{}", listing[0].4);
    assert!(!listing[0].4.contains("9:00"));
}

#[test]
fn an_occurrence_moved_to_another_day_leaves_its_old_day() {
    let journal = Journal::new(Some("America/Denver"));
    let first = note(
        1,
        json!({"occurrence": "2026-10-28T15:00:00Z", "change": "added",
               "changed_at": "2026-10-19T09:43:00-06:00",
               "start": "2026-10-28T09:00:00-06:00", "end": "2026-10-28T09:15:00-06:00"}),
    );
    let moved = note(
        1,
        json!({"occurrence": "2026-10-28T15:00:00Z", "changed_at": "2026-10-20T16:31:00-06:00",
               "start": "2026-10-30T13:00:00-06:00", "end": "2026-10-30T13:15:00-06:00"}),
    );
    let first = journal.save("20261019", "094000_300", &[&first]);
    journal.index(&first);
    assert_eq!(journal.listed(STANDUP), ["20261028"]);
    let moved = journal.save("20261020", "163000_300", &[&moved]);
    journal.index(&moved);
    assert_eq!(journal.listed(STANDUP), ["20261030"]);
    assert_eq!(journal.listing()[0].3, "afternoon");
}

#[test]
fn a_segment_delete_falls_back_to_the_latest_surviving_note_in_both_orders() {
    for reverse in [false, true] {
        let journal = Journal::new(Some("America/Denver"));
        let added = journal.save("20261019", "094000_300", &[golden(0)]);
        let moved = timed(
            MEETING,
            "changed",
            "2026-10-20T08:00:00-06:00",
            "2026-10-28T14:00:00-06:00",
            "2026-10-28T15:00:00-06:00",
        );
        let moved = journal.save("20261020", "080000_300", &[&moved]);
        let mut order = [&added, &moved];
        if reverse {
            order.reverse();
        }
        for file in order {
            journal.index(file);
        }
        assert_eq!(journal.listed(MEETING), ["20261028"]);
        // Deleting a segment holding only an older note changes nothing.
        let older = timed(
            MEETING,
            "added",
            "2026-10-18T10:00:00-06:00",
            "2026-10-26T10:00:00-06:00",
            "2026-10-26T11:00:00-06:00",
        );
        let older = journal.save("20261018", "100000_300", &[&older]);
        journal.index(&older);
        let before = journal.listing();
        journal.delete_segment_of(&older);
        assert_eq!(journal.listing(), before);
        // Deleting the latest note's segment falls back to the one before it.
        journal.delete_segment_of(&moved);
        assert_eq!(journal.listed(MEETING), ["20261027"], "reverse {reverse}");
        assert_eq!(journal.listing()[0].1, added);
        // With no note left, the event is dropped.
        journal.delete_segment_of(&added);
        assert!(journal.listed(MEETING).is_empty());
        assert_eq!(journal.count("SELECT count(*) FROM calendar_notes"), 0);
    }
}

#[test]
fn a_cancelled_meeting_comes_back_when_its_removal_is_deleted() {
    let journal = Journal::new(Some("America/Denver"));
    let added = journal.save("20261019", "094000_300", &[golden(0)]);
    let removed = journal.save("20261022", "080500_300", &[golden(4)]);
    journal.index(&removed);
    journal.index(&added);
    assert!(journal.listed(MEETING).is_empty());
    journal.delete_segment_of(&removed);
    assert_eq!(journal.listed(MEETING), ["20261027"]);
}

#[test]
fn a_deleted_note_leaves_no_text_in_any_index_table() {
    let journal = Journal::new(Some("America/Denver"));
    let kept = timed(
        MEETING,
        "added",
        "2026-10-19T09:42:00-06:00",
        "2026-10-27T10:00:00-06:00",
        "2026-10-27T11:00:00-06:00",
    );
    let kept = journal.save("20261019", "094000_300", &[&kept]);
    let secret = note(
        0,
        json!({"changed_at": "2026-10-20T08:00:00-06:00", "title": "quokka summit",
               "notes": "bring the narwhal ledger", "location": "zeppelin hangar",
               "start": "2026-10-28T14:00:00-06:00", "end": "2026-10-28T15:00:00-06:00"}),
    );
    let secret = journal.save("20261020", "080000_300", &[&secret]);
    journal.index(&kept);
    journal.index(&secret);
    let words = ["quokka", "narwhal", "zeppelin"];
    let holding = |journal: &Journal| -> Vec<String> {
        let conn = journal.conn();
        let tables = {
            let mut statement = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'chunks%' AND name NOT LIKE 'sqlite_%'")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let mut holding = Vec::new();
        for table in tables {
            let mut statement = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for column in 0..columns {
                    let text = match row.get_ref(column).unwrap() {
                        rusqlite::types::ValueRef::Text(bytes) => {
                            String::from_utf8_lossy(bytes).into_owned()
                        }
                        rusqlite::types::ValueRef::Blob(bytes) => {
                            String::from_utf8_lossy(bytes).into_owned()
                        }
                        _ => continue,
                    };
                    if words.iter().any(|word| text.contains(word)) {
                        holding.push(table.clone());
                    }
                }
            }
        }
        holding
    };
    let matches = |journal: &Journal| -> i64 {
        journal
            .count("SELECT count(*) FROM chunks WHERE chunks MATCH 'quokka OR narwhal OR zeppelin'")
    };
    // The instrument sees the text while it is there.
    assert!(!holding(&journal).is_empty());
    assert!(matches(&journal) >= 2);
    journal.delete_segment_of(&secret);
    assert_eq!(holding(&journal), Vec::<String>::new());
    assert_eq!(matches(&journal), 0);
    assert_eq!(journal.listed(MEETING), ["20261027"]);
}

#[test]
fn an_interrupted_supersede_keeps_the_older_listing_and_a_retry_lists_the_newer() {
    let journal = Journal::new(Some("America/Denver"));
    let older = journal.save("20261019", "094000_300", &[golden(0)]);
    journal.index(&older);
    let newer = timed(
        MEETING,
        "changed",
        "2026-10-20T08:00:00-06:00",
        "2026-10-28T14:00:00-06:00",
        "2026-10-28T15:00:00-06:00",
    );
    let newer = journal.save("20261020", "080000_300", &[&newer]);
    let before = journal.listing();
    let interrupted =
        interrupted_after_first_write(|| rescan_file(journal.path(), Path::new(&newer)));
    assert!(interrupted.is_err());
    assert_eq!(journal.listing(), before);
    assert_eq!(journal.listed(MEETING), ["20261027"]);
    assert_eq!(
        journal.count("SELECT count(*) FROM calendar_notes WHERE path LIKE '20261020/%'"),
        0
    );
    journal.index(&newer);
    assert_eq!(journal.listed(MEETING), ["20261028"]);
    assert_eq!(journal.listing().len(), 1);
}

#[test]
fn an_unreadable_settings_file_keeps_the_listing_and_reports_once_until_it_is_fixed() {
    let journal = Journal::new(Some("America/Denver"));
    let older = journal.save("20261019", "094000_300", &[golden(0)]);
    scan_journal(journal.path(), true).unwrap();
    assert_eq!(journal.listed(MEETING), ["20261027"]);
    let before = journal.listing();
    let settings = journal.path().join("config/journal.json");
    fs::write(&settings, r#"{"identity":{"timezone":NaN}}"#).unwrap();
    let newer = timed(
        MEETING,
        "changed",
        "2026-10-20T08:00:00-06:00",
        "2026-10-28T14:00:00-06:00",
        "2026-10-28T15:00:00-06:00",
    );
    let newer = journal.save("20261020", "080000_300", &[&newer]);
    assert!(rescan_file(journal.path(), Path::new(&newer)).is_err());
    let report = scan_journal(journal.path(), false).unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].starts_with("your settings file"));
    assert_eq!(journal.listing(), before);
    // Once the file reads again, the next scan lists the newer note with no
    // other action.
    fs::write(&settings, r#"{"identity":{"timezone":"America/Denver"}}"#).unwrap();
    scan_journal(journal.path(), false).unwrap();
    assert_eq!(journal.listed(MEETING), ["20261028"]);
    assert_eq!(journal.listing()[0].1, newer);
    assert_ne!(older, newer);
}

#[test]
fn with_no_zone_configured_timed_events_use_the_fallback_zone() {
    let journal = Journal::new(None);
    let late = timed(
        MEETING,
        "added",
        "2026-10-19T09:42:00-06:00",
        "2026-10-27T23:30:00Z",
        "2026-10-28T00:30:00Z",
    );
    let early = timed(
        STANDUP,
        "added",
        "2026-10-19T09:43:00-06:00",
        "2026-10-28T03:00:00Z",
        "2026-10-28T03:30:00Z",
    );
    let file = journal.save("20261019", "094000_300", &[&late, &early]);
    journal.index(&file);
    let zone = solstone_core_journal_config::host_zone();
    for (line, event) in [(&late, MEETING), (&early, STANDUP)] {
        let schedule = solstone_core_format::calendar::parse_calendar_note(line)
            .unwrap()
            .schedule
            .unwrap();
        let expected = solstone_core_format::calendar::listed_days(&schedule, &zone)
            .days
            .iter()
            .map(|day| day.format("%Y%m%d").to_string())
            .collect::<Vec<_>>();
        assert_eq!(journal.listed(event), expected, "{event} in {zone}");
    }
}

#[test]
fn day_rules_buckets_and_the_366_day_bound_reach_the_listing() {
    let journal = Journal::new(Some("America/Denver"));
    let overnight = timed(
        "overnight",
        "added",
        "2026-10-19T09:42:00-06:00",
        "2026-10-27T14:00:00-06:00",
        "2026-10-29T00:00:00-06:00",
    );
    let open = note(
        2,
        json!({"event": "open-ended", "changed_at": "2026-10-19T09:42:00-06:00", "start": "2026-12-24", "end": null}),
    );
    let long = note(
        2,
        json!({"event": "sabbatical", "changed_at": "2026-10-19T09:42:00-06:00", "start": "2026-01-01", "end": "2028-01-01"}),
    );
    let file = journal.save("20261019", "094000_300", &[&overnight, &open, &long]);
    journal.index(&file);
    let listing = journal.listing();
    assert_eq!(journal.listed("overnight"), ["20261027", "20261028"]);
    let buckets = listing
        .iter()
        .filter(|row| row.4.contains("overnight"))
        .map(|row| row.3.as_str())
        .collect::<Vec<_>>();
    assert_eq!(buckets, ["afternoon", ""]);
    assert_eq!(journal.listed("open-ended"), ["20261224"]);
    assert_eq!(journal.listed("sabbatical").len(), 366);
    let continuing =
        journal.count("SELECT count(*) FROM calendar_hits WHERE version='latest' AND continues=1");
    assert_eq!(continuing, 366);
}

#[test]
fn a_full_rebuild_from_stored_notes_yields_the_same_listing() {
    let journal = Journal::new(Some("America/Denver"));
    for (day, segment, line) in [
        ("20261019", "094000_300", golden(0).to_owned()),
        ("20261020", "163000_300", golden(1).to_owned()),
        ("20261022", "080500_300", golden(4).to_owned()),
    ] {
        let file = journal.save(day, segment, &[&line]);
        journal.index(&file);
    }
    let incremental = journal.listing();
    reset_index(journal.path()).unwrap();
    scan_journal(journal.path(), true).unwrap();
    assert_eq!(journal.listing(), incremental);
    assert_eq!(journal.listed(STANDUP), ["20261021"]);
}

#[test]
fn notes_are_read_only_from_calendar_streams() {
    let journal = Journal::new(Some("America/Denver"));
    let elsewhere = journal.save_in(CAPTURE, "20261019", "094000_300", &[golden(0)]);
    let calendar = journal.save("20261020", "163000_300", &[golden(2)]);
    journal.index(&elsewhere);
    journal.index(&calendar);
    assert_eq!(journal.stored().len(), 1);
    assert!(journal.listed(MEETING).is_empty());
    assert_eq!(
        journal.listed("48fa6175-2399-3bac-960a-fff947563ba7"),
        ["20261031"]
    );
}

#[test]
fn every_calendar_row_is_classified_for_connections() {
    let journal = Journal::new(Some("America/Denver"));
    journal.capture("20261027", "090000_300", "an ordinary morning");
    let file = journal.save("20261019", "094000_300", &[golden(0)]);
    scan_journal(journal.path(), true).unwrap();
    assert_eq!(journal.listed(MEETING), ["20261027"]);
    assert_eq!(
        journal.count(
            "SELECT count(*) FROM chunks c LEFT JOIN chunk_classification cc ON cc.path=c.path WHERE cc.path IS NULL"
        ),
        0
    );
    let (category, basis): (String, String) = journal
        .conn()
        .query_row(
            "SELECT category, basis FROM chunk_classification WHERE path=?",
            params![file],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (category.as_str(), basis.as_str()),
        ("transcripts", "journal_wide")
    );
    assert_eq!(
        journal.count("SELECT count(*) FROM chunk_classification WHERE unclassified=1"),
        0
    );
}

#[test]
fn an_unreadable_settings_file_never_empties_the_listing_on_delete() {
    // Through a segment delete, and through a scan that finds the file gone.
    for through_scan in [false, true] {
        let journal = Journal::new(Some("America/Denver"));
        let older = timed(
            MEETING,
            "added",
            "2026-10-18T10:00:00-06:00",
            "2026-10-26T10:00:00-06:00",
            "2026-10-26T11:00:00-06:00",
        );
        let older = journal.save("20261018", "100000_300", &[&older]);
        let newer = journal.save("20261019", "094000_300", &[golden(0)]);
        journal.index(&older);
        journal.index(&newer);
        assert_eq!(journal.listed(MEETING), ["20261027"]);
        let settings = journal.path().join("config/journal.json");
        fs::write(&settings, r#"{"identity":{"timezone":NaN}}"#).unwrap();
        if through_scan {
            fs::remove_dir_all(journal.path().join("chronicle/20261018")).unwrap();
            scan_journal(journal.path(), true).unwrap();
        } else {
            journal.delete_segment_of(&older);
        }
        assert_eq!(journal.listed(MEETING), ["20261027"], "scan {through_scan}");
    }
}

#[test]
fn a_fallback_that_needs_the_zone_waits_for_the_settings_file_and_is_then_listed() {
    let journal = Journal::new(Some("America/Denver"));
    let older = timed(
        MEETING,
        "added",
        "2026-10-18T10:00:00-06:00",
        "2026-10-26T10:00:00-06:00",
        "2026-10-26T11:00:00-06:00",
    );
    let older = journal.save("20261018", "100000_300", &[&older]);
    let newer = journal.save("20261019", "094000_300", &[golden(0)]);
    journal.index(&older);
    journal.index(&newer);
    let settings = journal.path().join("config/journal.json");
    fs::write(&settings, r#"{"identity":{"timezone":NaN}}"#).unwrap();
    // The newer note's segment goes; its rows go with it, and the older note
    // cannot be listed until the zone can be read.
    journal.delete_segment_of(&newer);
    assert!(journal.listed(MEETING).is_empty());
    fs::write(&settings, r#"{"identity":{"timezone":"America/Denver"}}"#).unwrap();
    scan_journal(journal.path(), false).unwrap();
    assert_eq!(journal.listed(MEETING), ["20261026"]);
}

#[test]
fn a_late_older_note_leaves_the_listing_hit_and_its_row_id_unchanged() {
    let journal = Journal::new(Some("America/Denver"));
    let newer = journal.save("20261019", "094000_300", &[golden(0)]);
    journal.index(&newer);
    let row_id = |journal: &Journal| -> i64 {
        journal.count("SELECT rowid FROM calendar_hits WHERE version='latest'")
    };
    let before = row_id(&journal);
    let older = timed(
        MEETING,
        "added",
        "2026-10-18T10:00:00-06:00",
        "2026-10-26T10:00:00-06:00",
        "2026-10-26T11:00:00-06:00",
    );
    let older = journal.save("20261018", "100000_300", &[&older]);
    journal.index(&older);
    assert_eq!(journal.listed(MEETING), ["20261027"]);
    assert_eq!(
        journal.count("SELECT count(*) FROM calendar_hits WHERE version='latest'"),
        1
    );
    assert_eq!(row_id(&journal), before);
    // A newer note does replace it.
    let moved = timed(
        MEETING,
        "changed",
        "2026-10-20T08:00:00-06:00",
        "2026-10-28T14:00:00-06:00",
        "2026-10-28T15:00:00-06:00",
    );
    let moved = journal.save("20261020", "080000_300", &[&moved]);
    journal.index(&moved);
    assert_eq!(journal.listed(MEETING), ["20261028"]);
    assert_ne!(row_id(&journal), before);
}

const IMPORTS: &str = "import.ics";
const WEEKLY: &str = "weekly-sync";

/// One date of a weekly series as an import wrote it: placed at `changed_at`,
/// happening at 09:00 or another hour on `date` in Denver.
struct Imported<'a> {
    import: &'a str,
    imported_at: &'a str,
    changed_at: &'a str,
}

impl Imported<'_> {
    /// One date on its own, as an override writes it.
    fn date(&self, date: &str, hour: u32) -> String {
        self.line(date, hour, false)
    }

    fn line(&self, date: &str, hour: u32, expanded: bool) -> String {
        let start = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap();
        let zone: chrono_tz::Tz = "America/Denver".parse().unwrap();
        let local = start.and_local_timezone(zone).unwrap();
        // The original start: 09:00 on the series' date, whatever hour the
        // date is moved to.
        let original = start
            .date()
            .and_hms_opt(9, 0, 0)
            .unwrap()
            .and_local_timezone(zone)
            .unwrap();
        let end = local + chrono::Duration::minutes(30);
        let mut line = note(
            1,
            json!({
                "event": WEEKLY, "change": "added", "changed_at": self.changed_at,
                "occurrence": original.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                "start": local.to_rfc3339(), "end": end.to_rfc3339(),
                "repeats": "Weekly", "title": format!("sync {}", self.import),
                "import": self.import, "imported_at": self.imported_at,
            }),
        );
        if expanded {
            let mut record: serde_json::Map<String, Value> = serde_json::from_str(&line).unwrap();
            record.insert("expanded".into(), json!(true));
            line = Value::Object(record).to_string();
        }
        line
    }

    /// The dates of a series as an import expanded them from its rule.
    fn dates(&self, dates: &[&str]) -> Vec<String> {
        dates.iter().map(|date| self.line(date, 9, true)).collect()
    }
}

impl Journal {
    /// Store a calendar file an import wrote beside its transcript.
    fn save_import(&self, day: &str, segment: &str, lines: &[String]) -> String {
        let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
        self.save_in(IMPORTS, day, segment, &refs)
    }

    /// The days and files an event is listed under.
    fn listed_from(&self, event: &str) -> Vec<(String, String)> {
        let conn = self.conn();
        let mut statement = conn
            .prepare(
                "SELECT c.day, c.path FROM chunks c JOIN calendar_hits h ON h.rowid=c.rowid \
                 WHERE h.version='latest' AND h.event=? ORDER BY c.day",
            )
            .unwrap();
        statement
            .query_map([event], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
}

fn days(listed: &[(String, String)]) -> Vec<&str> {
    listed.iter().map(|(day, _)| day.as_str()).collect()
}

#[test]
fn the_import_uploaded_last_defines_a_series_whatever_its_id() {
    for (tuesdays_uploaded, wednesdays_uploaded) in [
        ("2026-10-20T12:00:00-06:00", "2026-10-22T12:00:00-06:00"),
        ("2026-10-22T12:00:00-06:00", "2026-10-20T12:00:00-06:00"),
    ] {
        for reverse in [false, true] {
            let journal = Journal::new(Some("America/Denver"));
            let tuesdays = Imported {
                import: "20261020_120000",
                imported_at: tuesdays_uploaded,
                changed_at: "2026-10-19T09:42:00-06:00",
            };
            // Its id sorts first, though it may be the later upload.
            let wednesdays = Imported {
                import: "20250101_000000",
                imported_at: wednesdays_uploaded,
                changed_at: "2026-10-21T10:00:00-06:00",
            };
            let mut files = [
                journal.save_import(
                    "20261019",
                    "094200_600",
                    &tuesdays.dates(&["2026-10-27", "2026-11-03", "2026-11-10"]),
                ),
                journal.save_import(
                    "20261021",
                    "100000_301",
                    &wednesdays.dates(&["2026-10-21", "2026-10-28", "2026-11-04", "2026-11-11"]),
                ),
            ];
            if reverse {
                files.reverse();
            }
            for file in &files {
                journal.index(file);
            }
            let expected = if wednesdays_uploaded > tuesdays_uploaded {
                vec!["20261021", "20261028", "20261104", "20261111"]
            } else {
                // The Tuesdays' import covers 10-27 onward; the Wednesday
                // before it is outside that span and still counts.
                vec!["20261021", "20261027", "20261103", "20261110"]
            };
            assert_eq!(
                days(&journal.listed_from(WEEKLY)),
                expected,
                "{tuesdays_uploaded} {reverse}"
            );
        }
    }
}

#[test]
fn deleting_the_newest_import_s_segments_lets_the_older_one_count_again() {
    for reverse in [false, true] {
        let journal = Journal::new(Some("America/Denver"));
        let older = Imported {
            import: "20261020_120000",
            imported_at: "2026-10-20T12:00:00-06:00",
            changed_at: "2026-10-19T09:42:00-06:00",
        };
        let newer = Imported {
            import: "20261022_120000",
            imported_at: "2026-10-22T12:00:00-06:00",
            changed_at: "2026-10-21T10:00:00-06:00",
        };
        // The newer export moved one date to the afternoon, an override it
        // placed in its own window.
        let moved = Imported {
            changed_at: "2026-10-21T15:00:00-06:00",
            ..newer
        };
        let older_file = journal.save_import(
            "20261019",
            "094200_600",
            &older.dates(&["2026-10-21", "2026-10-28", "2026-11-04", "2026-11-11"]),
        );
        let series = journal.save_import(
            "20261021",
            "100000_600",
            &newer.dates(&["2026-10-21", "2026-10-28", "2026-11-11"]),
        );
        let override_file =
            journal.save_import("20261021", "150000_600", &[moved.date("2026-11-04", 14)]);
        let mut files = [&older_file, &series, &override_file];
        if reverse {
            files.reverse();
        }
        for file in files {
            journal.index(file);
        }
        let listed = journal.listed_from(WEEKLY);
        assert_eq!(
            days(&listed),
            ["20261021", "20261028", "20261104", "20261111"]
        );
        assert_eq!(listed[2].1, override_file);
        assert!(listed.iter().all(|(_, path)| path != &older_file));
        // The owner deleted the one date's segment: the older import does
        // not bring that date back.
        journal.delete_segment_of(&override_file);
        assert_eq!(
            days(&journal.listed_from(WEEKLY)),
            ["20261021", "20261028", "20261111"],
            "reverse {reverse}"
        );
        // With every segment of the newer import gone, the older counts again.
        journal.delete_segment_of(&series);
        let listed = journal.listed_from(WEEKLY);
        assert_eq!(
            days(&listed),
            ["20261021", "20261028", "20261104", "20261111"]
        );
        assert!(listed.iter().all(|(_, path)| path == &older_file));
    }
}

#[test]
fn a_newer_import_holding_only_one_changed_date_leaves_the_rest_listed() {
    for reverse in [false, true] {
        let journal = Journal::new(Some("America/Denver"));
        let series = Imported {
            import: "20261020_120000",
            imported_at: "2026-10-20T12:00:00-06:00",
            changed_at: "2026-10-19T09:42:00-06:00",
        };
        let changed = Imported {
            import: "20261022_120000",
            imported_at: "2026-10-22T12:00:00-06:00",
            changed_at: "2026-10-21T10:00:00-06:00",
        };
        let series_file = journal.save_import(
            "20261019",
            "094200_600",
            &series.dates(&["2026-10-21", "2026-10-28", "2026-11-04", "2026-11-11"]),
        );
        let changed_file =
            journal.save_import("20261021", "100000_600", &[changed.date("2026-10-28", 14)]);
        let mut files = [&series_file, &changed_file];
        if reverse {
            files.reverse();
        }
        for file in files {
            journal.index(file);
        }
        assert_eq!(
            journal.listed_from(WEEKLY),
            [
                ("20261021".to_owned(), series_file.clone()),
                ("20261028".to_owned(), changed_file.clone()),
                ("20261104".to_owned(), series_file.clone()),
                ("20261111".to_owned(), series_file.clone()),
            ],
            "reverse {reverse}"
        );
    }
}

#[test]
fn every_newer_import_sets_aside_older_dates_it_covers_in_every_order() {
    let tuesdays = [
        "2026-10-27",
        "2026-11-03",
        "2026-11-10",
        "2026-11-17",
        "2026-11-24",
        "2026-12-01",
    ];
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for order in orders {
        let journal = Journal::new(Some("America/Denver"));
        let first = Imported {
            import: "20261020_120000",
            imported_at: "2026-10-20T12:00:00-06:00",
            changed_at: "2026-10-19T09:42:00-06:00",
        };
        // The series shortened to end on 11-17.
        let shortened = Imported {
            import: "20261022_120000",
            imported_at: "2026-10-22T12:00:00-06:00",
            changed_at: "2026-10-21T10:00:00-06:00",
        };
        // Then one date moved, sent on its own.
        let moved = Imported {
            import: "20261024_120000",
            imported_at: "2026-10-24T12:00:00-06:00",
            changed_at: "2026-10-23T11:00:00-06:00",
        };
        let files = [
            journal.save_import("20261019", "094200_600", &first.dates(&tuesdays)),
            journal.save_import("20261021", "100000_600", &shortened.dates(&tuesdays[..4])),
            journal.save_import("20261023", "110000_600", &[moved.date("2026-11-10", 14)]),
        ];
        for index in order {
            journal.index(&files[index]);
        }
        assert_eq!(
            journal.listed_from(WEEKLY),
            [
                ("20261027".to_owned(), files[1].clone()),
                ("20261103".to_owned(), files[1].clone()),
                ("20261110".to_owned(), files[2].clone()),
                ("20261117".to_owned(), files[1].clone()),
            ],
            "order {order:?}"
        );
        // With the shortened import's segment deleted, the first import's
        // later dates count again; the moved date still stands.
        journal.delete_segment_of(&files[1]);
        assert_eq!(
            journal.listed_from(WEEKLY),
            [
                ("20261027".to_owned(), files[0].clone()),
                ("20261103".to_owned(), files[0].clone()),
                ("20261110".to_owned(), files[2].clone()),
                ("20261117".to_owned(), files[0].clone()),
                ("20261124".to_owned(), files[0].clone()),
                ("20261201".to_owned(), files[0].clone()),
            ],
            "order {order:?}"
        );
    }
}

#[test]
fn a_narrower_newer_export_leaves_older_dates_outside_it_listed() {
    let journal = Journal::new(Some("America/Denver"));
    let older = Imported {
        import: "20261020_120000",
        imported_at: "2026-10-20T12:00:00-06:00",
        changed_at: "2026-10-19T09:42:00-06:00",
    };
    let newer = Imported {
        import: "20261101_120000",
        imported_at: "2026-11-01T12:00:00-06:00",
        changed_at: "2026-10-30T10:00:00-06:00",
    };
    let older_file = journal.save_import(
        "20261019",
        "094200_600",
        &older.dates(&["2026-10-21", "2026-10-28", "2026-11-04", "2026-11-11"]),
    );
    let newer_file = journal.save_import(
        "20261030",
        "100000_600",
        &[newer.date("2026-11-04", 11), newer.date("2026-11-11", 11)],
    );
    journal.index(&newer_file);
    journal.index(&older_file);
    let listed = journal.listed_from(WEEKLY);
    assert_eq!(
        listed,
        [
            ("20261021".to_owned(), older_file.clone()),
            ("20261028".to_owned(), older_file.clone()),
            ("20261104".to_owned(), newer_file.clone()),
            ("20261111".to_owned(), newer_file.clone()),
        ]
    );
}

#[test]
fn a_phone_note_for_a_date_the_newest_import_dropped_stays_listed() {
    let journal = Journal::new(Some("America/Denver"));
    let tuesdays = Imported {
        import: "20261020_120000",
        imported_at: "2026-10-20T12:00:00-06:00",
        changed_at: "2026-10-19T09:42:00-06:00",
    };
    let wednesdays = Imported {
        import: "20261022_120000",
        imported_at: "2026-10-22T12:00:00-06:00",
        changed_at: "2026-10-21T10:00:00-06:00",
    };
    let phone = tuesdays.date("2026-10-27", 9);
    let mut phone: serde_json::Map<String, Value> = serde_json::from_str(&phone).unwrap();
    phone.remove("import");
    phone.remove("imported_at");
    phone.insert("changed_at".into(), json!("2026-10-20T16:30:00-06:00"));
    phone.insert("title".into(), json!("sync from the phone"));
    let phone = journal.save(
        "20261020",
        "163000_300",
        &[&Value::Object(phone).to_string()],
    );
    let files = [
        journal.save_import(
            "20261019",
            "094200_600",
            &tuesdays.dates(&["2026-10-27", "2026-11-03"]),
        ),
        journal.save_import(
            "20261021",
            "100000_600",
            &wednesdays.dates(&["2026-10-21", "2026-10-28", "2026-11-04"]),
        ),
        phone.clone(),
    ];
    for file in &files {
        journal.index(file);
    }
    let listed = journal.listed_from(WEEKLY);
    assert_eq!(
        days(&listed),
        ["20261021", "20261027", "20261028", "20261104"]
    );
    assert_eq!(listed[1].1, phone);
}

#[test]
fn imported_events_are_listed_only_with_no_stored_day_hit() {
    let journal = Journal::new(Some("America/Denver"));
    let import = Imported {
        import: "20261020_120000",
        imported_at: "2026-10-20T12:00:00-06:00",
        changed_at: "2026-10-19T09:42:00-06:00",
    };
    let file = journal.save_import(
        "20261019",
        "094200_600",
        &import.dates(&["2026-10-21", "2026-10-28"]),
    );
    scan_journal(journal.path(), true).unwrap();
    assert_eq!(days(&journal.listed_from(WEEKLY)), ["20261021", "20261028"]);
    assert!(journal.stored().is_empty());
    assert_eq!(
        journal.count("SELECT count(*) FROM chunks WHERE day='20261019'"),
        0
    );
    let (basis, facets): (String, i64) = journal
        .conn()
        .query_row(
            "SELECT basis, (SELECT count(*) FROM chunks WHERE path=? AND facet<>'') FROM chunk_classification WHERE path=?",
            params![file, file],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((basis.as_str(), facets), ("journal_wide", 0));
}

#[test]
fn an_import_file_outside_its_segment_span_is_skipped_and_reported_once() {
    let journal = Journal::new(Some("America/Denver"));
    let good = Imported {
        import: "20261020_120000",
        imported_at: "2026-10-20T12:00:00-06:00",
        changed_at: "2026-10-19T09:42:00-06:00",
    };
    let utc = Imported {
        changed_at: "2026-10-19T15:42:00Z",
        ..good
    };
    let bad = journal.save_import("20261019", "094200_600", &utc.dates(&["2026-10-21"]));
    let ok = journal.save_import("20261019", "154000_600", &utc.dates(&["2026-10-28"]));
    let report = scan_journal(journal.path(), true).unwrap();
    let about = report
        .warnings
        .iter()
        .filter(|warning| warning.contains(&bad))
        .count();
    assert_eq!(about, 1, "{:?}", report.warnings);
    assert!(report.warnings.iter().all(|warning| !warning.contains(&ok)));
    assert_eq!(days(&journal.listed_from(WEEKLY)), ["20261028"]);
    let again = scan_journal(journal.path(), false).unwrap();
    assert!(
        again.warnings.iter().all(|warning| !warning.contains(&bad)),
        "{:?}",
        again.warnings
    );
}

#[test]
fn an_index_made_before_imports_were_listed_gains_their_columns() {
    let journal = Journal::new(Some("America/Denver"));
    fs::create_dir_all(db_path(journal.path()).parent().unwrap()).unwrap();
    {
        let conn = journal.conn();
        conn.execute(
            "CREATE TABLE calendar_notes (path TEXT NOT NULL, line INTEGER NOT NULL, file_name TEXT NOT NULL, stream TEXT NOT NULL, event TEXT NOT NULL, occurrence TEXT NOT NULL, changed_at_ms INTEGER NOT NULL, change_rank INTEGER NOT NULL, changed_at TEXT NOT NULL, record TEXT NOT NULL, PRIMARY KEY(path, line))",
            [],
        )
        .unwrap();
    }
    let import = Imported {
        import: "20261020_120000",
        imported_at: "2026-10-20T12:00:00-06:00",
        changed_at: "2026-10-19T09:42:00-06:00",
    };
    let file = journal.save_import("20261019", "094200_600", &import.dates(&["2026-10-21"]));
    journal.index(&file);
    assert_eq!(days(&journal.listed_from(WEEKLY)), ["20261021"]);
    assert_eq!(
        journal.count("SELECT count(*) FROM calendar_notes WHERE import='20261020_120000'"),
        1
    );
}
