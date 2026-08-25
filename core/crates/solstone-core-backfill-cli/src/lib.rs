// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native implementation of the `journal backfill-processing-records` operation.

mod classify;
mod cli;
mod commit;
mod record;

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use chrono::{DateTime, Utc};

pub use commit::{AtomicWriter, Writer};

/// Run the backfill operation against an already-resolved journal path.
///
/// The caller owns time and output streams so the operation remains reproducible
/// and testable without a process boundary.
pub fn run(
    args: &[OsString],
    journal: &Path,
    instant: DateTime<Utc>,
    writer: &dyn Writer,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let options = match cli::parse(args) {
        Ok(cli::ParseResult::Help) => {
            let _ = stdout.write_all(cli::HELP.as_bytes());
            return 0;
        }
        Ok(cli::ParseResult::Options(options)) => options,
        Err(error) => {
            let _ = stderr.write_all(cli::USAGE.as_bytes());
            let _ = writeln!(
                stderr,
                "journal backfill-processing-records: error: {error}"
            );
            return 2;
        }
    };

    if let Some(day) = options.day.as_deref()
        && !classify::is_day_key(day)
    {
        let _ = writeln!(stderr, "expected day in YYYYMMDD format");
        return 1;
    }

    if let Err(errors) = classify::refuse_ambiguous_named_default(journal, options.day.as_deref()) {
        for error in errors {
            let _ = writeln!(stderr, "{error}");
        }
        return 1;
    }

    let mut report = match classify::plan(journal, options.day.as_deref(), instant, stderr) {
        Ok(report) => report,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return 1;
        }
    };
    if options.commit {
        commit_eligible(journal, &mut report, writer, stderr);
    }

    let _ = writeln!(
        stdout,
        "{}",
        if options.commit {
            "COMMITTED"
        } else {
            "DRY RUN (no changes written)"
        }
    );
    report.counts.write_to(stdout);
    exit_code(options.commit, &report)
}

pub(crate) fn commit_eligible(
    journal: &Path,
    report: &mut classify::Report,
    writer: &dyn Writer,
    stderr: &mut dyn Write,
) {
    for item in &report.eligible {
        match commit::commit(journal, item, writer) {
            Ok(()) => {}
            Err(commit::CommitError::Marker(error)) => {
                report.counts.move_stamp_to_write_failed();
                let _ = writeln!(
                    stderr,
                    "Stamped {}, but could not mark day {} updated: {error}",
                    item.path.display(),
                    item.day
                );
            }
            Err(error) => {
                report.counts.move_stamp_to_write_failed();
                let _ = writeln!(stderr, "Could not stamp {}: {error}", item.path.display());
            }
        }
    }
}

pub(crate) fn exit_code(commit: bool, report: &classify::Report) -> i32 {
    if commit && report.counts.write_failed > 0 {
        3
    } else {
        0
    }
}

#[cfg(test)]
mod tests;
