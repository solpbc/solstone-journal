// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{Duration, NaiveDate};
use solstone_core_journal_io::day_path;

pub(crate) fn selected_day(args_day: Option<&str>, cadence: bool, today: NaiveDate) -> String {
    args_day.map(ToOwned::to_owned).unwrap_or_else(|| {
        (if cadence {
            today
        } else {
            today - Duration::days(1)
        })
        .format("%Y%m%d")
        .to_string()
    })
}

pub(crate) fn create_day(journal: &Path, day: &str) -> Result<PathBuf, String> {
    // Intentional divergence: malformed --day is a named, clean exit-1 message,
    // rather than the retained Python command's traceback.
    day_path(journal, Some(day), true).map_err(|_| "day must be YYYYMMDD".to_owned())
}

pub(crate) fn updated(journal: &Path, today: NaiveDate) -> Result<Vec<String>, String> {
    let exclude = BTreeSet::from([today.format("%Y%m%d").to_string()]);
    solstone_core_system::catchup::updated_days(journal, &exclude)
        .map_err(|error| error.to_string())
}
