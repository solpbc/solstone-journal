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

pub(crate) fn create_day_in(locale: &str, journal: &Path, day: &str) -> Result<PathBuf, String> {
    // Intentional divergence: malformed --day is a named, clean exit-1 message,
    // rather than the retained Python command's traceback.
    day_path(journal, day, true)
        .map_err(|_| solstone_core_cli_copy::render_for(locale, "journal.cli.talent.path.day", &[]))
}

pub(crate) fn updated(locale: &str, journal: &Path, today: Date) -> Result<Vec<String>, String> {
    let exclude = BTreeSet::from([solstone_core_time::day_key(today)]);
    use solstone_core_cli_copy::{display_for, render_for};
    use solstone_core_journal_io::cli_diagnostics::{health_marker_detail, path_detail};
    use solstone_core_system::catchup::CatchupError;
    solstone_core_system::catchup::updated_days(journal, &exclude).map_err(|error| match error {
        CatchupError::Path(error) => display_for(
            locale,
            "journal.cli.think.updated.path",
            [("error", &path_detail(locale, &error))],
        ),
        CatchupError::Marker(error) => display_for(
            locale,
            "journal.cli.think.updated.marker",
            [("error", &health_marker_detail(locale, &error))],
        ),
        CatchupError::Io { path, source } => display_for(
            locale,
            "journal.cli.think.updated.io",
            [("path", &path.display()), ("error", &source)],
        ),
        CatchupError::State(detail) => display_for(
            locale,
            "journal.cli.think.updated.state",
            [("detail", &detail)],
        ),
        CatchupError::CapabilityUnavailable => {
            render_for(locale, "journal.cli.think.updated.capability", &[])
        }
    })
}

#[cfg(test)]
pub(crate) fn create_day(journal: &Path, day: &str) -> Result<PathBuf, String> {
    create_day_in(solstone_core_cli_copy::ENGLISH, journal, day)
}
