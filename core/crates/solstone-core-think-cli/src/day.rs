// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use jiff::civil::Date;
use solstone_core_journal_io::day_path;

pub(crate) fn selected_day(args_day: Option<&str>, cadence: bool, today: Date) -> String {
    args_day.map(ToOwned::to_owned).unwrap_or_else(|| {
        solstone_core_time::day_key(if cadence {
            today
        } else {
            today
                .yesterday()
                .expect("the day before today is a real date")
        })
    })
}

pub(crate) fn create_day(journal: &Path, day: &str) -> Result<PathBuf, String> {
    // Intentional divergence: malformed --day is a named, clean exit-1 message,
    // rather than the retained Python command's traceback.
    day_path(journal, day, true).map_err(|_| "day must be YYYYMMDD".to_owned())
}

pub(crate) fn updated(journal: &Path, today: Date) -> Result<Vec<String>, String> {
    let exclude = BTreeSet::from([solstone_core_time::day_key(today)]);
    solstone_core_system::catchup::updated_days(journal, &exclude)
        .map_err(|error| error.to_string())
}
