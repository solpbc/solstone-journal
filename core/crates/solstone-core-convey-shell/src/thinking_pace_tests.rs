// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use jiff::tz::TimeZone;

use super::*;

fn denver() -> TimeZone {
    TimeZone::get("America/Denver").expect("zone")
}

/// 2026-11-18 11:20:00 in Denver (UTC-7).
const NOW: i64 = 1_795_026_000;
/// 2026-11-18 11:00 in Denver, the hour in progress at NOW.
const CURRENT: i64 = NOW - 20 * 60;

fn row(use_id: &str, start: i64, day: &str, status: &str) -> String {
    format!(
        "{{\"use_id\":\"{use_id}\",\"day\":\"{day}\",\"ts\":{},\"status\":\"{status}\",\"prompt\":\"long text\"}}\n",
        start * 1000
    )
}

fn append(path: &Path, text: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open");
    file.write_all(text.as_bytes()).expect("write");
}

fn read(index: &mut Index, talents: &Path) -> Pace {
    index.refresh(talents, &denver(), NOW);
    pace(index.rows(), &denver(), NOW)
}

fn day_view(pace: &Pace) -> &View {
    &pace.views[0]
}

#[test]
fn hours_hold_recent_older_and_unfinished_apart() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let hour = CURRENT - HOUR; // 10:00
    append(
        &talents.join("20261118.jsonl"),
        &row("1", hour + 5, "20261118", "completed"),
    );
    // Yesterday's capture is still recent.
    append(
        &talents.join("20261117.jsonl"),
        &row("2", hour + 6, "20261117", "completed"),
    );
    // Catching up on an old day lands in that old day's file.
    append(
        &talents.join("20260301.jsonl"),
        &row("3", hour + 7, "20260301", "completed"),
    );
    append(
        &talents.join("20260301.jsonl"),
        &row("4", hour + 8, "20260301", "error"),
    );
    let pace = read(&mut Index::default(), talents);
    let view = day_view(&pace);
    let last = view.hour_bars.last().expect("bar");
    assert_eq!(
        (last.start, last.day.as_str(), last.hour),
        (hour, "20261118", 10)
    );
    assert_eq!((last.recent, last.older), (2, 1));
    assert_eq!((view.thoughts, view.older, view.unfinished), (3, 1, 1));
    assert_eq!(view.thinking_hours, 1);
    assert_eq!(view.median, Some(3.0));
}

#[test]
fn the_hour_in_progress_is_left_out_of_the_views_and_counted_apart() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    append(
        &talents.join("20261118.jsonl"),
        &row("1", CURRENT + 60, "20261118", "completed"),
    );
    append(
        &talents.join("20261118.jsonl"),
        &row("2", CURRENT + 61, "20261118", "error"),
    );
    let pace = read(&mut Index::default(), talents);
    assert_eq!(pace.this_hour, 1);
    assert_eq!(pace.through, CURRENT);
    assert_eq!(day_view(&pace).thoughts, 0);
    assert_eq!(day_view(&pace).median, None);
    assert!(day_view(&pace).busiest.is_none());
}

#[test]
fn the_median_is_over_hours_that_had_thinking() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let path = talents.join("20261118.jsonl");
    let mut id = 0;
    for (back, count) in [(1, 10), (2, 2), (5, 4)] {
        for _ in 0..count {
            id += 1;
            append(
                &path,
                &row(
                    &id.to_string(),
                    CURRENT - back * HOUR,
                    "20261118",
                    "completed",
                ),
            );
        }
    }
    let pace = read(&mut Index::default(), talents);
    let view = day_view(&pace);
    assert_eq!(view.hours, 24);
    assert_eq!(view.thinking_hours, 3);
    assert_eq!(view.median, Some(4.0));
    let busiest = view.busiest.as_ref().expect("busiest");
    assert_eq!((busiest.start, busiest.thoughts), (CURRENT - HOUR, 10));
    assert_eq!(pace.views[1].hour_bars.len(), 7 * 24);
}

#[test]
fn the_last_row_for_a_use_wins() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let path = talents.join("20261118.jsonl");
    append(&path, &row("7", CURRENT - HOUR, "20261118", "error"));
    append(&path, &row("7", CURRENT - HOUR, "20261118", "completed"));
    let view = read(&mut Index::default(), talents).views.remove(0);
    assert_eq!((view.thoughts, view.unfinished), (1, 0));
}

#[test]
fn only_appended_bytes_are_read_and_a_torn_row_waits() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let path = talents.join("20261118.jsonl");
    append(&path, &row("1", CURRENT - HOUR, "20261118", "completed"));
    let mut index = Index::default();
    assert_eq!(day_view(&read(&mut index, talents)).thoughts, 1);
    let whole = row("2", CURRENT - HOUR, "20261118", "completed");
    let (head, tail) = whole.split_at(20);
    append(&path, head);
    assert_eq!(day_view(&read(&mut index, talents)).thoughts, 1);
    append(&path, tail);
    assert_eq!(day_view(&read(&mut index, talents)).thoughts, 2);
    let offset = index.files[&path].offset;
    assert_eq!(offset, fs::metadata(&path).expect("meta").len());
}

#[test]
fn a_replaced_or_removed_file_is_read_again() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let path = talents.join("20261118.jsonl");
    append(&path, &row("1", CURRENT - HOUR, "20261118", "completed"));
    append(&path, &row("2", CURRENT - HOUR, "20261118", "completed"));
    let mut index = Index::default();
    assert_eq!(day_view(&read(&mut index, talents)).thoughts, 2);
    let replacement = talents.join("replacement");
    fs::write(
        &replacement,
        row("3", CURRENT - HOUR, "20261118", "completed"),
    )
    .expect("write");
    fs::rename(&replacement, &path).expect("rename");
    assert_eq!(day_view(&read(&mut index, talents)).thoughts, 1);
    fs::remove_file(&path).expect("remove");
    assert_eq!(day_view(&read(&mut index, talents)).thoughts, 0);
    assert!(index.files.is_empty());
}

#[test]
fn a_file_last_changed_before_the_window_is_not_opened() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let path = talents.join("20250101.jsonl");
    append(&path, &row("1", CURRENT - HOUR, "20250101", "completed"));
    let long_ago = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    fs::File::options()
        .write(true)
        .open(&path)
        .expect("open")
        .set_modified(long_ago)
        .expect("mtime");
    let mut index = Index::default();
    assert_eq!(day_view(&read(&mut index, talents)).thoughts, 0);
    assert!(index.files.is_empty());
}

#[test]
fn other_files_and_unreadable_rows_are_skipped() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    fs::create_dir(talents.join("activity")).expect("dir");
    append(
        &talents.join("activity").join("1_active.jsonl"),
        &row("1", CURRENT - HOUR, "20261118", "completed"),
    );
    append(
        &talents.join("notes.jsonl"),
        &row("2", CURRENT - HOUR, "20261118", "completed"),
    );
    append(
        &talents.join("20261118.jsonl"),
        "not json\n{\"use_id\":\"3\"}\n",
    );
    append(
        &talents.join("20261118.jsonl"),
        &row("4", CURRENT - HOUR, "20261118", "completed"),
    );
    assert_eq!(day_view(&read(&mut Index::default(), talents)).thoughts, 1);
}

#[test]
fn an_empty_journal_has_every_hour_and_nothing_in_them() {
    let temp = tempfile::tempdir().expect("temp");
    let pace = read(&mut Index::default(), &temp.path().join("talents"));
    assert_eq!(pace.views.len(), 3);
    assert_eq!(pace.views[2].hours, 30 * 24);
    assert_eq!(pace.views[2].day_bars.len(), 31);
    assert!(
        pace.views
            .iter()
            .all(|view| view.thoughts == 0 && view.median.is_none())
    );
}

#[test]
fn the_month_view_shows_each_days_typical_thinking_hour() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let path = talents.join("20261117.jsonl");
    // Three thinking hours on the 17th: 8 (6 recent, 2 older), 2, and 4.
    let mut id = 0;
    for (start, day, count) in [
        (CURRENT - 20 * HOUR, "20261117", 6),
        (CURRENT - 20 * HOUR, "20260301", 2),
        (CURRENT - 19 * HOUR, "20261117", 2),
        (CURRENT - 18 * HOUR, "20261117", 4),
    ] {
        for _ in 0..count {
            id += 1;
            append(&path, &row(&id.to_string(), start, day, "completed"));
        }
    }
    let pace = read(&mut Index::default(), talents);
    let month = &pace.views[2];
    let day = month
        .day_bars
        .iter()
        .find(|bar| bar.day == "20261117")
        .expect("day");
    assert_eq!((day.hours, day.thinking_hours), (24, 3));
    assert_eq!((day.thoughts, day.older_thoughts), (14, 2));
    // The typical hour is 4, of which 2 in 14 were about older days.
    assert!((day.recent + day.older - 4.0).abs() < 1e-9);
    assert!((day.older - 4.0 * 2.0 / 14.0).abs() < 1e-9);
    // The window's first and last days are partial.
    assert_eq!(month.day_bars.last().expect("today").hours, 11);
    // 720 hours back from 11:00 crosses the November 1 change, so the first
    // day starts at noon.
    assert_eq!(month.day_bars.first().expect("first").hours, 12);
}

#[test]
fn a_zone_change_starts_the_index_over() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    // 22:30 on the 17th in Denver is 14:30 on the 18th in Tokyo.
    let start = CURRENT - 11 * HOUR - 90 * 60;
    append(
        &talents.join("20261116.jsonl"),
        &row("1", start, "20261116", "completed"),
    );
    let mut index = Index::default();
    index.refresh(talents, &denver(), NOW);
    assert_eq!(index.rows().next().expect("row").kind, Kind::Recent);
    let tokyo = TimeZone::get("Asia/Tokyo").expect("zone");
    index.refresh(talents, &tokyo, NOW);
    // In Tokyo the run started on the 18th, so the 16th is older than yesterday.
    assert_eq!(index.rows().next().expect("row").kind, Kind::Older);
}

#[test]
fn hours_follow_the_owner_zone_on_a_half_hour_offset() {
    let kolkata = TimeZone::get("Asia/Kolkata").expect("zone");
    // 11:20 in Denver is 23:50 in Kolkata: the hour began 50 minutes earlier.
    assert_eq!(hour_start(&kolkata, NOW), NOW - 50 * 60);
    assert_eq!(hour_start(&denver(), NOW), CURRENT);
}

fn row_with(use_id: &str, start: i64, runtime: f64, day: &str, name: &str) -> String {
    format!(
        "{{\"use_id\":\"{use_id}\",\"name\":\"{name}\",\"day\":\"{day}\",\"ts\":{},\"runtime_seconds\":{runtime},\"status\":\"completed\"}}\n",
        start * 1000
    )
}

/// A local wall time in `zone`, as epoch seconds.
fn local(zone: &str, text: &str) -> i64 {
    let zone = TimeZone::get(zone).expect("zone");
    let civil: jiff::civil::DateTime = text.parse().expect("civil");
    zone.to_timestamp(civil).expect("instant").as_second()
}

#[test]
fn a_thought_counts_in_the_hour_it_finished() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let path = talents.join("20261118.jsonl");
    // Asked for at 08:10 and finished an hour later, at 09:10.
    append(
        &path,
        &row_with(
            "1",
            CURRENT - 3 * HOUR + 600,
            3600.0,
            "20261118",
            "activity",
        ),
    );
    // Asked for at 10:50 and finished at 11:05, inside the hour in progress.
    append(
        &path,
        &row_with("2", CURRENT - 600, 900.0, "20261118", "activity"),
    );
    let pace = read(&mut Index::default(), talents);
    let bars = &day_view(&pace).hour_bars;
    let at = |start: i64| bars.iter().find(|bar| bar.start == start).expect("bar");
    assert_eq!(at(CURRENT - 2 * HOUR).recent, 1);
    assert_eq!(at(CURRENT - 3 * HOUR).recent, 0);
    assert_eq!(at(CURRENT - HOUR).recent, 0);
    assert_eq!(pace.this_hour, 1);
}

#[test]
fn retired_newsletter_runs_are_left_out() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let path = talents.join("20261118.jsonl");
    append(
        &path,
        &row_with("1", CURRENT - HOUR, 0.0, "20261118", "facet_newsletter"),
    );
    append(
        &path,
        &row_with("2", CURRENT - HOUR, 0.0, "20261118", "activity"),
    );
    assert_eq!(day_view(&read(&mut Index::default(), talents)).thoughts, 1);
}

#[test]
fn a_file_rewritten_in_place_is_read_again() {
    let temp = tempfile::tempdir().expect("temp");
    let talents = temp.path();
    let path = talents.join("20261118.jsonl");
    append(&path, &row("1", CURRENT - HOUR, "20261118", "completed"));
    let mut index = Index::default();
    assert_eq!(day_view(&read(&mut index, talents)).thoughts, 1);
    let offset = index.files[&path].offset as usize;
    // Same file, emptied and written again past the old cursor.
    let longer = row("22222", CURRENT - HOUR, "20261118", "completed")
        + &row("33333", CURRENT - HOUR, "20261118", "completed");
    assert_ne!(longer.as_bytes()[offset - 1], b'\n');
    fs::write(&path, &longer).expect("rewrite");
    assert_eq!(day_view(&read(&mut index, talents)).thoughts, 2);
}

#[test]
fn yesterday_follows_the_calendar_across_a_long_day() {
    // 23:30 on November 1, the 25-hour day: yesterday is October 31.
    let at = local("America/Denver", "2026-11-01T23:30");
    let line = row_with("1", at, 0.0, "20261031", "activity");
    let (_, parsed) = parse_row(line.trim_end().as_bytes(), &denver()).expect("row");
    assert_eq!(parsed.kind, Kind::Recent);
    let line = row_with("2", at, 0.0, "20261030", "activity");
    let (_, parsed) = parse_row(line.trim_end().as_bytes(), &denver()).expect("row");
    assert_eq!(parsed.kind, Kind::Older);
}

#[test]
fn hours_before_a_half_hour_clock_change_are_still_counted() {
    let zone = TimeZone::get("Australia/Lord_Howe").expect("zone");
    // Lord Howe moves its clocks forward half an hour on October 4, 2026.
    let now = local("Australia/Lord_Howe", "2026-10-05T12:20");
    let before = local("Australia/Lord_Howe", "2026-10-03T10:15");
    let rows = [Row {
        at: before,
        kind: Kind::Recent,
    }];
    let pace = pace(rows.iter(), &zone, now);
    assert_eq!(pace.views[2].thoughts, 1);
    let week = &pace.views[1];
    let bar = week
        .hour_bars
        .iter()
        .find(|bar| bar.recent == 1)
        .expect("counted");
    assert_eq!((bar.day.as_str(), bar.hour), ("20261003", 10));
}
