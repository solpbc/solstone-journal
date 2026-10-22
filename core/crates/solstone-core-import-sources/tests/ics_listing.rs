// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Calendar files imported from `.ics` exports, saved and read by the index
//! into the listing of when each event happens.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::json;
use solstone_core_import::RegistrySource;
use solstone_core_import_sources::ics_notes::ImportStamp;
use solstone_core_import_sources::save::{self, WriteOutcome};

const GOLDEN: &str = include_str!("../../../fixtures/calendar_notes/golden.jsonl");
const DESIGN_REVIEW: &str = "6CF25C80-68D2-44D9-B52C-6CAD5BEC8C10";
const STANDUP: &str = "CCBF5C02-6D92-477A-AB25-1A2ED19E1D67";

fn golden(index: usize) -> &'static str {
    GOLDEN.lines().nth(index).unwrap()
}

struct Journal {
    root: tempfile::TempDir,
    sources: tempfile::TempDir,
}

impl Journal {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            json!({"identity": {"timezone": "America/Denver"}}).to_string(),
        )
        .unwrap();
        fs::create_dir_all(root.path().join("streams")).unwrap();
        fs::write(
            root.path().join("streams/phone_calendar.json"),
            json!({
                "name": "phone_calendar", "kind": "observer", "host": null, "platform": null,
                "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
                "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "source": "calendar",
            })
            .to_string(),
        )
        .unwrap();
        Self {
            root,
            sources: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    /// Save an export as import `id`, uploaded at `imported_at`, without
    /// indexing it.
    fn save(&self, id: &str, imported_at: &str, calendar: &str) -> WriteOutcome {
        let source = self.sources.path().join(format!("{id}.ics"));
        fs::write(&source, calendar).unwrap();
        let stamp = ImportStamp {
            id: id.to_owned(),
            imported_at: DateTime::parse_from_rfc3339(imported_at)
                .unwrap()
                .with_timezone(&Utc),
        };
        let rendered = save::render(
            RegistrySource::Ics,
            &source,
            id,
            Tz::America__Denver,
            Some(&stamp),
        )
        .unwrap()
        .unwrap();
        fs::create_dir_all(self.path().join("imports").join(id)).unwrap();
        let outcome = save::write_rendered(self.path(), Some(id), &rendered);
        assert!(outcome.error.is_none(), "{:?}", outcome.error);
        for created in &outcome.created {
            let marker = created.path.parent().unwrap().join("stream.json");
            if !marker.exists() {
                solstone_core_segment::advance_unbound_stream(
                    self.path(),
                    &created.stream,
                    &created.day,
                    &created.segment,
                    save::stream_hints(RegistrySource::Ics),
                )
                .unwrap();
            }
        }
        outcome
    }

    /// The files a save wrote or removed, as the index's to-do list hands
    /// them over.
    fn written(outcome: &WriteOutcome) -> Vec<PathBuf> {
        outcome
            .created
            .iter()
            .map(|created| created.path.clone())
            .chain(outcome.calendar_files.iter().cloned())
            .collect()
    }

    fn index(&self, paths: &[PathBuf]) {
        for path in paths {
            if path.exists() {
                solstone_core_indexer_store::scan::rescan_file(self.path(), path).unwrap();
            } else {
                let rel = path
                    .strip_prefix(self.path().join("chronicle"))
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                solstone_core_indexer_store::db::prune_by_paths(self.path(), &[&rel]).unwrap();
            }
        }
    }

    /// Save and index an import.
    fn import(&self, id: &str, imported_at: &str, calendar: &str) -> WriteOutcome {
        let outcome = self.save(id, imported_at, calendar);
        self.index(&Self::written(&outcome));
        outcome
    }

    /// Store a phone's calendar file, returning its path.
    fn phone(&self, day: &str, segment: &str, lines: &[&str]) -> PathBuf {
        let dir = self
            .path()
            .join("chronicle")
            .join(day)
            .join("phone_calendar")
            .join(segment);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("stream.json"),
            json!({"stream": "phone_calendar", "prev_day": null, "prev_segment": null, "seq": 1})
                .to_string(),
        )
        .unwrap();
        let mut bytes = lines.join("\n").into_bytes();
        bytes.push(b'\n');
        let path = dir.join(solstone_core_format::calendar::calendar_file_name(&bytes));
        fs::write(&path, bytes).unwrap();
        path
    }

    fn query<T>(
        &self,
        sql: &str,
        event: &str,
        row: impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Vec<T> {
        let conn = solstone_core_indexer_store::open_index_reader(self.path()).unwrap();
        let mut statement = conn.prepare(sql).unwrap();
        statement
            .query_map([event], row)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Each listing row of an event: (day, time of day, text).
    fn listed(&self, event: &str) -> Vec<(String, String, String)> {
        self.query(
            "SELECT c.day, c.time_bucket, c.content FROM chunks c JOIN calendar_hits h ON h.rowid=c.rowid \
             WHERE h.version='latest' AND h.event=? ORDER BY c.day, c.idx",
            event,
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
    }

    fn days(&self, event: &str) -> Vec<String> {
        let mut days = self
            .listed(event)
            .into_iter()
            .map(|(day, _, _)| day)
            .collect::<Vec<_>>();
        days.dedup();
        days
    }

    /// The occurrences an event is listed for, one row per listed day.
    fn listed_occurrences(&self, event: &str) -> Vec<String> {
        self.query(
            "SELECT occurrence FROM calendar_hits WHERE version='latest' AND event=? ORDER BY occurrence",
            event,
            |row| row.get(0),
        )
    }

    fn recorded(&self, event: &str) -> usize {
        self.query(
            "SELECT count(*) FROM calendar_hits WHERE version='recorded' AND event=?",
            event,
            |row| row.get::<_, i64>(0),
        )[0] as usize
    }

    /// Remove a segment as an owner's delete does, and tell the index.
    fn delete_segment_of(&self, path: &Path) {
        let segment = path.parent().unwrap();
        fs::remove_dir_all(segment).unwrap();
        let rel = segment
            .strip_prefix(self.path().join("chronicle"))
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        solstone_core_indexer_store::db::prune_by_paths(self.path(), &[&rel]).unwrap();
    }
}

/// A calendar file holding these events, one string of properties each.
fn calendar(events: &[&str]) -> String {
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Work\r\n");
    for event in events {
        text.push_str("BEGIN:VEVENT\r\n");
        for line in event.lines().map(str::trim).filter(|line| !line.is_empty()) {
            text.push_str(line);
            text.push_str("\r\n");
        }
        text.push_str("END:VEVENT\r\n");
    }
    text.push_str("END:VCALENDAR\r\n");
    text
}

fn weekly(start: &str, rule: &str, modified: &str) -> String {
    format!(
        "UID:weekly-sync\nSUMMARY:weekly sync\nDTSTART;TZID=America/Denver:{start}\nRRULE:{rule}\nLAST-MODIFIED:{modified}"
    )
}

const TUESDAYS: [&str; 10] = [
    "20261006", "20261013", "20261020", "20261027", "20261103", "20261110", "20261117", "20261124",
    "20261201", "20261208",
];

#[test]
fn a_newer_export_with_an_earlier_end_drops_the_dates_after_it() {
    let journal = Journal::new();
    journal.import(
        "20261005_180000",
        "2026-10-05T18:00:00Z",
        &calendar(&[&weekly(
            "20261006T090000",
            "FREQ=WEEKLY;COUNT=10",
            "20261001T150000Z",
        )]),
    );
    assert_eq!(journal.days("weekly-sync"), TUESDAYS);
    let newer = journal.import(
        "20261101_180000",
        "2026-11-01T18:00:00Z",
        &calendar(&[&weekly(
            "20261006T090000",
            "FREQ=WEEKLY;UNTIL=20261110T235959Z",
            "20261031T150000Z",
        )]),
    );
    assert_eq!(journal.days("weekly-sync"), TUESDAYS[..6]);
    // With the newer import's window deleted, the older export counts again.
    journal.delete_segment_of(&newer.calendar_files[0]);
    assert_eq!(journal.days("weekly-sync"), TUESDAYS);
}

#[test]
fn a_newer_export_of_one_changed_date_leaves_the_rest_of_the_series_listed() {
    let journal = Journal::new();
    journal.import(
        "20261005_180000",
        "2026-10-05T18:00:00Z",
        &calendar(&[&weekly(
            "20261006T090000",
            "FREQ=WEEKLY;COUNT=10",
            "20261001T150000Z",
        )]),
    );
    let moved = "UID:weekly-sync\nSUMMARY:weekly sync\nRECURRENCE-ID;TZID=America/Denver:20261020T090000\nDTSTART;TZID=America/Denver:20261020T130000\nLAST-MODIFIED:20261015T150000Z";
    journal.import(
        "20261016_180000",
        "2026-10-16T18:00:00Z",
        &calendar(&[moved]),
    );
    assert_eq!(journal.days("weekly-sync"), TUESDAYS);
    let buckets = journal
        .listed("weekly-sync")
        .into_iter()
        .map(|(day, bucket, _)| (day, bucket))
        .filter(|(_, bucket)| bucket != "morning")
        .collect::<Vec<_>>();
    assert_eq!(buckets, [("20261020".to_owned(), "afternoon".to_owned())]);
}

#[test]
fn a_single_changed_date_after_a_shortened_export_keeps_the_dropped_dates_out() {
    let journal = Journal::new();
    journal.import(
        "20261005_180000",
        "2026-10-05T18:00:00Z",
        &calendar(&[&weekly(
            "20261006T090000",
            "FREQ=WEEKLY;UNTIL=20261201T235959Z",
            "20261001T150000Z",
        )]),
    );
    let shortened = journal.import(
        "20261012_180000",
        "2026-10-12T18:00:00Z",
        &calendar(&[&weekly(
            "20261006T090000",
            "FREQ=WEEKLY;UNTIL=20261117T235959Z",
            "20261010T150000Z",
        )]),
    );
    let moved = "UID:weekly-sync\nSUMMARY:weekly sync\nRECURRENCE-ID;TZID=America/Denver:20261110T090000\nDTSTART;TZID=America/Denver:20261110T130000\nLAST-MODIFIED:20261015T150000Z";
    journal.import(
        "20261016_180000",
        "2026-10-16T18:00:00Z",
        &calendar(&[moved]),
    );
    assert_eq!(journal.days("weekly-sync"), TUESDAYS[..7]);
    // Deleting the shortened export's window brings the later dates back.
    journal.delete_segment_of(&shortened.calendar_files[0]);
    assert_eq!(journal.days("weekly-sync"), TUESDAYS[..9]);
}

#[test]
fn a_series_moved_from_tuesday_to_wednesday_leaves_its_old_tuesdays() {
    let journal = Journal::new();
    journal.import(
        "20261005_180000",
        "2026-10-05T18:00:00Z",
        &calendar(&[&weekly(
            "20261006T090000",
            "FREQ=WEEKLY;COUNT=10",
            "20261001T150000Z",
        )]),
    );
    journal.import(
        "20261101_180000",
        "2026-11-01T18:00:00Z",
        &calendar(&[&weekly(
            "20261007T090000",
            "FREQ=WEEKLY;COUNT=10",
            "20261031T150000Z",
        )]),
    );
    // The newer export covers its first Wednesday onward, so every Tuesday
    // after it leaves; 10-06 is before the moved series' first date and
    // outside what the newer export covers.
    assert_eq!(
        journal.days("weekly-sync"),
        [
            "20261006", "20261007", "20261014", "20261021", "20261028", "20261104", "20261111",
            "20261118", "20261125", "20261202", "20261209",
        ]
    );
}

#[test]
fn a_phone_change_and_an_imported_override_of_one_date_list_once() {
    let series = format!(
        "UID:{STANDUP}\nSUMMARY:standup\nDTSTART;TZID=America/Denver:20261019T090000\nDTEND;TZID=America/Denver:20261019T091500\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;COUNT=5\nLAST-MODIFIED:20261015T150000Z"
    );
    let moved = format!(
        "UID:{STANDUP}\nSUMMARY:standup\nRECURRENCE-ID;TZID=America/Denver:20261021T090000\nDTSTART;TZID=America/Denver:20261021T093000\nDTEND;TZID=America/Denver:20261021T094500\nLAST-MODIFIED:20261020T200000Z"
    );
    for phone_first in [true, false] {
        let journal = Journal::new();
        let outcome = journal.save(
            "20261021_120000",
            "2026-10-21T18:00:00Z",
            &calendar(&[&series, &moved]),
        );
        let phone = journal.phone("20261020", "163000_300", &[golden(1)]);
        let mut paths = Journal::written(&outcome);
        if phone_first {
            paths.insert(0, phone);
        } else {
            paths.push(phone);
        }
        journal.index(&paths);
        let on_the_day = journal
            .listed(STANDUP)
            .into_iter()
            .filter(|(day, _, _)| day == "20261021")
            .collect::<Vec<_>>();
        assert_eq!(on_the_day.len(), 1, "phone first {phone_first}");
        assert!(on_the_day[0].2.contains("9:30"), "{}", on_the_day[0].2);
        assert_eq!(
            journal
                .listed_occurrences(STANDUP)
                .iter()
                .filter(|occurrence| *occurrence == "2026-10-21T15:00:00Z")
                .count(),
            1
        );
        assert_eq!(journal.days(STANDUP).len(), 5);
    }
}

#[test]
fn a_phone_event_and_an_imported_one_with_its_id_list_the_later_version_once() {
    for (modified, start, expected) in [
        ("20261020T150000Z", "20261028T100000", "20261028"),
        ("20261018T150000Z", "20261029T100000", "20261027"),
    ] {
        for phone_first in [true, false] {
            let journal = Journal::new();
            let event = format!(
                "UID:{DESIGN_REVIEW}\nSUMMARY:design review\nDTSTART;TZID=America/Denver:{start}\nLAST-MODIFIED:{modified}"
            );
            let outcome = journal.save(
                "20261021_120000",
                "2026-10-21T18:00:00Z",
                &calendar(&[&event]),
            );
            let phone = journal.phone("20261019", "094000_300", &[golden(0)]);
            let mut paths = Journal::written(&outcome);
            if phone_first {
                paths.insert(0, phone);
            } else {
                paths.push(phone);
            }
            journal.index(&paths);
            assert_eq!(
                journal.days(DESIGN_REVIEW),
                [expected],
                "{modified} phone first {phone_first}"
            );
            assert_eq!(journal.listed(DESIGN_REVIEW).len(), 1);
        }
    }
}

#[test]
fn overrides_excluded_dates_cancellations_and_windows_zones_reach_the_listing() {
    let journal = Journal::new();
    let series = "
        UID:standup-1
        SUMMARY:standup
        DTSTART;TZID=America/Denver:20261020T090000
        RRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=4
        EXDATE;TZID=America/Denver:20261027T090000
        LAST-MODIFIED:20261019T154200Z
    ";
    // Confirmed, older than its series, and moved to the next afternoon.
    let moved = "
        UID:standup-1
        SUMMARY:standup
        RECURRENCE-ID;TZID=America/Denver:20261027T090000
        DTSTART;TZID=America/Denver:20261028T140000
        STATUS:CONFIRMED
        LAST-MODIFIED:20261001T120000Z
    ";
    let daily = "
        UID:sync-2
        SUMMARY:sync
        DTSTART;TZID=America/Denver:20261021T090000
        RRULE:FREQ=DAILY;COUNT=3
        LAST-MODIFIED:20261019T154300Z
    ";
    let utc_override = "
        UID:sync-2
        SUMMARY:sync
        RECURRENCE-ID:20261022T150000Z
        DTSTART;TZID=America/Denver:20261022T130000
        LAST-MODIFIED:20261019T154400Z
    ";
    let cancelled = "
        UID:gone-3
        SUMMARY:retro
        DTSTART;TZID=America/Denver:20261021T150000
        RRULE:FREQ=WEEKLY;COUNT=3
        STATUS:CANCELLED
        LAST-MODIFIED:20261019T154500Z
    ";
    let pacific = "
        UID:pacific-4
        SUMMARY:late call
        DTSTART;TZID=Pacific Standard Time:20261023T233000
        LAST-MODIFIED:20261019T154600Z
    ";
    journal.import(
        "20261020_120000",
        "2026-10-20T18:00:00Z",
        &calendar(&[series, moved, daily, utc_override, cancelled, pacific]),
    );
    let standup = journal.listed("standup-1");
    let days = standup
        .iter()
        .map(|(day, bucket, _)| (day.as_str(), bucket.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        days,
        [
            ("20261020", "morning"),
            ("20261028", "afternoon"),
            ("20261103", "morning"),
            ("20261110", "morning"),
        ]
    );
    let sync = journal.listed("sync-2");
    let buckets = sync
        .iter()
        .map(|(day, bucket, _)| (day.as_str(), bucket.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        buckets,
        [
            ("20261021", "morning"),
            ("20261022", "afternoon"),
            ("20261023", "morning"),
        ]
    );
    assert!(journal.listed("gone-3").is_empty());
    // 23:30 in Los Angeles is past midnight in Denver.
    assert_eq!(journal.days("pacific-4"), ["20261024"]);
    for event in ["standup-1", "sync-2", "pacific-4"] {
        assert_eq!(journal.recorded(event), 0, "{event}");
    }
}

#[test]
fn a_reimport_with_a_new_excluded_date_removes_it() {
    let journal = Journal::new();
    let rule = "FREQ=WEEKLY;COUNT=4";
    journal.import(
        "20261005_180000",
        "2026-10-05T18:00:00Z",
        &calendar(&[&weekly("20261006T090000", rule, "20261001T150000Z")]),
    );
    assert_eq!(journal.days("weekly-sync"), TUESDAYS[..4]);
    let excluded = format!(
        "{}\nEXDATE;TZID=America/Denver:20261020T090000",
        weekly("20261006T090000", rule, "20261012T150000Z")
    );
    journal.import(
        "20261013_180000",
        "2026-10-13T18:00:00Z",
        &calendar(&[&excluded]),
    );
    assert_eq!(
        journal.days("weekly-sync"),
        ["20261006", "20261013", "20261027"]
    );
}

#[test]
fn a_reimport_moves_an_edited_event_and_leaves_an_unchanged_one_alone() {
    let journal = Journal::new();
    let dinner = |start: &str, modified: &str| {
        format!(
            "UID:dinner\nSUMMARY:dinner\nDTSTART;TZID=America/Denver:{start}\nLAST-MODIFIED:{modified}"
        )
    };
    let lunch = "UID:lunch\nSUMMARY:lunch\nDTSTART;TZID=America/Denver:20261029T120000\nLAST-MODIFIED:20261002T150000Z";
    let first = calendar(&[&dinner("20261030T190000", "20261001T150000Z"), lunch]);
    journal.import("20261005_180000", "2026-10-05T18:00:00Z", &first);
    let before = (journal.listed("dinner"), journal.listed("lunch"));
    assert_eq!(journal.days("dinner"), ["20261030"]);
    // The same export again, a week later.
    journal.import("20261012_180000", "2026-10-12T18:00:00Z", &first);
    assert_eq!((journal.listed("dinner"), journal.listed("lunch")), before);
    // Then edited: dinner moves to the next night.
    let edited = calendar(&[&dinner("20261031T190000", "20261015T150000Z"), lunch]);
    journal.import("20261019_180000", "2026-10-19T18:00:00Z", &edited);
    assert_eq!(journal.days("dinner"), ["20261031"]);
    assert_eq!(journal.listed("lunch"), before.1);
}

#[test]
fn two_imports_sharing_a_window_keep_each_other_s_events() {
    let journal = Journal::new();
    let event = |uid: &str| {
        format!(
            "UID:{uid}\nSUMMARY:{uid}\nDTSTART;TZID=America/Denver:20261030T120000\nLAST-MODIFIED:20261019T154200Z"
        )
    };
    let first = journal.import(
        "20261020_120000",
        "2026-10-20T18:00:00Z",
        &calendar(&[&event("first")]),
    );
    let second = journal.import(
        "20261021_120000",
        "2026-10-21T18:00:00Z",
        &calendar(&[&event("second")]),
    );
    assert_eq!(
        first.calendar_files[0].parent(),
        second.calendar_files[0].parent()
    );
    assert!(first.calendar_files[0].exists());
    assert_eq!(journal.days("first"), ["20261030"]);
    assert_eq!(journal.days("second"), ["20261030"]);
}

#[test]
fn resaving_an_import_replaces_only_its_own_calendar_files() {
    let journal = Journal::new();
    let event = |title: &str| {
        format!(
            "UID:planning\nSUMMARY:{title}\nDTSTART;TZID=America/Denver:20261030T120000\nLAST-MODIFIED:20261019T154200Z"
        )
    };
    let other = "UID:other\nSUMMARY:other\nDTSTART;TZID=America/Denver:20261031T120000\nLAST-MODIFIED:20261019T154200Z";
    let first = journal.import(
        "20261020_120000",
        "2026-10-20T18:00:00Z",
        &calendar(&[&event("planning")]),
    );
    let neighbour = journal.import(
        "20261021_120000",
        "2026-10-21T18:00:00Z",
        &calendar(&[other]),
    );
    let old = first.calendar_files[0].clone();
    let segment = old.parent().unwrap().to_path_buf();
    assert_eq!(neighbour.calendar_files[0].parent().unwrap(), segment);
    // A file an interrupted earlier save of the same import left behind.
    let leftover_line = fs::read_to_string(&old)
        .unwrap()
        .replace("\"planning\"", "\"planning, draft\"");
    let leftover = segment.join(solstone_core_format::calendar::calendar_file_name(
        leftover_line.as_bytes(),
    ));
    fs::write(&leftover, &leftover_line).unwrap();
    assert_ne!(leftover, old);

    let again = journal.save(
        "20261020_120000",
        "2026-10-20T18:00:00Z",
        &calendar(&[&event("planning, final")]),
    );
    let written = again.calendar_files[0].clone();
    // The new file comes first; then the import's other files go.
    let mut pruned = again.calendar_files[1..].to_vec();
    pruned.sort();
    let mut expected = vec![old.clone(), leftover.clone()];
    expected.sort();
    assert_eq!(pruned, expected);
    assert!(written.exists() && !old.exists() && !leftover.exists());
    assert!(neighbour.calendar_files[0].exists());
    journal.index(&Journal::written(&again));
    let listed = journal.listed("planning");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].2.contains("planning, final"), "{}", listed[0].2);
    assert_eq!(journal.days("other"), ["20261031"]);
}
