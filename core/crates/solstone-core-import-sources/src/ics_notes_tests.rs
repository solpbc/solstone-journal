// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use chrono_tz::Tz;
use serde_json::{Map, Value, json};
use solstone_core_format::calendar::validate_import_calendar_file;

use super::*;
use crate::save::{RenderedImport, SegmentFile};
use crate::zip_names::fixtures::{ascii, build_zip};

const DENVER: Tz = Tz::America__Denver;

fn stamp(id: &str, at: &str) -> ImportStamp {
    ImportStamp {
        id: id.to_owned(),
        imported_at: DateTime::parse_from_rfc3339(at)
            .unwrap()
            .with_timezone(&Utc),
    }
}

fn october() -> ImportStamp {
    stamp("20261020_180000", "2026-10-20T18:00:00Z")
}

/// A calendar file holding these events, one string of properties each.
fn calendar(name: Option<&str>, events: &[&str]) -> String {
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    if let Some(name) = name {
        text.push_str(&format!("X-WR-CALNAME:{name}\r\n"));
    }
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

fn render_file(path: &Path, zone: Tz, stamp: &ImportStamp) -> RenderedImport {
    crate::ics::render_with_notes(path, &zone, Some(stamp)).unwrap()
}

fn render(text: &str, zone: Tz, stamp: &ImportStamp) -> RenderedImport {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calendar.ics");
    std::fs::write(&path, text).unwrap();
    render_file(&path, zone, stamp)
}

fn render_zip(members: &[(&str, &str)], zone: Tz, stamp: &ImportStamp) -> RenderedImport {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export.zip");
    let members = members
        .iter()
        .map(|(name, text)| ascii(name, text.as_bytes()))
        .collect::<Vec<_>>();
    std::fs::write(&path, build_zip(&members)).unwrap();
    render_file(&path, zone, stamp)
}

fn calendar_files(rendered: &RenderedImport) -> Vec<&SegmentFile> {
    rendered.files.iter().filter(|file| file.calendar).collect()
}

/// Every calendar note line the import wrote, in file order.
fn notes_of(rendered: &RenderedImport) -> Vec<Map<String, Value>> {
    calendar_files(rendered)
        .iter()
        .flat_map(|file| {
            file.contents
                .lines()
                .map(|line| serde_json::from_str::<Map<String, Value>>(line).unwrap())
        })
        .collect()
}

fn of_event<'a>(lines: &'a [Map<String, Value>], event: &str) -> Vec<&'a Map<String, Value>> {
    lines.iter().filter(|line| line["event"] == event).collect()
}

fn occurrences(lines: &[&Map<String, Value>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line["occurrence"].as_str().unwrap_or("").to_owned())
        .collect()
}

fn field<'a>(lines: &'a [&Map<String, Value>], key: &str) -> Vec<&'a Value> {
    lines.iter().map(|line| &line[key]).collect()
}

/// Every calendar file is valid against its own segment's span.
fn assert_valid(rendered: &RenderedImport) {
    for file in calendar_files(rendered) {
        validate_import_calendar_file(
            &file.name,
            file.contents.as_bytes(),
            &file.day,
            &file.segment,
        )
        .unwrap_or_else(|error| panic!("{} {}: {error:?}", file.day, file.segment));
    }
}

const WEEKLY_STANDUP: &str = "
    UID:standup-1
    SUMMARY:standup
    DTSTART;TZID=America/Denver:20261020T090000
    DTEND;TZID=America/Denver:20261020T091500
    RRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=4
    LAST-MODIFIED:20261019T154200Z
";

#[test]
fn identity_comes_from_uid_and_recurrence_id() {
    let override_text = "
        UID:standup-1
        SUMMARY:standup, moved
        RECURRENCE-ID;TZID=America/Denver:20261027T090000
        DTSTART;TZID=America/Denver:20261027T100000
        DTEND;TZID=America/Denver:20261027T101500
        LAST-MODIFIED:20261019T160000Z
    ";
    let one_off = "
        UID:review-7
        SUMMARY:design review
        DTSTART:20261023T160000Z
        DTEND:20261023T170000Z
        LAST-MODIFIED:20261019T170000Z
    ";
    let rendered = render(
        &calendar(Some("Work"), &[WEEKLY_STANDUP, override_text, one_off]),
        DENVER,
        &october(),
    );
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    let standup = of_event(&lines, "standup-1");
    let mut written = occurrences(&standup);
    written.sort();
    assert_eq!(
        written,
        [
            "2026-10-20T15:00:00Z",
            "2026-10-27T15:00:00Z",
            "2026-11-03T16:00:00Z",
            "2026-11-10T16:00:00Z",
        ]
    );
    // The override stands for its occurrence: its own start and title.
    let moved = standup
        .iter()
        .find(|line| line["occurrence"] == "2026-10-27T15:00:00Z")
        .unwrap();
    assert_eq!(moved["start"], "2026-10-27T10:00:00-06:00");
    assert_eq!(moved["title"], "standup, moved");
    let review = of_event(&lines, "review-7");
    assert_eq!(review.len(), 1);
    assert_eq!(review[0]["occurrence"], Value::Null);
}

#[test]
fn an_event_with_no_uid_keeps_one_identity_across_imports() {
    let first = "
        SUMMARY:dentist
        DTSTART;TZID=America/Denver:20261110T080000
        CREATED:20261001T120000Z
    ";
    let second = "
        SUMMARY:haircut
        DTSTART;TZID=America/Denver:20261112T080000
        CREATED:20261001T120000Z
    ";
    let text = calendar(None, &[first, second]);
    let events = |stamp: &ImportStamp| {
        notes_of(&render(&text, DENVER, stamp))
            .iter()
            .map(|line| line["event"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let once = events(&october());
    let again = events(&stamp("20261101_120000", "2026-11-01T12:00:00Z"));
    assert_eq!(once.len(), 2);
    assert_ne!(once[0], once[1]);
    assert!(once.iter().all(|event| event.starts_with("ics-")));
    let mut sorted = again.clone();
    sorted.sort();
    let mut expected = once.clone();
    expected.sort();
    assert_eq!(sorted, expected);
}

#[test]
fn fields_map_from_the_file() {
    let meeting = "
        UID:meeting-1
        SUMMARY:planning
        LOCATION:room 4
        DESCRIPTION:bring the mocks
        URL:https://example.com/plan
        DTSTART;TZID=America/Denver:20261022T100000
        DTEND;TZID=America/Denver:20261022T110000
        ORGANIZER;CN=Alex Rivera:mailto:Alex@Example.com
        ATTENDEE;CN=Alex Rivera;PARTSTAT=ACCEPTED:mailto:alex@example.com
        ATTENDEE;PARTSTAT=DECLINED:mailto:dana@example.com
        ATTENDEE;PARTSTAT=TENTATIVE:mailto:eli@example.com
        ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:fay@example.com
        ATTENDEE;PARTSTAT=DELEGATED:mailto:gus@example.com
        LAST-MODIFIED:20261019T154200Z
    ";
    let utc = "
        UID:call-2
        SUMMARY:call
        DTSTART:20261023T160000Z
        LAST-MODIFIED:20261019T154300Z
    ";
    let all_day = "
        UID:holiday-3
        SUMMARY:Halloween
        DTSTART;VALUE=DATE:20261031
        DTEND;VALUE=DATE:20261101
        LAST-MODIFIED:20261019T154400Z
    ";
    let floating = "
        UID:walk-4
        SUMMARY:walk
        DTSTART:20261024T070000
        RRULE:FREQ=DAILY;COUNT=2
        LAST-MODIFIED:20261019T154500Z
    ";
    let rendered = render(
        &calendar(Some("Work"), &[meeting, utc, all_day, floating]),
        DENVER,
        &october(),
    );
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    let meeting = of_event(&lines, "meeting-1")[0];
    assert_eq!(meeting["calendar"], "Work");
    assert_eq!(meeting["location"], "room 4");
    assert_eq!(meeting["notes"], "bring the mocks");
    assert_eq!(meeting["url"], "https://example.com/plan");
    assert_eq!(
        meeting["organizer"],
        json!({"email": "alex@example.com", "name": "Alex Rivera"})
    );
    let statuses = meeting["attendees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|person| {
            (
                person["email"].as_str().unwrap(),
                person["status"].as_str().unwrap(),
                person["you"].clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        [
            ("alex@example.com", "accepted", json!(false)),
            ("dana@example.com", "declined", json!(false)),
            ("eli@example.com", "tentative", json!(false)),
            ("fay@example.com", "pending", json!(false)),
            ("gus@example.com", "unknown", json!(false)),
        ]
    );
    let zones = ["meeting-1", "call-2", "holiday-3", "walk-4"]
        .map(|event| of_event(&lines, event)[0]["time_zone"].clone());
    assert_eq!(
        zones,
        [
            json!("America/Denver"),
            json!("UTC"),
            Value::Null,
            Value::Null
        ]
    );
    let starts = ["meeting-1", "call-2", "holiday-3", "walk-4"]
        .map(|event| of_event(&lines, event)[0]["start"].clone());
    assert_eq!(
        starts,
        [
            json!("2026-10-22T10:00:00-06:00"),
            json!("2026-10-23T16:00:00Z"),
            json!("2026-10-31"),
            json!("2026-10-24T07:00:00"),
        ]
    );
    let walks = of_event(&lines, "walk-4");
    assert_eq!(
        occurrences(&walks),
        ["2026-10-24T07:00:00", "2026-10-25T07:00:00"]
    );
    assert_eq!(meeting["repeats"], Value::Null);
    assert!(walks[0]["repeats"].as_str().is_some());
    assert_eq!(of_event(&lines, "holiday-3")[0]["all_day"], true);
    assert_eq!(meeting["all_day"], false);
    for line in &lines {
        assert_eq!(line["import"], "20261020_180000");
        assert_eq!(line["imported_at"], "2026-10-20T12:00:00-06:00");
        assert_eq!(line["change"], "added");
    }
}

#[test]
fn the_calendar_name_comes_from_the_file_then_the_export_member() {
    let event = |uid: &str| {
        format!(
            "UID:{uid}\nSUMMARY:{uid}\nDTSTART:20261023T160000Z\nLAST-MODIFIED:20261019T154200Z"
        )
    };
    let rendered = render_zip(
        &[
            ("Family.ics", &calendar(None, &[&event("a")])),
            ("Team.ics", &calendar(Some("Work"), &[&event("b")])),
        ],
        DENVER,
        &october(),
    );
    let lines = notes_of(&rendered);
    assert_eq!(of_event(&lines, "a")[0]["calendar"], "Family");
    assert_eq!(of_event(&lines, "b")[0]["calendar"], "Work");
    let rendered = render(&calendar(None, &[&event("c")]), DENVER, &october());
    assert_eq!(notes_of(&rendered)[0]["calendar"], Value::Null);
}

#[test]
fn each_rule_shape_expands() {
    let cases = [
        (
            "FREQ=DAILY;COUNT=3",
            vec![
                "2026-10-20T15:00:00Z",
                "2026-10-21T15:00:00Z",
                "2026-10-22T15:00:00Z",
            ],
        ),
        (
            "FREQ=WEEKLY;BYDAY=TU,TH;COUNT=3",
            vec![
                "2026-10-20T15:00:00Z",
                "2026-10-22T15:00:00Z",
                "2026-10-27T15:00:00Z",
            ],
        ),
        (
            "FREQ=MONTHLY;BYMONTHDAY=20;COUNT=3",
            vec![
                "2026-10-20T15:00:00Z",
                "2026-11-20T16:00:00Z",
                "2026-12-20T16:00:00Z",
            ],
        ),
        (
            "FREQ=MONTHLY;BYDAY=3TU;COUNT=3",
            vec![
                "2026-10-20T15:00:00Z",
                "2026-11-17T16:00:00Z",
                "2026-12-15T16:00:00Z",
            ],
        ),
        (
            "FREQ=YEARLY;COUNT=2",
            vec!["2026-10-20T15:00:00Z", "2027-10-20T15:00:00Z"],
        ),
        (
            "FREQ=DAILY;INTERVAL=10;UNTIL=20261115T000000Z",
            vec![
                "2026-10-20T15:00:00Z",
                "2026-10-30T15:00:00Z",
                "2026-11-09T16:00:00Z",
            ],
        ),
    ];
    for (rule, expected) in cases {
        let event = format!(
            "UID:series\nSUMMARY:series\nDTSTART;TZID=America/Denver:20261020T090000\nRRULE:{rule}\nLAST-MODIFIED:20261019T154200Z"
        );
        let rendered = render(&calendar(None, &[&event]), DENVER, &october());
        assert_valid(&rendered);
        let lines = notes_of(&rendered);
        assert_eq!(occurrences(&of_event(&lines, "series")), expected, "{rule}");
        assert_eq!(rendered.calendar_counts, CalendarNoteCounts::default());
    }
}

#[test]
fn a_weekly_series_keeps_its_local_time_across_the_daylight_saving_change() {
    let rendered = render(&calendar(None, &[WEEKLY_STANDUP]), DENVER, &october());
    let lines = notes_of(&rendered);
    let standup = of_event(&lines, "standup-1");
    assert_eq!(
        field(&standup, "start"),
        [
            "2026-10-20T09:00:00-06:00",
            "2026-10-27T09:00:00-06:00",
            "2026-11-03T09:00:00-07:00",
            "2026-11-10T09:00:00-07:00",
        ]
    );
    assert_eq!(
        field(&standup, "end"),
        [
            "2026-10-20T09:15:00-06:00",
            "2026-10-27T09:15:00-06:00",
            "2026-11-03T09:15:00-07:00",
            "2026-11-10T09:15:00-07:00",
        ]
    );
    assert_eq!(
        occurrences(&standup)[1..3],
        ["2026-10-27T15:00:00Z", "2026-11-03T16:00:00Z"]
    );
}

#[cfg(all(test, feature = "full-tests"))]
/// The bytes of a calendar import whose expansion crosses a daylight-saving
/// change, holding floating and all-day series too.
fn zone_sensitive_bytes() -> Vec<String> {
    let floating = "
        UID:walk
        SUMMARY:walk
        DTSTART:20261024T070000
        RRULE:FREQ=WEEKLY;COUNT=3
        LAST-MODIFIED:20261019T154500Z
    ";
    let all_day = "
        UID:bins
        SUMMARY:bins
        DTSTART;VALUE=DATE:20261026
        RRULE:FREQ=WEEKLY;COUNT=3
        LAST-MODIFIED:20261019T154600Z
    ";
    let rendered = render(
        &calendar(None, &[WEEKLY_STANDUP, floating, all_day]),
        DENVER,
        &october(),
    );
    calendar_files(&rendered)
        .iter()
        .map(|file| {
            format!(
                "{}/{}/{}\n{}",
                file.day, file.segment, file.name, file.contents
            )
        })
        .collect()
}

/// Run by the test below in a child process with a different `TZ`.
#[cfg(all(test, feature = "full-tests"))]
#[test]
fn zone_sensitive_bytes_for_a_child_process() {
    if let Some(out) = std::env::var_os("SOLSTONE_CALENDAR_BYTES_OUT") {
        std::fs::write(out, zone_sensitive_bytes().join("\u{1e}")).unwrap();
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[test]
fn expansion_bytes_do_not_depend_on_the_process_time_zone() {
    let dir = tempfile::tempdir().unwrap();
    let here = zone_sensitive_bytes().join("\u{1e}");
    assert!(here.contains("2026-11-03T09:00:00-07:00"));
    for zone in ["Asia/Tokyo", "Pacific/Auckland", "America/New_York", "UTC"] {
        let out = dir.path().join(zone.replace('/', "_"));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ics_notes::tests::zone_sensitive_bytes_for_a_child_process",
                "--test-threads=1",
            ])
            .env("TZ", zone)
            .env("SOLSTONE_CALENDAR_BYTES_OUT", &out)
            .status()
            .unwrap();
        assert!(status.success(), "{zone}");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), here, "{zone}");
    }
}

#[test]
fn a_long_daily_series_keeps_its_most_recent_dates_into_the_next_year() {
    let event = "
        UID:daily
        SUMMARY:pills
        DTSTART;TZID=America/Denver:20101020T080000
        RRULE:FREQ=DAILY
        LAST-MODIFIED:20261019T154200Z
    ";
    let shorter = "
        UID:short
        SUMMARY:stretch
        DTSTART;TZID=America/Denver:20260101T080000
        RRULE:FREQ=DAILY
        LAST-MODIFIED:20261019T154200Z
    ";
    let rendered = render(&calendar(None, &[event, shorter]), DENVER, &october());
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    let daily = of_event(&lines, "daily");
    assert_eq!(daily.len(), MAX_SERIES_OCCURRENCES);
    // The import was on 2026-10-20 at noon; a year on, 08:00 on 2027-10-20
    // is the last date.
    let last = NaiveDate::from_ymd_opt(2027, 10, 20).unwrap();
    let first = last - Duration::days(MAX_SERIES_OCCURRENCES as i64 - 1);
    let at_eight = |date: NaiveDate| {
        DENVER
            .from_local_datetime(&date.and_hms_opt(8, 0, 0).unwrap())
            .unwrap()
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    };
    let occurrences = occurrences(&daily);
    assert_eq!(occurrences.first().unwrap(), &at_eight(first));
    assert_eq!(occurrences.last().unwrap(), &at_eight(last));
    assert!(
        occurrences
            .iter()
            .any(|occurrence| occurrence.starts_with("2027-"))
    );
    // A series under the cap keeps every date from its start.
    let short = of_event(&lines, "short");
    assert_eq!(short[0]["occurrence"], "2026-01-01T15:00:00Z");
    assert_eq!(
        short.len(),
        (last - NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()).num_days() as usize + 1
    );
    assert_eq!(rendered.calendar_counts.truncated_series, 1);
    assert_eq!(rendered.calendar_counts.skipped(), 0);
}

#[test]
fn a_rule_that_cannot_be_expanded_writes_its_first_date() {
    let first = "
        UID:odd-1
        SUMMARY:odd
        DTSTART;TZID=America/Denver:20261021T090000
        RRULE:FREQ=FORTNIGHTLY
        LAST-MODIFIED:20261019T154200Z
    ";
    let second = "
        UID:odd-2
        SUMMARY:odder
        DTSTART:20261122T180000Z
        RRULE:FREQ=WEEKLY;BYDAY=XX
        LAST-MODIFIED:20261019T154200Z
    ";
    let rendered = render(&calendar(None, &[first, second]), DENVER, &october());
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    assert_eq!(
        occurrences(&of_event(&lines, "odd-1")),
        ["2026-10-21T15:00:00Z"]
    );
    assert_eq!(
        occurrences(&of_event(&lines, "odd-2")),
        ["2026-11-22T18:00:00Z"]
    );
    assert_eq!(rendered.calendar_counts.unexpandable_series, 2);
}

#[test]
fn changed_at_is_the_placing_instant_in_the_journal_offset_beside_its_transcript() {
    let autumn = "
        UID:autumn
        SUMMARY:autumn
        DTSTART:20261101T160000Z
        LAST-MODIFIED:20261019T154200Z
    ";
    let winter = "
        UID:winter
        SUMMARY:winter
        DTSTART:20270101T160000Z
        CREATED:20261215T170000Z
    ";
    let rendered = render(&calendar(None, &[autumn, winter]), DENVER, &october());
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    assert_eq!(
        of_event(&lines, "autumn")[0]["changed_at"],
        "2026-10-19T09:42:00-06:00"
    );
    assert_eq!(
        of_event(&lines, "winter")[0]["changed_at"],
        "2026-12-15T10:00:00-07:00"
    );
    // Each line sits in the segment of the transcript that holds its event.
    for (event, title) in [("autumn", "autumn"), ("winter", "winter")] {
        let notes = calendar_files(&rendered)
            .into_iter()
            .find(|file| file.contents.contains(&format!("\"event\":\"{event}\"")))
            .unwrap();
        let transcript = rendered
            .files
            .iter()
            .find(|file| !file.calendar && file.contents.contains(title))
            .unwrap();
        assert_eq!(
            (&notes.day, &notes.segment),
            (&transcript.day, &transcript.segment)
        );
    }
    // The same instants in Tokyo's offset, for a journal kept there.
    let tokyo = render(&calendar(None, &[autumn]), Tz::Asia__Tokyo, &october());
    assert_eq!(
        notes_of(&tokyo)[0]["changed_at"],
        "2026-10-20T00:42:00+09:00"
    );
    assert_eq!(
        notes_of(&tokyo)[0]["imported_at"],
        "2026-10-21T03:00:00+09:00"
    );
}

#[test]
fn a_file_is_checked_against_its_real_segment_and_a_utc_twin_fails() {
    let event = "
        UID:autumn
        SUMMARY:autumn
        DTSTART:20261101T160000Z
        LAST-MODIFIED:20261019T154200Z
    ";
    let rendered = render(&calendar(None, &[event]), DENVER, &october());
    let file = calendar_files(&rendered)[0];
    assert!(
        validate_import_calendar_file(
            &file.name,
            file.contents.as_bytes(),
            &file.day,
            &file.segment
        )
        .is_ok()
    );
    let utc = file
        .contents
        .replace("2026-10-19T09:42:00-06:00", "2026-10-19T15:42:00Z");
    assert_ne!(utc, file.contents);
    let name = solstone_core_format::calendar::calendar_file_name(utc.as_bytes());
    let error =
        validate_import_calendar_file(&name, utc.as_bytes(), &file.day, &file.segment).unwrap_err();
    assert_eq!(error.line, Some(1));
}

#[test]
fn two_series_of_7000_dates_split_into_two_valid_consecutive_files() {
    let series = |uid: &str| {
        format!(
            "UID:{uid}\nSUMMARY:{uid}\nDTSTART;TZID=America/Denver:20170101T080000\nRRULE:FREQ=DAILY;COUNT=3500\nLAST-MODIFIED:20261019T154200Z"
        )
    };
    let rendered = render(
        &calendar(None, &[&series("left"), &series("right")]),
        DENVER,
        &october(),
    );
    assert_valid(&rendered);
    let files = calendar_files(&rendered);
    assert_eq!(files.len(), 2);
    assert_eq!(
        (&files[0].day, &files[0].segment),
        (&files[1].day, &files[1].segment)
    );
    let counts = files
        .iter()
        .map(|file| file.contents.lines().count())
        .collect::<Vec<_>>();
    assert_eq!(counts, [5_000, 2_000]);
    let lines = notes_of(&rendered);
    assert_eq!(of_event(&lines, "left").len(), 3_500);
    assert_eq!(of_event(&lines, "right").len(), 3_500);
    // The second file picks up where the first stopped.
    assert_eq!(lines[4_999]["event"], "right");
    assert_eq!(lines[5_000]["event"], "right");
    assert!(lines[4_999]["occurrence"].as_str() < lines[5_000]["occurrence"].as_str());
}

#[test]
fn one_event_in_two_calendars_is_written_once() {
    let event =
        "UID:shared\nSUMMARY:offsite\nDTSTART:20261105T160000Z\nLAST-MODIFIED:20261019T154200Z";
    let rendered = render_zip(
        &[
            ("Work.ics", &calendar(None, &[event])),
            ("Home.ics", &calendar(None, &[event])),
        ],
        DENVER,
        &october(),
    );
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    let shared = of_event(&lines, "shared");
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0]["calendar"], "Home");
    // With equal change times, a cancellation in either calendar wins.
    let cancelled = format!("{event}\nSTATUS:CANCELLED");
    for members in [
        [("Home.ics", event), ("Work.ics", cancelled.as_str())],
        [("Home.ics", cancelled.as_str()), ("Work.ics", event)],
    ] {
        let texts = members.map(|(name, text)| (name, calendar(None, &[text])));
        let members = texts
            .iter()
            .map(|(name, text)| (*name, text.as_str()))
            .collect::<Vec<_>>();
        let rendered = render_zip(&members, DENVER, &october());
        let lines = notes_of(&rendered);
        let shared = of_event(&lines, "shared");
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0]["change"], "removed");
    }
}

#[test]
fn a_windows_zone_name_is_mapped_and_an_unknown_one_is_skipped_and_counted() {
    let pacific = "
        UID:pacific
        SUMMARY:pacific
        DTSTART;TZID=Pacific Standard Time:20261022T090000
        DTEND;TZID=Pacific Standard Time:20261022T100000
        LAST-MODIFIED:20261019T154200Z
    ";
    let tokyo = "
        UID:tokyo
        SUMMARY:tokyo
        DTSTART;TZID=Tokyo Standard Time:20261022T090000
        LAST-MODIFIED:20261019T154200Z
    ";
    let custom = "
        UID:custom
        SUMMARY:custom
        DTSTART;TZID=Customized Time Zone:20261022T090000
        LAST-MODIFIED:20261019T154200Z
    ";
    let undated = "
        UID:undated
        SUMMARY:undated
        DTSTART:20261022T150000Z
    ";
    // With no start, its line would fail the contract.
    let unscheduled = "
        UID:unscheduled
        SUMMARY:unscheduled
        LAST-MODIFIED:20261019T154200Z
    ";
    let rendered = render(
        &calendar(None, &[pacific, tokyo, custom, undated, unscheduled]),
        DENVER,
        &october(),
    );
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    let pacific = of_event(&lines, "pacific")[0];
    assert_eq!(pacific["time_zone"], "America/Los_Angeles");
    assert_eq!(pacific["start"], "2026-10-22T09:00:00-07:00");
    let tokyo = of_event(&lines, "tokyo")[0];
    assert_eq!(tokyo["time_zone"], "Asia/Tokyo");
    assert_eq!(tokyo["start"], "2026-10-22T09:00:00+09:00");
    assert!(of_event(&lines, "custom").is_empty());
    assert!(of_event(&lines, "undated").is_empty());
    assert!(of_event(&lines, "unscheduled").is_empty());
    // The import still writes every event's transcript.
    assert_eq!(rendered.entries, 5);
    assert_eq!(
        rendered.calendar_counts,
        CalendarNoteCounts {
            no_change_time: 1,
            unknown_zone: 1,
            invalid: 1,
            ..CalendarNoteCounts::default()
        }
    );
}

#[test]
fn skipped_and_shortened_counts_reach_the_import_summary() {
    let clean = crate::save::ics_summary(4, 1, &CalendarNoteCounts::default());
    for counts in [
        CalendarNoteCounts {
            no_change_time: 2,
            unknown_zone: 1,
            ..CalendarNoteCounts::default()
        },
        CalendarNoteCounts {
            invalid: 3,
            truncated_series: 4,
            unexpandable_series: 5,
            ..CalendarNoteCounts::default()
        },
    ] {
        let summary = crate::save::ics_summary(4, 1, &counts);
        assert!(summary.starts_with(&clean), "{summary}");
        let numbers = summary[clean.len()..]
            .replace(',', "")
            .split(|character: char| !character.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .map(|part| part.parse::<u64>().unwrap())
            .filter(|number| *number != MAX_SERIES_OCCURRENCES as u64)
            .collect::<Vec<_>>();
        let mut expected = vec![counts.skipped()];
        expected.extend(
            [counts.no_change_time, counts.unknown_zone, counts.invalid]
                .into_iter()
                .filter(|count| *count > 0),
        );
        if counts.skipped() == 0 {
            expected.clear();
        }
        for shortened in [counts.truncated_series, counts.unexpandable_series] {
            if shortened > 0 {
                expected.push(shortened);
            }
        }
        assert_eq!(numbers, expected, "{summary}");
        assert!(
            !summary.contains(" 0 "),
            "a zero reason is never shown: {summary}"
        );
    }
}

#[test]
fn excluded_and_added_dates_overrides_and_cancellations() {
    let series = "
        UID:standup-1
        SUMMARY:standup
        DTSTART;TZID=America/Denver:20261020T090000
        RRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=4
        EXDATE;TZID=America/Denver:20261103T090000
        RDATE;TZID=America/Denver:20261022T120000
        LAST-MODIFIED:20261019T154200Z
    ";
    // An override older than its series still stands for its occurrence.
    let older_override = "
        UID:standup-1
        SUMMARY:standup, late
        RECURRENCE-ID;TZID=America/Denver:20261027T090000
        DTSTART;TZID=America/Denver:20261027T113000
        LAST-MODIFIED:20261001T120000Z
    ";
    let cancelled_occurrence = "
        UID:standup-1
        SUMMARY:standup
        RECURRENCE-ID;TZID=America/Denver:20261110T090000
        DTSTART;TZID=America/Denver:20261110T090000
        STATUS:CANCELLED
        LAST-MODIFIED:20261019T160000Z
    ";
    let cancelled_event = "
        UID:party
        SUMMARY:party
        DTSTART:20261030T010000Z
        STATUS:CANCELLED
        LAST-MODIFIED:20261019T170000Z
    ";
    let rendered = render(
        &calendar(
            None,
            &[
                series,
                older_override,
                cancelled_occurrence,
                cancelled_event,
            ],
        ),
        DENVER,
        &october(),
    );
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    let mut standup = of_event(&lines, "standup-1");
    standup.sort_by_key(|line| line["occurrence"].as_str().map(str::to_owned));
    let summary = standup
        .iter()
        .map(|line| {
            (
                line["occurrence"].as_str().unwrap(),
                line["change"].as_str().unwrap(),
                line["start"].as_str(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        [
            (
                "2026-10-20T15:00:00Z",
                "added",
                Some("2026-10-20T09:00:00-06:00")
            ),
            (
                "2026-10-22T18:00:00Z",
                "added",
                Some("2026-10-22T12:00:00-06:00")
            ),
            (
                "2026-10-27T15:00:00Z",
                "added",
                Some("2026-10-27T11:30:00-06:00")
            ),
            ("2026-11-03T16:00:00Z", "removed", None),
            ("2026-11-10T16:00:00Z", "removed", None),
        ]
    );
    let party = of_event(&lines, "party");
    assert_eq!(party.len(), 1);
    assert_eq!(party[0]["change"], "removed");
    assert_eq!(party[0]["occurrence"], Value::Null);
}

#[test]
fn an_override_in_utc_against_a_zoned_series_replaces_its_one_date() {
    for (recurrence_id, exdate) in [
        ("RECURRENCE-ID:20261027T150000Z", ""),
        (
            "RECURRENCE-ID;TZID=America/Denver:20261027T090000",
            "EXDATE:20261027T150000Z",
        ),
    ] {
        let series = format!("{WEEKLY_STANDUP}\n{exdate}");
        let moved = format!(
            "UID:standup-1\nSUMMARY:moved\n{recurrence_id}\nDTSTART;TZID=America/Denver:20261028T140000\nLAST-MODIFIED:20261019T160000Z"
        );
        let rendered = render(&calendar(None, &[&series, &moved]), DENVER, &october());
        assert_valid(&rendered);
        let lines = notes_of(&rendered);
        let at = of_event(&lines, "standup-1")
            .into_iter()
            .filter(|line| line["occurrence"] == "2026-10-27T15:00:00Z")
            .collect::<Vec<_>>();
        assert_eq!(at.len(), 1, "{recurrence_id} {exdate}");
        assert_eq!(at[0]["change"], "added");
        assert_eq!(at[0]["start"], "2026-10-28T14:00:00-06:00");
        assert_eq!(of_event(&lines, "standup-1").len(), 4);
    }
}

#[test]
fn a_cancelled_series_removes_every_date() {
    let series = format!("{WEEKLY_STANDUP}\nSTATUS:CANCELLED");
    let rendered = render(&calendar(None, &[&series]), DENVER, &october());
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    let standup = of_event(&lines, "standup-1");
    assert_eq!(standup.len(), 4);
    assert!(standup.iter().all(|line| line["change"] == "removed"));
    assert!(standup.iter().all(|line| line["start"] == Value::Null));
}

#[test]
fn only_lines_expanded_from_a_rule_are_marked_expanded() {
    let one_off = "
        UID:review-7
        SUMMARY:design review
        DTSTART:20261023T160000Z
        LAST-MODIFIED:20261019T170000Z
    ";
    let moved = "
        UID:standup-1
        SUMMARY:standup, moved
        RECURRENCE-ID;TZID=America/Denver:20261027T090000
        DTSTART;TZID=America/Denver:20261027T100000
        LAST-MODIFIED:20261019T160000Z
    ";
    let rendered = render(
        &calendar(None, &[WEEKLY_STANDUP, moved, one_off]),
        DENVER,
        &october(),
    );
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    let marked = |line: &&&Map<String, Value>| line.get("expanded") == Some(&json!(true));
    let standup = of_event(&lines, "standup-1");
    assert_eq!(standup.iter().filter(marked).count(), 3);
    let moved = standup
        .iter()
        .find(|line| line["occurrence"] == "2026-10-27T15:00:00Z")
        .unwrap();
    assert!(moved.get("expanded").is_none());
    assert!(of_event(&lines, "review-7")[0].get("expanded").is_none());
}

#[test]
fn an_until_with_no_z_ends_the_series_in_its_own_frame() {
    let cases = [
        (
            "UID:bins\nSUMMARY:bins\nDTSTART;VALUE=DATE:20261027\nRRULE:FREQ=WEEKLY;UNTIL=20261110\nLAST-MODIFIED:20261019T154200Z",
            vec!["2026-10-27", "2026-11-03", "2026-11-10"],
        ),
        (
            "UID:walk\nSUMMARY:walk\nDTSTART:20261027T090000\nRRULE:FREQ=WEEKLY;UNTIL=20261110T090000\nLAST-MODIFIED:20261019T154200Z",
            vec![
                "2026-10-27T09:00:00",
                "2026-11-03T09:00:00",
                "2026-11-10T09:00:00",
            ],
        ),
        (
            "UID:sync\nSUMMARY:sync\nDTSTART;TZID=America/Denver:20261027T090000\nRRULE:FREQ=WEEKLY;UNTIL=20261110T090000\nLAST-MODIFIED:20261019T154200Z",
            vec![
                "2026-10-27T15:00:00Z",
                "2026-11-03T16:00:00Z",
                "2026-11-10T16:00:00Z",
            ],
        ),
    ];
    for (event, expected) in cases {
        let rendered = render(&calendar(None, &[event]), DENVER, &october());
        assert_valid(&rendered);
        let lines = notes_of(&rendered);
        let mut written = occurrences(&lines.iter().collect::<Vec<_>>());
        written.sort();
        assert_eq!(written, expected, "{event}");
        assert!(lines.iter().all(|line| line["expanded"] == true));
        assert_eq!(rendered.calendar_counts, CalendarNoteCounts::default());
    }
}

#[test]
fn an_override_that_writes_no_line_leaves_its_date_to_the_series() {
    for (override_text, skipped) in [
        (
            "UID:standup-1\nSUMMARY:standup, moved\nRECURRENCE-ID;TZID=America/Denver:20261027T090000\nDTSTART;TZID=America/Denver:20261027T100000",
            CalendarNoteCounts {
                no_change_time: 1,
                ..CalendarNoteCounts::default()
            },
        ),
        (
            "UID:standup-1\nSUMMARY:standup, moved\nRECURRENCE-ID;TZID=America/Denver:20261027T090000\nDTSTART;TZID=Customized Time Zone:20261027T100000\nLAST-MODIFIED:20261019T160000Z",
            CalendarNoteCounts {
                unknown_zone: 1,
                ..CalendarNoteCounts::default()
            },
        ),
    ] {
        let rendered = render(
            &calendar(None, &[WEEKLY_STANDUP, override_text]),
            DENVER,
            &october(),
        );
        assert_valid(&rendered);
        let lines = notes_of(&rendered);
        let date = of_event(&lines, "standup-1")
            .into_iter()
            .filter(|line| line["occurrence"] == "2026-10-27T15:00:00Z")
            .collect::<Vec<_>>();
        assert_eq!(date.len(), 1, "{override_text}");
        assert_eq!(date[0]["start"], "2026-10-27T09:00:00-06:00");
        assert_eq!(rendered.calendar_counts, skipped);
    }
}

#[test]
fn a_series_of_added_dates_alone_keeps_its_own_start() {
    let event = "
        UID:dates
        SUMMARY:planning
        DTSTART;TZID=America/Denver:20261021T090000
        RDATE;TZID=America/Denver:20261028T090000
        LAST-MODIFIED:20261019T154200Z
    ";
    let rendered = render(&calendar(None, &[event]), DENVER, &october());
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    let mut written = occurrences(&of_event(&lines, "dates"));
    written.sort();
    assert_eq!(written, ["2026-10-21T15:00:00Z", "2026-10-28T15:00:00Z"]);
    // Dates listed in the file are not a rule's expansion.
    assert!(lines.iter().all(|line| line.get("expanded").is_none()));
}

#[test]
fn an_import_writes_at_most_its_line_limit_and_counts_the_rest() {
    assert_eq!(MAX_IMPORT_LINES, 200_000);
    let series = |uid: &str, modified: &str| {
        format!(
            "UID:{uid}\nSUMMARY:{uid}\nDTSTART;TZID=America/Denver:20261020T090000\nRRULE:FREQ=DAILY;COUNT=4\nLAST-MODIFIED:{modified}"
        )
    };
    let text = calendar(
        None,
        &[
            &series("first", "20261019T154200Z"),
            &series("second", "20261019T160000Z"),
            &series("third", "20261019T170000Z"),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calendar.ics");
    std::fs::write(&path, text).unwrap();
    for (cap, written, over) in [(9, vec!["first", "second"], 1), (4, vec!["first"], 2)] {
        let rendered =
            crate::ics::render_with_line_cap(&path, &DENVER, Some(&october()), cap).unwrap();
        assert_valid(&rendered);
        let lines = notes_of(&rendered);
        let mut events = lines
            .iter()
            .map(|line| line["event"].as_str().unwrap())
            .collect::<Vec<_>>();
        events.dedup();
        assert_eq!(events, written, "cap {cap}");
        assert!(lines.len() <= cap);
        assert_eq!(rendered.calendar_counts.over_limit, over);
        // Every event's transcript is still written.
        assert_eq!(rendered.entries, 3);
    }
}

#[test]
fn an_unreadable_change_time_is_skipped_and_never_stamped_with_the_start() {
    for (field, other) in [
        ("LAST-MODIFIED", "garbage"),
        ("CREATED", "20261340T999999Z"),
    ] {
        let event =
            format!("UID:broken\nSUMMARY:broken\nDTSTART:20261105T160000Z\n{field}:{other}");
        let good =
            "UID:fine\nSUMMARY:fine\nDTSTART:20261106T160000Z\nLAST-MODIFIED:20261019T154200Z";
        let rendered = render(&calendar(None, &[&event, good]), DENVER, &october());
        let lines = notes_of(&rendered);
        assert!(of_event(&lines, "broken").is_empty(), "{field}");
        assert_eq!(of_event(&lines, "fine").len(), 1);
        assert_eq!(rendered.calendar_counts.no_change_time, 1);
        assert_eq!(rendered.entries, 2);
    }
    // An unreadable LAST-MODIFIED is not replaced by CREATED either.
    let both = "UID:both\nSUMMARY:both\nDTSTART:20261105T160000Z\nLAST-MODIFIED:garbage\nCREATED:20261001T120000Z";
    let rendered = render(&calendar(None, &[both]), DENVER, &october());
    assert!(notes_of(&rendered).is_empty());
    assert_eq!(rendered.calendar_counts.no_change_time, 1);
}

#[test]
fn a_window_never_spans_a_daylight_saving_change() {
    // The clocks go back at 02:00 on 2026-11-01 and forward at 02:00 on
    // 2026-03-08: three minutes apart by the clock of the world, an hour apart
    // on the wall.
    for (before, after) in [
        ("20261101T075800Z", "20261101T080100Z"),
        ("20260308T085800Z", "20260308T090100Z"),
    ] {
        let event = |uid: &str, modified: &str| {
            format!("UID:{uid}\nSUMMARY:{uid}\nDTSTART:20261120T160000Z\nLAST-MODIFIED:{modified}")
        };
        let rendered = render(
            &calendar(None, &[&event("before", before), &event("after", after)]),
            DENVER,
            &october(),
        );
        assert_valid(&rendered);
        let transcripts = rendered.files.iter().filter(|file| !file.calendar).count();
        assert_eq!(transcripts, 2, "{before}");
        assert_eq!(notes_of(&rendered).len(), 2);
    }
}

#[test]
fn windows_zone_names_map_to_current_iana_names() {
    for (windows, iana) in [
        ("India Standard Time", "Asia/Kolkata"),
        ("FLE Standard Time", "Europe/Kyiv"),
        ("Nepal Standard Time", "Asia/Kathmandu"),
        ("Pacific Standard Time", "America/Los_Angeles"),
    ] {
        assert_eq!(resolve_zone(windows).unwrap().name(), iana);
    }
    for line in WINDOWS_ZONES.lines().filter(|line| !line.starts_with('#')) {
        let (windows, iana) = line.split_once('\t').unwrap();
        // A Windows name that is itself an IANA name ("UTC") is read as one.
        if windows.parse::<Tz>().is_ok() {
            continue;
        }
        assert_eq!(resolve_zone(windows).unwrap().name(), iana, "{windows}");
    }
}

#[test]
fn a_line_too_large_for_a_file_is_skipped_and_counted() {
    let notes = "x".repeat(9 * 1024 * 1024);
    let event = format!(
        "UID:huge\nSUMMARY:huge\nDTSTART:20261105T160000Z\nDESCRIPTION:{notes}\nLAST-MODIFIED:20261019T154200Z"
    );
    let small =
        "UID:small\nSUMMARY:small\nDTSTART:20261106T160000Z\nLAST-MODIFIED:20261019T154200Z";
    let rendered = render(&calendar(None, &[&event, small]), DENVER, &october());
    assert_valid(&rendered);
    let lines = notes_of(&rendered);
    assert!(of_event(&lines, "huge").is_empty());
    assert_eq!(of_event(&lines, "small").len(), 1);
    assert_eq!(rendered.calendar_counts.invalid, 1);
}

#[test]
fn a_series_in_two_export_members_is_counted_once() {
    let long = "UID:daily\nSUMMARY:pills\nDTSTART;TZID=America/Denver:20101020T080000\nRRULE:FREQ=DAILY\nLAST-MODIFIED:20261019T154200Z";
    let odd = "UID:odd\nSUMMARY:odd\nDTSTART:20261122T180000Z\nRRULE:FREQ=FORTNIGHTLY\nLAST-MODIFIED:20261019T154200Z";
    let undated = "UID:undated\nSUMMARY:undated\nDTSTART:20261122T180000Z";
    let member = calendar(None, &[long, odd, undated]);
    let rendered = render_zip(
        &[("Work.ics", &member), ("Home.ics", &member)],
        DENVER,
        &october(),
    );
    assert_eq!(
        rendered.calendar_counts,
        CalendarNoteCounts {
            no_change_time: 1,
            truncated_series: 1,
            unexpandable_series: 1,
            ..CalendarNoteCounts::default()
        }
    );
}

#[test]
fn one_event_and_one_day_take_the_singular() {
    for (count, expected) in [(0, "days"), (1, "day"), (2, "days"), (11, "days")] {
        assert_eq!(crate::save::noun(count, "day", "days"), expected, "{count}");
    }
    // The summary pairs each count with its own noun.
    let one = crate::save::ics_summary(1, 1, &CalendarNoteCounts::default());
    assert!(
        one.contains("1 calendar event ") && one.ends_with(" 1 day"),
        "{one}"
    );
    let many = crate::save::ics_summary(3, 2, &CalendarNoteCounts::default());
    assert!(
        many.contains("3 calendar events ") && many.ends_with(" 2 days"),
        "{many}"
    );
}

