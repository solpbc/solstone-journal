// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{collections::BTreeMap, ffi::OsString, path::Path};

use chrono::{DateTime, Utc};
use solstone_core_system_health::{FilesystemHealthLogSource, FilesystemSegmentSource};

use crate::{
    BacklogViewReader, DayScanRequest, DocumentWriter, FilesystemDayCacheWriter, JournalStatsError,
    backlog::degraded_backlog_view, cli, document::assemble_document, scan_day_with_cache,
    tokens::scan_tokens,
};

/// Observable result of a journal-level CLI invocation.
#[derive(Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run journal statistics with caller-supplied time, package roots, and seams.
pub fn run_cli(
    args: &[OsString],
    journal_root: &Path,
    now: DateTime<Utc>,
    system_talent_root: &Path,
    apps_root: &Path,
    backlog_reader: &dyn BacklogViewReader,
    document_writer: &dyn DocumentWriter,
) -> CliRun {
    let options = match cli::parse(args) {
        Ok(cli::ParseResult::Help) => return success(cli::HELP),
        Ok(cli::ParseResult::Options(options)) => options,
        Err(error) => return usage_error(&error),
    };

    match run(
        journal_root,
        now,
        system_talent_root,
        apps_root,
        options.use_cache,
        backlog_reader,
        document_writer,
        options.debug,
    ) {
        Ok(diagnostics) => {
            let mut stderr = String::new();
            if options.verbose {
                stderr.push_str(&format!(
                    "Statistics saved to {}/stats.json\n",
                    journal_root.display()
                ));
            }
            if options.debug {
                for diagnostic in diagnostics {
                    stderr.push_str(&format!("{diagnostic}\n"));
                }
            }
            CliRun {
                stdout: String::new(),
                stderr,
                exit_code: 0,
            }
        }
        Err(error) => CliRun {
            stdout: String::new(),
            stderr: format!("Error writing stats.json: {error}\n"),
            exit_code: 1,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    journal_root: &Path,
    now: DateTime<Utc>,
    system_talent_root: &Path,
    apps_root: &Path,
    use_cache: bool,
    backlog_reader: &dyn BacklogViewReader,
    document_writer: &dyn DocumentWriter,
    debug: bool,
) -> Result<Vec<String>, JournalStatsError> {
    let days = solstone_core_journal_io::day_dirs(journal_root)?;
    let mut days = days.into_iter().collect::<Vec<_>>();
    days.sort_by(|left, right| left.0.cmp(&right.0));

    let segments = FilesystemSegmentSource;
    let health = FilesystemHealthLogSource::new(journal_root);
    let cache_writer = FilesystemDayCacheWriter;
    let mut scans = BTreeMap::new();
    for (day, _) in days {
        let outcome = scan_day_with_cache(
            DayScanRequest {
                journal_root,
                day: &day,
                now,
                system_talent_root,
                apps_root,
                segment_source: &segments,
                health_source: &health,
                cache_writer: &cache_writer,
            },
            use_cache,
        )?;
        scans.insert(day, outcome.scan);
    }

    let backlog = backlog_reader
        .read_backlog_view(journal_root, now)
        .unwrap_or_else(|_| degraded_backlog_view());
    let mut diagnostics = Vec::new();
    let tokens = scan_tokens(journal_root, now, use_cache, &mut diagnostics);
    let document = assemble_document(&scans, tokens, backlog, now);
    document.validate()?;
    document_writer.write_document(&journal_root.join("stats.json"), &document)?;
    if !debug {
        diagnostics.clear();
    }
    Ok(diagnostics)
}

fn success(stdout: &str) -> CliRun {
    CliRun {
        stdout: stdout.to_owned(),
        stderr: String::new(),
        exit_code: 0,
    }
}

fn usage_error(error: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("{}journal journal-stats: error: {error}\n", cli::USAGE),
        exit_code: 2,
    }
}
