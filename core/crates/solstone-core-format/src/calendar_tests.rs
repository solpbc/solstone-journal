// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{FixedOffset, NaiveDate};
use chrono_tz::Tz;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::*;

const GOLDEN: &str = include_str!("../../../fixtures/calendar_notes/golden.jsonl");

fn golden(index: usize) -> &'static str {
    GOLDEN.lines().nth(index).expect("golden line")
}

fn file_name(bytes: &[u8]) -> String {
    format!("calendar_{:x}.jsonl", Sha256::digest(bytes))
}

/// The file a device sends for these lines, named by its digest.
fn file(lines: &[&str]) -> (String, Vec<u8>) {
    let mut bytes = lines.join("\n").into_bytes();
    bytes.push(b'\n');
    (file_name(&bytes), bytes)
}

fn validate(
    lines: &[&str],
    day: &str,
    segment: &str,
) -> Result<Vec<CalendarNote>, CalendarFileError> {
    let (name, bytes) = file(lines);
    validate_calendar_file(&name, &bytes, day, segment)
}

fn with(line: &str, key: &str, value: Value) -> String {
    let mut record: Map<String, Value> = serde_json::from_str(line).unwrap();
    record.insert(key.to_owned(), value);
    Value::Object(record).to_string()
}

fn without(line: &str, key: &str) -> String {
    let mut record: Map<String, Value> = serde_json::from_str(line).unwrap();
    record.remove(key);
    Value::Object(record).to_string()
}

fn date(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
}

fn days(listing: &Listing) -> Vec<String> {
    listing
        .days
        .iter()
        .map(|day| day.format("%Y%m%d").to_string())
        .collect()
}

fn schedule_of(line: &str) -> Schedule {
    parse_calendar_note(line).unwrap().schedule.unwrap()
}

/// A timed note with these times, built from golden line 1.
fn timed(start: &str, end: Value) -> String {
    let line = with(golden(0), "start", json!(start));
    with(&line, "end", end)
}

fn all_day(start: &str, end: Value) -> String {
    let line = with(golden(2), "start", json!(start));
    let line = with(&line, "changed_at", json!("2026-10-19T09:42:00-06:00"));
    with(&line, "end", end)
}

#[test]
fn each_golden_line_is_accepted_as_a_file_of_its_own() {
    let cases = [
        (0, "20261019", "094000_300", Change::Added, "design review"),
        (1, "20261020", "163000_300", Change::Changed, "standup"),
        (2, "20261020", "163000_300", Change::Added, "Halloween"),
        (
            3,
            "20261020",
            "163000_300",
            Change::Added,
            "Kate Bell's Birthday",
        ),
        (
            4,
            "20261022",
            "080500_300",
            Change::Removed,
            "design review",
        ),
    ];
    for (index, day, segment, change, title) in cases {
        let notes = validate(&[golden(index)], day, segment).expect("golden line accepted");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].change, change);
        assert_eq!(notes[0].record["title"], title);
        assert_eq!(notes[0].changed_at_text(), notes[0].record["changed_at"]);
    }
    // The removal of line 1's meeting carries no schedule; its add does.
    assert!(parse_calendar_note(golden(4)).unwrap().schedule.is_none());
    assert!(parse_calendar_note(golden(0)).unwrap().schedule.is_some());
}

#[test]
fn golden_occurrences_normalize_to_their_identities() {
    let identities = (0..5)
        .map(|index| {
            let note = parse_calendar_note(golden(index)).unwrap();
            (note.event.clone(), note.occurrence.clone())
        })
        .collect::<Vec<_>>();
    assert_eq!(identities[0], identities[4]);
    assert_eq!(identities[1].1, "2026-10-21T15:00:00Z");
    assert_eq!(identities[3].1, "2027-01-20");
    assert_eq!(identities[2].1, "");
}

#[test]
fn identity_normalizes_z_offset_and_fractional_seconds() {
    let spellings = [
        "2026-10-21T15:00:00Z",
        "2026-10-21T15:00:00.000Z",
        "2026-10-21T15:00:00+00:00",
        "2026-10-21T09:00:00-06:00",
    ];
    let normalized = spellings
        .iter()
        .map(|spelling| {
            parse_calendar_note(&with(golden(1), "occurrence", json!(spelling)))
                .unwrap()
                .occurrence
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        normalized,
        BTreeSet::from(["2026-10-21T15:00:00Z".to_owned()])
    );
    let other = parse_calendar_note(&with(
        golden(1),
        "occurrence",
        json!("2026-10-22T15:00:00Z"),
    ))
    .unwrap()
    .occurrence;
    assert_eq!(other, "2026-10-22T15:00:00Z");
    let floating =
        parse_calendar_note(&with(golden(1), "occurrence", json!("2026-10-21T09:00:00")))
            .unwrap()
            .occurrence;
    assert_eq!(floating, "2026-10-21T09:00:00");

    // Two lines whose occurrences only spell the same instant differently are
    // one identity, so the file is refused.
    let a = with(golden(1), "occurrence", json!("2026-10-21T15:00:00Z"));
    let b = with(
        golden(1),
        "occurrence",
        json!("2026-10-21T15:00:00.000+00:00"),
    );
    let error = validate(&[&a, &b], "20261020", "163000_300").unwrap_err();
    assert_eq!(error.line, Some(2));
}

#[test]
fn every_line_refusal_names_its_line() {
    let base = golden(0);
    let bad = [
        "{not json".to_owned(),
        "[1,2]".to_owned(),
        without(base, "url"),
        without(base, "attendees"),
        with(base, "v", json!(2)),
        with(base, "v", json!("1")),
        with(base, "change", json!("moved")),
        with(base, "event", Value::Null),
        with(base, "event", json!("")),
        with(base, "occurrence", json!("tuesday")),
        with(base, "changed_at", json!("2026-10-19T09:42:00")),
        with(base, "changed_at", json!("not a time")),
        with(base, "all_day", Value::Null),
        with(base, "all_day", json!("false")),
        with(base, "start", Value::Null),
        with(base, "start", json!("soon")),
        with(base, "end", json!("later")),
        with(base, "start", json!("2026-10-27")),
        all_day("2026-10-31T00:00:00-06:00", json!("2026-11-01")),
        timed("2026-10-27T10:00:00", json!("2026-10-27T11:00:00-06:00")),
        timed(
            "2026-10-27T10:00:00-06:00",
            json!("2026-10-27T09:00:00-06:00"),
        ),
        all_day("2026-10-31", json!("2026-10-30")),
        with(base, "attendees", json!("x")),
        with(base, "attendees", json!([{"email": 7}])),
        with(base, "attendees", json!([{"you": "yes"}])),
        with(base, "title", json!({})),
        with(base, "notes", json!(3)),
        with(base, "organizer", json!("alex")),
        with(base, "time_zone", json!(true)),
        with(base, "repeats", json!([])),
        with(base, "calendar", json!(1)),
        with(base, "location", json!(false)),
    ];
    // The good twins of the schedule cases parse.
    for good in [
        all_day("2026-10-31", json!("2026-11-01")),
        timed("2026-10-27T10:00:00", json!("2026-10-27T11:00:00")),
        timed(
            "2026-10-27T10:00:00-06:00",
            json!("2026-10-27T10:00:00-06:00"),
        ),
        with(base, "end", Value::Null),
        with(base, "attendees", json!([])),
        with(base, "title", Value::Null),
    ] {
        assert!(parse_calendar_note(&good).is_ok(), "{good}");
    }
    // The bad line follows a good line with another identity, so the refusal
    // names line 2.
    let other = with(golden(1), "changed_at", json!("2026-10-19T09:43:00-06:00"));
    assert!(validate(&[&other, base], "20261019", "094000_300").is_ok());
    for line in &bad {
        assert!(parse_calendar_note(line).is_err(), "{line}");
        let error = validate(&[&other, line], "20261019", "094000_300")
            .expect_err(&format!("refused: {line}"));
        assert_eq!(error.line, Some(2), "{line}: {error}");
    }
}

#[test]
fn a_removal_is_checked_only_against_the_every_line_rules() {
    // Its null dates and all_day are fine, and so is junk in them.
    let removed = with(golden(4), "start", json!("not a date"));
    let removed = with(&removed, "all_day", json!("maybe"));
    assert!(validate(&[golden(4)], "20261022", "080500_300").is_ok());
    assert!(validate(&[&removed], "20261022", "080500_300").is_ok());
    // But a display field of the wrong type still refuses it.
    let bad = with(golden(4), "title", json!({}));
    assert!(validate(&[&bad], "20261022", "080500_300").is_err());
    let bad = with(golden(4), "event", json!(""));
    assert!(validate(&[&bad], "20261022", "080500_300").is_err());
}

#[test]
fn unknown_keys_are_ignored() {
    let line = with(golden(0), "travel_time", json!({"minutes": 15}));
    let notes = validate(&[&line], "20261019", "094000_300").unwrap();
    assert_eq!(notes[0].record["travel_time"]["minutes"], 15);
    assert_eq!(notes[0].event, "6CF25C80-68D2-44D9-B52C-6CAD5BEC8C10");
}

#[test]
fn the_whole_file_is_refused_when_empty_or_misnamed() {
    let empty = validate_calendar_file(&file_name(b""), b"", "20261019", "094000_300");
    assert_eq!(empty.unwrap_err().line, None);
    let (_, bytes) = file(&[golden(0)]);
    for name in [
        "calendar_abc.jsonl".to_owned(),
        file_name(&bytes)
            .to_uppercase()
            .replace("CALENDAR_", "calendar_")
            .replace(".JSONL", ".jsonl"),
        file_name(&bytes).replace(".jsonl", ".json"),
        "notes.jsonl".to_owned(),
    ] {
        let error = validate_calendar_file(&name, &bytes, "20261019", "094000_300").unwrap_err();
        assert_eq!(error.line, None, "{name}");
    }
    assert!(validate_calendar_file(&file_name(&bytes), &bytes, "20261019", "094000_300").is_ok());
    assert!(is_calendar_file_name(&file_name(&bytes)));
    assert!(names_calendar_file("calendar_abc.jsonl"));
    assert!(!names_calendar_file("audio.jsonl"));
}

#[test]
fn the_window_is_read_in_each_line_s_own_offset() {
    // Both 01:32 wall-clock times fall in 01:30's window on the night the
    // clocks go back, whichever offset each carries.
    let first = with(golden(0), "changed_at", json!("2026-11-01T01:32:00-06:00"));
    let second = with(golden(2), "changed_at", json!("2026-11-01T01:32:00-07:00"));
    let notes = validate(&[&first, &second], "20261101", "013000_300").unwrap();
    assert_eq!(notes.len(), 2);
    assert_ne!(notes[0].changed_at_ms(), notes[1].changed_at_ms());

    // A line written at -06:00 matches only the window of its own wall clock,
    // whatever zone the journal keeps.
    let line = with(golden(0), "changed_at", json!("2026-10-19T09:42:00-06:00"));
    assert!(validate(&[&line], "20261019", "094000_300").is_ok());
    // The same instant in Tokyo's wall clock is the next day at 00:42.
    assert!(validate(&[&line], "20261020", "004000_300").is_err());

    // The last second of the window is in; one second past its end is out.
    let last = with(golden(0), "changed_at", json!("2026-10-19T09:44:59-06:00"));
    assert!(validate(&[&last], "20261019", "094000_300").is_ok());
    let past = with(golden(0), "changed_at", json!("2026-10-19T09:45:00-06:00"));
    let error = validate(&[&past], "20261019", "094000_300").unwrap_err();
    assert_eq!(error.line, Some(1));
    let before = with(golden(0), "changed_at", json!("2026-10-19T09:39:59-06:00"));
    assert!(validate(&[&before], "20261019", "094000_300").is_err());
}

#[test]
fn a_timed_event_lists_each_local_day_it_overlaps_end_exclusive() {
    let denver: Tz = "America/Denver".parse().unwrap();
    let tokyo: Tz = "Asia/Tokyo".parse().unwrap();
    let meeting = schedule_of(golden(0));
    let listing = listed_days(&meeting, &denver);
    assert_eq!(days(&listing), ["20261027"]);
    assert_eq!(listing.start_hour, Some(10));
    assert!(!listing.continues);
    let listing = listed_days(&meeting, &tokyo);
    assert_eq!(days(&listing), ["20261028"]);
    assert_eq!(listing.start_hour, Some(1));

    // An overnight event lists both nights.
    let overnight = schedule_of(&timed(
        "2026-10-27T22:00:00-06:00",
        json!("2026-10-28T02:00:00-06:00"),
    ));
    assert_eq!(
        days(&listed_days(&overnight, &denver)),
        ["20261027", "20261028"]
    );

    // Ending exactly at local midnight is not on the next day.
    let to_midnight = schedule_of(&timed(
        "2026-10-27T22:00:00-06:00",
        json!("2026-10-28T00:00:00-06:00"),
    ));
    assert_eq!(days(&listed_days(&to_midnight, &denver)), ["20261027"]);

    // A null end, or one equal to the start, means the start day only.
    let open = schedule_of(&timed("2026-10-27T22:00:00-06:00", Value::Null));
    assert_eq!(days(&listed_days(&open, &denver)), ["20261027"]);
    let instant = schedule_of(&timed(
        "2026-10-28T23:00:00-06:00",
        json!("2026-10-28T23:00:00-06:00"),
    ));
    assert_eq!(days(&listed_days(&instant, &denver)), ["20261028"]);
}

#[test]
fn days_across_a_daylight_saving_change_are_counted_by_the_calendar() {
    let denver: Tz = "America/Denver".parse().unwrap();
    // Saturday 23:00 to Monday 00:00 spans the 23-hour Sunday the clocks go
    // forward. Counting 24-hour steps would miss or add a day.
    let spring = schedule_of(&timed(
        "2026-03-07T23:00:00-07:00",
        json!("2026-03-09T00:00:00-06:00"),
    ));
    assert_eq!(
        days(&listed_days(&spring, &denver)),
        ["20260307", "20260308"]
    );
    // The 25-hour Sunday the clocks go back, midnight to midnight.
    let autumn = schedule_of(&timed(
        "2026-11-01T00:00:00-06:00",
        json!("2026-11-02T00:00:00-07:00"),
    ));
    assert_eq!(days(&listed_days(&autumn, &denver)), ["20261101"]);
}

#[test]
fn a_floating_event_is_wall_clock_time_in_the_journal_zone() {
    let denver: Tz = "America/Denver".parse().unwrap();
    let tokyo: Tz = "Asia/Tokyo".parse().unwrap();
    let line = timed("2026-10-21T23:30:00", json!("2026-10-22T01:00:00"));
    let line = with(&line, "time_zone", Value::Null);
    let floating = schedule_of(&line);
    for zone in [&denver, &tokyo] {
        let listing = listed_days(&floating, zone);
        assert_eq!(days(&listing), ["20261021", "20261022"]);
        assert_eq!(listing.start_hour, Some(23));
    }
    let morning = schedule_of(&timed("2026-10-21T09:00:00", json!("2026-10-21T09:15:00")));
    assert_eq!(days(&listed_days(&morning, &tokyo)), ["20261021"]);
    assert_eq!(listed_days(&morning, &tokyo).start_hour, Some(9));
}

#[test]
fn an_all_day_event_lists_its_own_dates_with_no_zone_shift() {
    let tokyo: Tz = "Asia/Tokyo".parse().unwrap();
    let honolulu: Tz = "Pacific/Honolulu".parse().unwrap();
    let halloween = schedule_of(golden(2));
    let birthday = schedule_of(golden(3));
    for zone in [&tokyo, &honolulu] {
        assert_eq!(days(&listed_days(&halloween, zone)), ["20261031"]);
        assert_eq!(days(&listed_days(&birthday, zone)), ["20270120"]);
        assert_eq!(listed_days(&halloween, zone).start_hour, None);
    }
    let long_weekend = schedule_of(&all_day("2026-10-31", json!("2026-11-03")));
    assert_eq!(
        days(&listed_days(&long_weekend, &tokyo)),
        ["20261031", "20261101", "20261102"]
    );
    let null_end = schedule_of(&all_day("2026-12-24", Value::Null));
    assert_eq!(days(&listed_days(&null_end, &tokyo)), ["20261224"]);
    let same_end = schedule_of(&all_day("2026-12-25", json!("2026-12-25")));
    assert_eq!(days(&listed_days(&same_end, &tokyo)), ["20261225"]);
}

#[test]
fn an_event_lists_at_most_366_days_and_says_it_continues() {
    let utc = FixedOffset::east_opt(0).unwrap();
    let long = schedule_of(&all_day("2026-01-01", json!("2028-01-01")));
    let listing = listed_days(&long, &utc);
    assert_eq!(listing.days.len(), MAX_LISTED_DAYS);
    assert_eq!(listing.days[0], date("2026-01-01"));
    assert_eq!(listing.days[365], date("2027-01-01"));
    assert!(listing.continues);
    let year = schedule_of(&all_day("2026-01-01", json!("2027-01-02")));
    let listing = listed_days(&year, &utc);
    assert_eq!(listing.days.len(), 366);
    assert!(!listing.continues);
    let timed_long = schedule_of(&timed(
        "2026-01-01T12:00:00+00:00",
        json!("2027-06-01T12:00:00+00:00"),
    ));
    let listing = listed_days(&timed_long, &utc);
    assert_eq!(listing.days.len(), MAX_LISTED_DAYS);
    assert!(listing.continues);
}

#[test]
fn a_note_reads_as_plain_text_with_absent_fields_left_out() {
    let meeting = render_calendar_note(&parse_calendar_note(golden(0)).unwrap());
    for expected in [
        "added",
        "design review",
        "2026",
        "10:00",
        "11:00",
        "Work",
        "room 4",
        "Alex Rivera",
        "accepted",
        "owner@example.com",
        "tentative",
        "bring the mocks",
        "9:42",
    ] {
        assert!(meeting.contains(expected), "{expected} in {meeting}");
    }
    let birthday = render_calendar_note(&parse_calendar_note(golden(3)).unwrap());
    for expected in ["Kate Bell's Birthday", "2027", "Birthdays", "every year"] {
        assert!(birthday.contains(expected), "{expected} in {birthday}");
    }
    let removed = render_calendar_note(&parse_calendar_note(golden(4)).unwrap());
    for expected in ["removed", "design review", "Work", "8:05"] {
        assert!(removed.contains(expected), "{expected} in {removed}");
    }
    for text in [&meeting, &birthday, &removed] {
        assert!(!text.contains("null"), "{text}");
        assert!(
            !text.lines().any(|line| line.trim_end().ends_with(':')),
            "{text}"
        );
    }
    // A removal carries no place, notes or invitees, so none are shown.
    for absent in ["room 4", "bring the mocks", "Alex Rivera", "10:00"] {
        assert!(!removed.contains(absent), "{absent} in {removed}");
    }
}

#[test]
fn rendered_text_is_bounded_however_long_the_notes_are() {
    let long = "plan ".repeat(40_000);
    let line = with(golden(0), "notes", json!(long));
    let text = render_calendar_note(&parse_calendar_note(&line).unwrap());
    assert!(text.len() < 16 * 1024, "{}", text.len());
    assert!(text.contains("plan plan"));
    let short = render_calendar_note(&parse_calendar_note(golden(0)).unwrap());
    assert!(short.len() < text.len());
}

#[test]
fn stored_files_are_read_line_by_line_keeping_line_indexes() {
    let text = format!("{}\n{{broken\n{}\n{}\n", golden(0), golden(2), golden(0));
    let (notes, warnings) = read_calendar_notes(&text);
    assert_eq!(
        notes.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(notes[1].1.record["title"], "Halloween");
    assert_eq!(warnings.len(), 2);
}

#[test]
fn the_tie_break_order_is_removed_then_changed_then_added() {
    assert!(Change::Removed > Change::Changed);
    assert!(Change::Changed > Change::Added);
    assert_eq!(
        [Change::Added, Change::Changed, Change::Removed].map(Change::rank),
        [0, 1, 2]
    );
}

#[test]
fn rendered_text_stays_inside_a_readable_entry_with_multi_byte_text() {
    // Every field at its longest in four-byte and three-byte characters.
    let wide = "\u{1F4C5}".repeat(5_000);
    let cjk = "\u{4F1A}".repeat(300);
    let attendees = (0..80)
        .map(|_| json!({"name": cjk, "email": cjk, "status": cjk, "you": false}))
        .collect::<Vec<_>>();
    let mut line = golden(0).to_owned();
    for key in [
        "title",
        "location",
        "notes",
        "url",
        "calendar",
        "repeats",
        "time_zone",
    ] {
        line = with(&line, key, json!(wide));
    }
    let line = with(&line, "attendees", json!(attendees));
    let text = render_calendar_note(&parse_calendar_note(&line).unwrap());
    assert!(text.len() <= 64 * 1024, "{} bytes", text.len());
    assert!(text.len() <= MAX_RENDERED_BYTES, "{} bytes", text.len());
    // The attendee case on its own: 50 names and statuses of 300 characters.
    let line = with(golden(0), "attendees", json!(attendees));
    let text = render_calendar_note(&parse_calendar_note(&line).unwrap());
    assert!(text.len() <= MAX_RENDERED_BYTES, "{} bytes", text.len());
    assert!(text.contains("design review"));
    // An ordinary note is untouched by the budget.
    let short = render_calendar_note(&parse_calendar_note(golden(0)).unwrap());
    assert!(short.contains("bring the mocks") && short.len() < 1024);
}

#[test]
fn a_file_must_be_sent_for_one_window_on_the_300_second_grid() {
    let line = with(golden(0), "changed_at", json!("2026-10-19T09:42:00-06:00"));
    assert!(validate(&[&line], "20261019", "094000_300").is_ok());
    assert!(validate(&[&line], "20261019", "093500_600").is_err());
    for segment in [
        "094100_300",
        "094001_300",
        "094000_60",
        "094000_301",
        "0940_300",
    ] {
        let error = validate(&[&line], "20261019", segment).unwrap_err();
        assert_eq!(error.line, None, "{segment}");
    }
    let late = with(golden(0), "changed_at", json!("2026-10-19T23:59:30-06:00"));
    assert!(validate(&[&late], "20261019", "235500_300").is_ok());
}

#[test]
fn a_file_over_5000_notes_or_8_mib_is_refused_whole() {
    let note = |index: usize| {
        let line = with(golden(2), "event", json!(format!("event-{index:05}")));
        with(&line, "changed_at", json!("2026-10-20T16:31:00-06:00"))
    };
    let lines = (0..5_001).map(note).collect::<Vec<_>>();
    let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
    let error = validate(&refs, "20261020", "163000_300").unwrap_err();
    assert_eq!(error.line, None);
    // Exactly 5,000 is a whole file.
    assert_eq!(
        validate(&refs[..5_000], "20261020", "163000_300")
            .unwrap()
            .len(),
        5_000
    );
    // Fewer notes, but more than 8 MiB of them.
    let long = "x".repeat(1_100_000);
    let big = (0..8)
        .map(|index| with(&note(index), "notes", json!(long)))
        .collect::<Vec<_>>();
    let refs = big.iter().map(String::as_str).collect::<Vec<_>>();
    let (name, bytes) = file(&refs);
    assert!(bytes.len() > MAX_FILE_BYTES);
    let error = validate_calendar_file(&name, &bytes, "20261020", "163000_300").unwrap_err();
    assert_eq!(error.line, None);
    let (name, bytes) = file(&refs[..7]);
    assert!(bytes.len() < MAX_FILE_BYTES);
    assert!(validate_calendar_file(&name, &bytes, "20261020", "163000_300").is_ok());
}

/// A golden line as an import writes it: its own import and upload time.
fn imported(line: &str, import: &str, imported_at: &str) -> String {
    with(
        &with(line, "import", json!(import)),
        "imported_at",
        json!(imported_at),
    )
}

fn validate_import(
    lines: &[&str],
    day: &str,
    segment: &str,
) -> Result<Vec<CalendarNote>, CalendarFileError> {
    let (name, bytes) = file(lines);
    validate_import_calendar_file(&name, &bytes, day, segment)
}

#[test]
fn an_import_file_is_checked_against_its_own_segment_span() {
    let line = imported(
        &with(golden(0), "changed_at", json!("2026-10-19T09:42:00-06:00")),
        "20261019_154200",
        "2026-10-20T11:05:00-06:00",
    );
    // A window starts at its first event, not on the 300-second grid, and
    // runs for its own length.
    for segment in ["094130_301", "093000_900", "094200_1"] {
        assert!(
            validate_import(&[&line], "20261019", segment).is_ok(),
            "{segment}"
        );
    }
    for segment in ["094300_600", "093000_720", "094130_0", "0941_301"] {
        assert!(
            validate_import(&[&line], "20261019", segment).is_err(),
            "{segment}"
        );
    }
    // The same instant written in UTC reads as 15:42 and falls outside it.
    let utc = with(&line, "changed_at", json!("2026-10-19T15:42:00Z"));
    let error = validate_import(&[&utc], "20261019", "094130_301").unwrap_err();
    assert_eq!(error.line, Some(1));
    assert!(validate_import(&[&utc], "20261019", "154000_300").is_ok());
}

#[test]
fn an_import_line_must_name_its_import_and_when_it_happened() {
    let base = with(golden(2), "changed_at", json!("2026-10-20T16:31:00-06:00"));
    let line = imported(&base, "20261020_223100", "2026-10-21T08:00:00-06:00");
    let notes = validate_import(&[&line], "20261020", "163000_300").unwrap();
    assert_eq!(
        import_stamp(&notes[0].record).unwrap(),
        (
            "20261020_223100".to_owned(),
            DateTime::parse_from_rfc3339("2026-10-21T08:00:00-06:00").unwrap()
        )
    );
    let other = imported(&base, "20250101_000000", "2026-12-01T09:00:00-07:00");
    let notes = validate_import(&[&other], "20261020", "163000_300").unwrap();
    assert_eq!(notes[0].record["import"], json!("20250101_000000"));
    for broken in [
        without(&line, "import"),
        without(&line, "imported_at"),
        with(&line, "import", json!("")),
        with(&line, "import", json!(7)),
        with(&line, "imported_at", json!("2026-10-21T08:00:00")),
        with(&line, "expanded", json!(false)),
        with(&line, "expanded", json!("yes")),
    ] {
        let error = validate_import(&[&broken], "20261020", "163000_300").unwrap_err();
        assert_eq!(error.line, Some(1), "{broken}");
    }
    assert!(
        validate_import(
            &[&with(&line, "expanded", json!(true))],
            "20261020",
            "163000_300"
        )
        .is_ok()
    );
    // A phone's file never needs them.
    assert!(validate(&[&base], "20261020", "163000_300").is_ok());
}

#[test]
fn written_files_hold_the_golden_bytes_and_read_back() {
    let records = [3, 1, 2]
        .into_iter()
        .map(|index| serde_json::from_str::<Map<String, Value>>(golden(index)).unwrap())
        .collect::<Vec<_>>();
    let files = write_calendar_files(records);
    assert_eq!(files.len(), 1);
    let (name, bytes) = &files[0];
    // Sorted by event: 48fa…, 701B…, CCBF….
    let expected = format!("{}\n{}\n{}\n", golden(2), golden(3), golden(1));
    assert_eq!(String::from_utf8(bytes.clone()).unwrap(), expected);
    assert_eq!(name, &file_name(bytes));
    assert_eq!(calendar_file_name(bytes), *name);
    assert!(validate_calendar_file(name, bytes, "20261020", "163000_300").is_ok());
    // Keys in any order are written sorted.
    let mut reversed = Map::new();
    let golden_record: Map<String, Value> = serde_json::from_str(golden(0)).unwrap();
    for (key, value) in golden_record.iter().rev() {
        reversed.insert(key.clone(), value.clone());
    }
    let files = write_calendar_files(vec![reversed]);
    assert_eq!(files[0].1, format!("{}\n", golden(0)).into_bytes());
}

#[test]
fn more_than_5000_notes_split_into_consecutive_valid_files() {
    let records = (0..7_000)
        .rev()
        .map(|index| {
            let line = imported(
                &with(
                    &with(
                        golden(1),
                        "occurrence",
                        json!(format!("2027-01-01T00:00:{:02}Z", index % 60)),
                    ),
                    "event",
                    json!(format!("series-{:05}", index / 60)),
                ),
                "20261020_223100",
                "2026-10-21T08:00:00-06:00",
            );
            serde_json::from_str::<Map<String, Value>>(&line).unwrap()
        })
        .collect::<Vec<_>>();
    let files = write_calendar_files(records);
    assert_eq!(files.len(), 2);
    let mut all = Vec::new();
    for (index, (name, bytes)) in files.iter().enumerate() {
        let notes = validate_import_calendar_file(name, bytes, "20261020", "163000_300").unwrap();
        assert_eq!(notes.len(), [5_000, 2_000][index]);
        all.extend(notes.into_iter().map(|note| (note.event, note.occurrence)));
    }
    // The second file carries on where the first stopped.
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(all, sorted);
    assert_eq!(all.len(), 7_000);
    assert_eq!(all[0].0, "series-00000");
    assert_eq!(all[6_999].0, "series-00116");
}
