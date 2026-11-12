// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The one writer of the owner's hidden body days.
//!
//! A hide or unhide changes only the store
//! (`solstone_core_format::content::body_hidden_days`); every reader decides
//! from it as it reads. Each call, even one that changes nothing, queues for
//! search the files whose entries the days decide, so a retry heals a lost
//! queue. The day's `body.md`, and those of the days after it that a night or a
//! longer experience reaches, follow through their build token.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::Path;

use solstone_core_format::content::body_hidden_days::{
    HiddenDays, hidden_days_bytes, hidden_days_path, is_day, read_hidden_days,
};
use solstone_core_format::content::body_records::BODY_RECORD_STREAMS;
use solstone_core_format::content::strava_body::STRAVA_STREAM;

use crate::body_day::BODY_DOCUMENT;

/// Why the hidden days weren't changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyHiddenError {
    /// A day given isn't a journal day.
    NotADay(String),
    /// The store can't be read; it is left as it is and nothing changes.
    Unreadable,
    /// The store couldn't be written, or the lock taken.
    Write(String),
}

impl fmt::Display for BodyHiddenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotADay(day) => write!(f, "{day} isn't a day"),
            Self::Unreadable => write!(f, "the hidden body days can't be read"),
            Self::Write(detail) => write!(f, "the hidden body days couldn't be saved: {detail}"),
        }
    }
}

impl std::error::Error for BodyHiddenError {}

/// What a hide or unhide left: every day hidden after it, and whether every
/// file it decides was queued for search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenDaysSaved {
    pub days: BTreeSet<String>,
    pub search_queued: bool,
}

/// Hide (`hidden`) or unhide `days`.
///
/// # Errors
///
/// A day that isn't one, a store that can't be read, or a write that fails;
/// in each case nothing is changed.
pub fn set_body_days_hidden(
    journal: &Path,
    days: &BTreeSet<String>,
    hidden: bool,
) -> Result<HiddenDaysSaved, BodyHiddenError> {
    update(journal, days, |held| {
        for day in days {
            if hidden {
                held.insert(day.clone());
            } else {
                held.remove(day);
            }
        }
    })
}

/// Add `incoming`, an archive's hidden days, to this journal's: an archive
/// merge brings the owner's choice with it. Returns how many were not hidden
/// here before, and whether search was queued.
///
/// # Errors
///
/// As [`set_body_days_hidden`].
pub fn merge_hidden_body_days(
    journal: &Path,
    incoming: &BTreeSet<String>,
) -> Result<(usize, bool), BodyHiddenError> {
    let mut added = 0;
    let saved = update(journal, incoming, |held| {
        for day in incoming {
            if held.insert(day.clone()) {
                added += 1;
            }
        }
    })?;
    Ok((added, saved.search_queued))
}

fn update(
    journal: &Path,
    days: &BTreeSet<String>,
    change: impl FnOnce(&mut BTreeSet<String>),
) -> Result<HiddenDaysSaved, BodyHiddenError> {
    if let Some(day) = days.iter().find(|day| !is_day(day)) {
        return Err(BodyHiddenError::NotADay(day.clone()));
    }
    let path = hidden_days_path(journal);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| BodyHiddenError::Write(error.to_string()))?;
    }
    let after = {
        let _lock = solstone_core_journal_io::hold_lock(
            &path,
            solstone_core_journal_io::LockOptions::default(),
        )
        .map_err(|error| BodyHiddenError::Write(error.to_string()))?;
        let HiddenDays::Days(mut held) = read_hidden_days(journal) else {
            return Err(BodyHiddenError::Unreadable);
        };
        let before = held.clone();
        change(&mut held);
        if held != before {
            solstone_core_journal_io::atomic_replace(
                &path,
                &hidden_days_bytes(&held),
                solstone_core_journal_io::AtomicWriteOptions::default(),
            )
            .map_err(|error| BodyHiddenError::Write(error.to_string()))?;
        }
        held
    };
    let mut search_queued = true;
    for day in days {
        search_queued &= queue_day(journal, day);
    }
    Ok(HiddenDaysSaved {
        days: after,
        search_queued,
    })
}

/// Queue the files whose search entries `day`'s hide decides. A queue that
/// fails is logged and reported: the owner's retry queues again.
fn queue_day(journal: &Path, day: &str) -> bool {
    let mut queued = true;
    let chronicle = journal.join("chronicle").join(day);
    let streams = BODY_RECORD_STREAMS.iter().copied().chain([STRAVA_STREAM]);
    let paths = streams
        .map(|stream| chronicle.join(stream))
        .filter(|path| path.is_dir())
        .chain([chronicle.join(BODY_DOCUMENT)]);
    for path in paths {
        if let Err(error) = solstone_core_indexer_store::todo::enqueue_path(journal, &path) {
            queued = false;
            log::warn!(
                "hidden body day {day}: couldn't queue {} for search: {error}",
                path.display()
            );
        }
    }
    queued
}

#[cfg(test)]
mod tests {
    use super::*;

    fn days(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn hide_and_unhide_keep_the_others_and_a_repeat_changes_nothing() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        assert_eq!(
            set_body_days_hidden(root, &days(&["20261103", "20261105"]), true)
                .unwrap()
                .days,
            days(&["20261103", "20261105"])
        );
        assert_eq!(
            set_body_days_hidden(root, &days(&["20261103"]), true)
                .unwrap()
                .days,
            days(&["20261103", "20261105"])
        );
        assert_eq!(
            set_body_days_hidden(root, &days(&["20261103"]), false)
                .unwrap()
                .days,
            days(&["20261105"])
        );
        assert_eq!(
            read_hidden_days(root),
            HiddenDays::Days(days(&["20261105"]))
        );
    }

    #[test]
    fn a_damaged_store_or_a_bad_day_changes_nothing() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        assert_eq!(
            set_body_days_hidden(root, &days(&["2026-11-03"]), true),
            Err(BodyHiddenError::NotADay("2026-11-03".to_owned()))
        );
        let path = hidden_days_path(root);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{not json").unwrap();
        assert_eq!(
            set_body_days_hidden(root, &days(&["20261103"]), true),
            Err(BodyHiddenError::Unreadable)
        );
        assert_eq!(
            merge_hidden_body_days(root, &days(&["20261103"])),
            Err(BodyHiddenError::Unreadable)
        );
        assert_eq!(fs::read(&path).unwrap(), b"{not json");
    }

    #[test]
    fn a_merge_adds_the_archives_days_and_counts_the_new_ones() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        set_body_days_hidden(root, &days(&["20261103"]), true).unwrap();
        assert_eq!(
            merge_hidden_body_days(root, &days(&["20261103", "20261001"]))
                .unwrap()
                .0,
            1
        );
        assert_eq!(
            read_hidden_days(root),
            HiddenDays::Days(days(&["20261001", "20261103"]))
        );
    }
}
