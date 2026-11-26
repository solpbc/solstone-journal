// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The days of body data the owner hid.
//!
//! The owner can hide a day of body data: everything every body source
//! recorded on a journal day leaves every reader, search and connections
//! included, until they unhide it. Nothing moves and nothing is stamped on a
//! record: the choice is kept as `{"days": ["YYYYMMDD", ...]}` at
//! [`HIDDEN_DAYS_RELATIVE_PATH`], so whatever comes in for a hidden day later
//! lands hidden. One writer keeps it; every reader, the search index among
//! them, reads it here.
//!
//! A store a later version wrote may hold more than this one reads
//! ([`HiddenDays::Partly`]): the days read stay hidden for every reader, and
//! search and connections take every other day as hidden too.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;
use solstone_core_journal_io::{JournalRoot, read_relative_file_bounded};

use super::body_records::{BodyRow, RowKind, parse_source_time};
use super::hide_list::{Unread, hide_list_bytes, parse_hide_list};

const DAYS: &str = "days";

/// Where the owner's hidden body days live, relative to the journal.
pub const HIDDEN_DAYS_RELATIVE_PATH: &str = "solstone/apps/body/hidden-days.json";

/// The largest store the reader accepts: room for every day of a long life.
pub const MAX_HIDDEN_DAYS_BYTES: usize = 1_048_576;

/// The owner's hidden body days, as read from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiddenDays {
    /// The days hidden; empty when the store doesn't exist.
    Days(BTreeSet<String>),
    /// These days are hidden, and a later version wrote more that this one
    /// can't read: any other day may be hidden too.
    Partly(BTreeSet<String>, Unread),
    /// The store exists but couldn't be read or isn't the expected shape.
    /// Nothing can rebuild what the owner chose, so it is never taken as
    /// "nothing hidden".
    Unreadable,
}

impl HiddenDays {
    /// Whether `day` is hidden, or `None` when that can't be known: the store
    /// can't be read, or a later version wrote more in it and `day` isn't
    /// among the days read. Search and connections take `None` as hidden.
    #[must_use]
    pub fn hides(&self, day: &str) -> Option<bool> {
        match self {
            Self::Days(days) => Some(days.contains(day)),
            Self::Partly(days, _) => days.contains(day).then_some(true),
            Self::Unreadable => None,
        }
    }

    /// Every day hidden, or `None` when the store can't be read or may hide
    /// more than this version reads. For search and connections.
    #[must_use]
    pub fn days(&self) -> Option<&BTreeSet<String>> {
        match self {
            Self::Days(days) => Some(days),
            Self::Partly(..) | Self::Unreadable => None,
        }
    }

    /// The days known to be hidden: all of them, or the ones read from a store
    /// that holds more; none when the store can't be read. For the owner's own
    /// pages, which show a day unless it is known to be hidden.
    #[must_use]
    pub fn known_days(&self) -> Cow<'_, BTreeSet<String>> {
        match self {
            Self::Days(days) | Self::Partly(days, _) => Cow::Borrowed(days),
            Self::Unreadable => Cow::Owned(BTreeSet::new()),
        }
    }

    /// What a later version wrote that this one can't read, to write back.
    #[must_use]
    pub fn unread(&self) -> Option<&Unread> {
        match self {
            Self::Partly(_, unread) => Some(unread),
            Self::Days(_) | Self::Unreadable => None,
        }
    }
}

/// Whether `day` names a calendar day as a journal day folder does.
#[must_use]
pub fn is_day(day: &str) -> bool {
    day.len() == 8
        && day.bytes().all(|byte| byte.is_ascii_digit())
        && solstone_core_time::parse_day(day).is_some()
}

/// The store's path in `journal`.
pub fn hidden_days_path(journal: &Path) -> PathBuf {
    journal.join(HIDDEN_DAYS_RELATIVE_PATH)
}

/// The days in a store's bytes, or `None` when they aren't the store's shape
/// or hold anything else: a field or an entry that isn't a day. For a reader
/// that must have the whole of the owner's choice or refuse.
pub fn parse_hidden_days(bytes: &[u8]) -> Option<BTreeSet<String>> {
    match parse_store(bytes) {
        HiddenDays::Days(days) => Some(days),
        HiddenDays::Partly(..) | HiddenDays::Unreadable => None,
    }
}

fn parse_store(bytes: &[u8]) -> HiddenDays {
    match parse_hide_list(bytes, DAYS, is_day) {
        Some((days, None)) => HiddenDays::Days(days),
        Some((days, Some(unread))) => HiddenDays::Partly(days, unread),
        None => HiddenDays::Unreadable,
    }
}

/// The bytes the store holds for `days`, in sorted order.
pub fn hidden_days_bytes(days: &BTreeSet<String>) -> Vec<u8> {
    hidden_days_bytes_keeping(days, None)
}

/// The bytes the store holds for `days`, with what a later version wrote in
/// it (`unread`) written back.
pub fn hidden_days_bytes_keeping(days: &BTreeSet<String>, unread: Option<&Unread>) -> Vec<u8> {
    let mut bytes = hide_list_bytes(DAYS, days, unread);
    bytes.push(b'\n');
    bytes
}

/// Read the owner's hidden body days from `journal`, telling a store that
/// doesn't exist (nothing hidden) from one that can't be read. A symlink
/// anywhere on the path reads as unreadable.
pub fn read_hidden_days(journal: &Path) -> HiddenDays {
    let Ok(root) = JournalRoot::open(journal) else {
        return if journal.exists() {
            HiddenDays::Unreadable
        } else {
            HiddenDays::Days(BTreeSet::new())
        };
    };
    match read_relative_file_bounded(
        &root,
        Path::new(HIDDEN_DAYS_RELATIVE_PATH),
        MAX_HIDDEN_DAYS_BYTES,
    ) {
        Ok(Some(observation)) => parse_store(&observation.bytes),
        Ok(None) => HiddenDays::Days(BTreeSet::new()),
        Err(_) => HiddenDays::Unreadable,
    }
}

/// The day a source time falls on, on the clock that placed its source's
/// windows, or its own clock when there is none.
#[must_use]
pub fn day_of(time: &str, zone: Option<&TimeZone>) -> Option<String> {
    let time = parse_source_time(time)?;
    let date = zone.map_or_else(
        || time.date(),
        |zone| zone.to_datetime(time.timestamp()).date(),
    );
    Some(solstone_core_time::day_key(date))
}

/// Which day each of a day's body rows belongs to, for a hide.
///
/// A row goes with the day of its window, except what belongs to a workout,
/// night or session that began on an earlier day: a later piece of it (each
/// carries the whole span) goes with the day it started, and so do its stages
/// and events, found through a piece of it on the same day. A night that starts
/// on a day goes with that day whole; a day's own rows stay with it.
#[derive(Debug)]
pub struct RowDays {
    window_day: String,
    zone: Option<TimeZone>,
    /// Each experience's starting day, by its id, from what this day holds.
    starts: HashMap<String, String>,
}

impl RowDays {
    /// What `rows`, all of one source on `window_day`, say about where their
    /// experiences started. `zone` is the source's placed zone.
    pub fn new<'r>(
        window_day: &str,
        zone: Option<TimeZone>,
        rows: impl IntoIterator<Item = &'r BodyRow>,
    ) -> Self {
        let mut starts = HashMap::new();
        for row in rows {
            match row.kind {
                RowKind::Experience => {
                    starts.insert(row.id.clone(), window_day.to_owned());
                }
                RowKind::ExperienceTile => {
                    if let (Some(of), Some(day)) = (
                        row.of.as_deref(),
                        row.start
                            .as_deref()
                            .and_then(|start| day_of(start, zone.as_ref())),
                    ) {
                        starts.entry(of.to_owned()).or_insert(day);
                    }
                }
                _ => {}
            }
        }
        Self {
            window_day: window_day.to_owned(),
            zone,
            starts,
        }
    }

    /// The day `row` goes with: its window's day, or an earlier day its
    /// experience started on.
    #[must_use]
    pub fn day_of_row(&self, row: &BodyRow) -> Cow<'_, str> {
        let found = match row.kind {
            RowKind::ExperienceTile if row.of.is_some() => row
                .start
                .as_deref()
                .and_then(|start| day_of(start, self.zone.as_ref())),
            RowKind::Stage | RowKind::Event => row
                .of
                .as_deref()
                .and_then(|of| self.starts.get(of))
                .cloned(),
            _ => None,
        };
        match found {
            Some(day) if day < self.window_day => Cow::Owned(day),
            _ => Cow::Borrowed(&self.window_day),
        }
    }

    /// Whether `row` goes with a day in `hidden`.
    #[must_use]
    pub fn hidden(&self, row: &BodyRow, hidden: &BTreeSet<String>) -> bool {
        hidden.contains(self.day_of_row(row).as_ref())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn write(root: &Path, contents: &[u8]) {
        let path = hidden_days_path(root);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn days(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn a_missing_store_hides_nothing() {
        let journal = TempDir::new().unwrap();
        assert_eq!(
            read_hidden_days(journal.path()),
            HiddenDays::Days(BTreeSet::new())
        );
        assert_eq!(
            read_hidden_days(journal.path()).hides("20261103"),
            Some(false)
        );
    }

    #[test]
    fn the_days_written_read_back() {
        let journal = TempDir::new().unwrap();
        let hidden = days(&["20261103", "20260101"]);
        write(journal.path(), &hidden_days_bytes(&hidden));
        let read = read_hidden_days(journal.path());
        assert_eq!(read, HiddenDays::Days(hidden));
        assert_eq!(read.hides("20261103"), Some(true));
        assert_eq!(read.hides("20261104"), Some(false));
    }

    #[test]
    fn a_damaged_store_is_never_nothing_hidden() {
        let journal = TempDir::new().unwrap();
        for bad in [
            &b"{"[..],
            b"[]",
            br#"{"days": "20261103"}"#,
            br#"{"more": 1}"#,
        ] {
            write(journal.path(), bad);
            let read = read_hidden_days(journal.path());
            assert_eq!(
                read,
                HiddenDays::Unreadable,
                "{}",
                String::from_utf8_lossy(bad)
            );
            assert_eq!(read.hides("20261103"), None);
            assert!(read.known_days().is_empty());
        }
    }

    #[test]
    fn a_later_versions_store_keeps_the_days_read_hidden_and_opens_no_other() {
        let journal = TempDir::new().unwrap();
        for later in [
            &br#"{"days": ["20261103"], "more": 1}"#[..],
            br#"{"days": ["20261103", "2026-11-05"]}"#,
            br#"{"days": ["20261103", {"from": "20261105", "to": "20261107"}]}"#,
        ] {
            write(journal.path(), later);
            let read = read_hidden_days(journal.path());
            let shown = String::from_utf8_lossy(later);
            // The day this version reads is hidden for every reader.
            assert_eq!(read.hides("20261103"), Some(true), "{shown}");
            assert_eq!(*read.known_days(), days(&["20261103"]), "{shown}");
            // No other day is known to be open, and the whole set isn't known.
            assert_eq!(read.hides("20261106"), None, "{shown}");
            assert_eq!(read.days(), None, "{shown}");
            // A reader that needs the whole of the choice still refuses.
            assert_eq!(parse_hidden_days(later), None, "{shown}");
        }
    }

    #[test]
    fn a_later_versions_store_is_written_back_whole() {
        let journal = TempDir::new().unwrap();
        write(
            journal.path(),
            br#"{"days": ["20261103", {"from": "20261105"}], "more": {"a": 1}}"#,
        );
        let read = read_hidden_days(journal.path());
        let mut held = read.known_days().into_owned();
        held.insert("20261101".to_owned());
        write(
            journal.path(),
            &hidden_days_bytes_keeping(&held, read.unread()),
        );
        let on_disk: serde_json::Value =
            serde_json::from_slice(&fs::read(hidden_days_path(journal.path())).unwrap()).unwrap();
        assert_eq!(
            on_disk,
            serde_json::json!({
                "days": ["20261101", "20261103", {"from": "20261105"}],
                "more": {"a": 1},
            })
        );
        let again = read_hidden_days(journal.path());
        assert_eq!(*again.known_days(), days(&["20261101", "20261103"]));
        assert_eq!(again.unread(), read.unread());
    }

    fn row(kind: RowKind, id: &str, of: Option<&str>, start: &str) -> BodyRow {
        let mut row = BodyRow::new(kind, id.to_owned(), "Apple Health", "sleep", "sleep".into());
        row.of = of.map(str::to_owned);
        row.start = Some(start.to_owned());
        row
    }

    #[test]
    fn a_night_goes_whole_with_the_day_it_starts() {
        let zone = solstone_core_time::zone("America/Denver");
        // The next morning holds the night's later piece and a stage of it, and
        // the morning's own readings and a nap that starts that day.
        let rows = [
            row(
                RowKind::ExperienceTile,
                "n1:2",
                Some("n1"),
                "2026-11-03 23:10:00 -0700",
            ),
            row(
                RowKind::Stage,
                "s1",
                Some("n1"),
                "2026-11-04 01:30:00 -0700",
            ),
            row(RowKind::Reading, "r1", None, "2026-11-04 07:00:00 -0700"),
            row(
                RowKind::Experience,
                "nap",
                None,
                "2026-11-04 13:00:00 -0700",
            ),
            row(
                RowKind::Stage,
                "s2",
                Some("nap"),
                "2026-11-04 13:10:00 -0700",
            ),
        ];
        let days = RowDays::new("20261104", zone, &rows);
        let on: Vec<String> = rows
            .iter()
            .map(|r| days.day_of_row(r).into_owned())
            .collect();
        assert_eq!(
            on,
            ["20261103", "20261103", "20261104", "20261104", "20261104"]
        );
        // Hiding the start day takes the night's part after midnight; hiding the
        // next day leaves it.
        let start = days_set(&["20261103"]);
        let next = days_set(&["20261104"]);
        assert!(days.hidden(&rows[0], &start) && days.hidden(&rows[1], &start));
        assert!(!days.hidden(&rows[2], &start) && !days.hidden(&rows[3], &start));
        assert!(!days.hidden(&rows[0], &next) && !days.hidden(&rows[1], &next));
        assert!(days.hidden(&rows[2], &next) && days.hidden(&rows[4], &next));
    }

    fn days_set(items: &[&str]) -> BTreeSet<String> {
        days(items)
    }
}
