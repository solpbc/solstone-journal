// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native implementation of the `journal backfill-processing-records` operation.

mod classify;
mod cli;
mod commit;
mod record;

use solstone_core_cli_copy as copy;

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use jiff::Timestamp;

pub use commit::{AtomicWriter, Writer};

/// Run the backfill operation against an already-resolved journal path.
///
/// The caller owns time and output streams so the operation remains reproducible
/// and testable without a process boundary.
pub fn run(
    args: &[OsString],
    journal: &Path,
    instant: Timestamp,
    writer: &dyn Writer,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let options = match cli::parse(args) {
        Ok(cli::ParseResult::Help) => {
            let _ = writeln!(stdout, "{}", copy::text("journal.cli.backfill.help"));
            return 0;
        }
        Ok(cli::ParseResult::Options(options)) => options,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return 2;
        }
    };

    if let Some(day) = options.day.as_deref()
        && !classify::is_day_key(day)
    {
        let _ = writeln!(stderr, "{}", copy::text("journal.cli.backfill.invalid.day"));
        return 1;
    }

    if let Err(errors) = classify::refuse_ambiguous_named_default(journal, options.day.as_deref()) {
        for error in errors {
            let _ = writeln!(stderr, "{}", classify::identity_error(&error));
        }
        return 1;
    }

    let mut report = match classify::plan(journal, options.day.as_deref(), instant, stderr) {
        Ok(report) => report,
        Err(error) => {
            let _ = writeln!(stderr, "{}", classify::identity_error(&error));
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
            copy::text("journal.cli.backfill.committed")
        } else {
            copy::text("journal.cli.backfill.preview")
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
        if let Err(error) = commit::commit(journal, item, writer) {
            report.counts.move_stamp_to_write_failed();
            let _ = writeln!(stderr, "{}", error.display_for(item));
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
