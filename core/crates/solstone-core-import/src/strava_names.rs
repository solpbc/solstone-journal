// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The names read from Strava workouts' own words, one line per live workout.
//!
//! `imports/.strava-owner-words.jsonl` keeps, for each workout whose first
//! tile is live, its day, a hash of the words its names were read from, the
//! people and gear read from those words, and its gear. It never keeps the
//! words themselves. Every write holds `imports/.strava`, the lock Strava's
//! import and its delete take, so a delete's removal is never lost.
//!
//! Only one run reads workouts' words at a time: it holds
//! `imports/.strava-names` while it reads, and a run that finds it held reads
//! none. The system lets go of that lock when its process ends, however it
//! ends, so a run that stopped part way never keeps another from reading.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteOptions, DEFAULT_LOCK_TIMEOUT, FileLock, LockError, LockOptions, hold_lock,
    write_jsonl,
};

/// The store, relative to the journal.
pub const STRAVA_NAMES_FILE: &str = "imports/.strava-owner-words.jsonl";
/// The lock Strava's import and its delete hold, relative to the journal.
pub const STRAVA_LOCK: &str = "imports/.strava";
/// The lock a run holds while it reads workouts' words, relative to the
/// journal.
pub const STRAVA_NAMES_RUN_LOCK: &str = "imports/.strava-names";

const FILE_MODE: u32 = 0o600;

/// A name read from a workout's words.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StravaName {
    pub name: String,
    /// `Person` or `Tool`.
    #[serde(rename = "type")]
    pub entity_type: String,
}

/// One live workout's line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StravaNamesLine {
    pub activity_id: String,
    /// The day of the workout's first tile, `YYYYMMDD`.
    pub day: String,
    /// The hash of the words `entities` were read from. `None` while they
    /// haven't been read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words_hash: Option<String>,
    /// The people and gear read from the words.
    #[serde(default)]
    pub entities: Vec<StravaName>,
    /// The workout's gear, as Strava names it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gear: Vec<String>,
}

impl StravaNamesLine {
    /// A line for a workout whose words haven't been read.
    #[must_use]
    pub fn unread(activity_id: &str, day: &str) -> Self {
        Self {
            activity_id: activity_id.to_owned(),
            day: day.to_owned(),
            words_hash: None,
            entities: Vec::new(),
            gear: Vec::new(),
        }
    }
}

/// The store's lines, by activity id.
pub type StravaNames = BTreeMap<String, StravaNamesLine>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StravaNamesError {
    /// The Strava lock couldn't be taken.
    Lock(String),
    /// The store exists but couldn't be read.
    Read(String),
    /// The store couldn't be written.
    Write(String),
}

impl fmt::Display for StravaNamesError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(detail) => write!(out, "Strava lock unavailable: {detail}"),
            Self::Read(detail) => write!(out, "Strava names unreadable: {detail}"),
            Self::Write(detail) => write!(out, "Strava names not written: {detail}"),
        }
    }
}

impl std::error::Error for StravaNamesError {}

/// The store's lines. A missing store has none. A line that can't be read is
/// left out and logged, so its workout's words are read again; the store is
/// rebuilt from the workouts, never the other way round.
pub fn read_strava_names(journal: &Path) -> Result<StravaNames, StravaNamesError> {
    let path = journal.join(STRAVA_NAMES_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StravaNames::new());
        }
        Err(error) => return Err(StravaNamesError::Read(error.to_string())),
    };
    let mut lines = StravaNames::new();
    for (number, line) in bytes.split(|&byte| byte == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<StravaNamesLine>(line) {
            Ok(line) => {
                lines.insert(line.activity_id.clone(), line);
            }
            Err(error) => log::warn!("{STRAVA_NAMES_FILE}: line {} left out: {error}", number + 1),
        }
    }
    Ok(lines)
}

/// Change the store under the Strava lock: read it, apply `change`, and write
/// it back when anything changed. Returns the lines as they now are. The lock
/// is held only for this; the caller must not already hold it.
pub fn update_strava_names(
    journal: &Path,
    change: impl FnOnce(&mut StravaNames),
) -> Result<StravaNames, StravaNamesError> {
    update_strava_names_inner(journal, change, || {})
}

#[cfg(test)]
pub(crate) fn update_strava_names_with_hook(
    journal: &Path,
    change: impl FnOnce(&mut StravaNames),
    before_write: impl FnOnce(),
) -> Result<StravaNames, StravaNamesError> {
    update_strava_names_inner(journal, change, before_write)
}

fn update_strava_names_inner(
    journal: &Path,
    change: impl FnOnce(&mut StravaNames),
    before_write: impl FnOnce(),
) -> Result<StravaNames, StravaNamesError> {
    let lock = hold_strava_lock(journal)?;
    let mut lines = read_strava_names(journal)?;
    let before = lines.clone();
    change(&mut lines);
    if lines != before {
        before_write();
        save_lines(journal, &lines)?;
    }
    drop(lock);
    Ok(lines)
}

/// Remove the lines of `activity_ids`, for a delete that already holds the
/// Strava lock (`_held`). Returns how many were removed.
pub fn remove_strava_names(
    journal: &Path,
    activity_ids: &BTreeSet<String>,
    _held: &FileLock,
) -> Result<usize, StravaNamesError> {
    let mut lines = read_strava_names(journal)?;
    let before = lines.len();
    lines.retain(|id, _| !activity_ids.contains(id));
    let removed = before - lines.len();
    if removed > 0 {
        save_lines(journal, &lines)?;
    }
    Ok(removed)
}

/// Take the Strava lock, as Strava's import and its delete do.
pub fn hold_strava_lock(journal: &Path) -> Result<FileLock, StravaNamesError> {
    hold_lock(
        journal.join(STRAVA_LOCK),
        LockOptions {
            timeout: DEFAULT_LOCK_TIMEOUT,
            mode: Some(FILE_MODE),
            ..LockOptions::default()
        },
    )
    .map_err(|error| StravaNamesError::Lock(error.to_string()))
}

/// Take the lock a run holds while it reads workouts' words, without
/// waiting: `None` when another run holds it now.
pub fn hold_strava_names_run(journal: &Path) -> Result<Option<FileLock>, StravaNamesError> {
    match hold_lock(
        journal.join(STRAVA_NAMES_RUN_LOCK),
        LockOptions {
            timeout: Duration::ZERO,
            mode: Some(FILE_MODE),
            ..LockOptions::default()
        },
    ) {
        Ok(lock) => Ok(Some(lock)),
        Err(LockError::Timeout(_)) => Ok(None),
        Err(error) => Err(StravaNamesError::Lock(error.to_string())),
    }
}

fn save_lines(journal: &Path, lines: &StravaNames) -> Result<(), StravaNamesError> {
    write_jsonl(
        journal.join(STRAVA_NAMES_FILE),
        lines.values(),
        AtomicWriteOptions {
            mode: Some(FILE_MODE),
        },
    )
    .map_err(|error| StravaNamesError::Write(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn line(id: &str, day: &str) -> StravaNamesLine {
        StravaNamesLine {
            words_hash: Some(format!("hash-{id}")),
            entities: vec![StravaName {
                name: "Sam".to_owned(),
                entity_type: "Person".to_owned(),
            }],
            gear: vec!["Road bike".to_owned()],
            ..StravaNamesLine::unread(id, day)
        }
    }

    fn journal_with(lines: &[StravaNamesLine]) -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("imports")).unwrap();
        update_strava_names(journal.path(), |store| {
            for line in lines {
                store.insert(line.activity_id.clone(), line.clone());
            }
        })
        .unwrap();
        journal
    }

    #[test]
    fn lines_round_trip_and_a_bad_line_is_left_out() {
        let journal = journal_with(&[line("11", "20261001"), line("12", "20261002")]);
        let path = journal.path().join(STRAVA_NAMES_FILE);
        let mut text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        text.push_str("not json\n");
        fs::write(&path, text).unwrap();
        let lines = read_strava_names(journal.path()).unwrap();
        assert_eq!(
            lines.keys().map(String::as_str).collect::<Vec<_>>(),
            ["11", "12"]
        );
        assert_eq!(lines["11"], line("11", "20261001"));
    }

    #[test]
    fn a_missing_store_has_no_lines() {
        let journal = tempfile::tempdir().unwrap();
        assert!(read_strava_names(journal.path()).unwrap().is_empty());
    }

    #[test]
    fn every_write_holds_the_strava_lock_and_releases_it() {
        let journal = journal_with(&[]);
        let lock = journal.path().join(STRAVA_LOCK);
        let mut held = None;
        update_strava_names_with_hook(
            journal.path(),
            |store| {
                store.insert("11".to_owned(), line("11", "20261001"));
            },
            || held = Some(solstone_core_journal_io::lock_is_held(&lock).unwrap()),
        )
        .unwrap();
        assert_eq!(held, Some(true));
        assert!(!solstone_core_journal_io::lock_is_held(&lock).unwrap());
        assert_eq!(read_strava_names(journal.path()).unwrap().len(), 1);
    }

    #[test]
    fn an_unchanged_store_is_not_rewritten() {
        let journal = journal_with(&[line("11", "20261001")]);
        let mut wrote = false;
        update_strava_names_with_hook(journal.path(), |_| {}, || wrote = true).unwrap();
        assert!(!wrote);
    }

    #[test]
    fn one_run_reads_at_a_time_and_its_lock_goes_with_it() {
        let journal = journal_with(&[]);
        let first = hold_strava_names_run(journal.path()).unwrap();
        assert!(first.is_some());
        assert!(hold_strava_names_run(journal.path()).unwrap().is_none());
        drop(first);
        assert!(hold_strava_names_run(journal.path()).unwrap().is_some());
    }

    #[test]
    fn a_delete_removes_only_its_workouts_lines() {
        let journal = journal_with(&[
            line("11", "20261001"),
            line("12", "20261002"),
            line("13", "20261003"),
        ]);
        let held = hold_strava_lock(journal.path()).unwrap();
        let removed = remove_strava_names(
            journal.path(),
            &BTreeSet::from(["11".to_owned(), "13".to_owned(), "99".to_owned()]),
            &held,
        )
        .unwrap();
        drop(held);
        assert_eq!(removed, 2);
        assert_eq!(
            read_strava_names(journal.path())
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["12"]
        );
    }
}
