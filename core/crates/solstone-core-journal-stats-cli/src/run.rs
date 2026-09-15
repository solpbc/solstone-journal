// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{collections::BTreeMap, ffi::OsString, path::Path};

use chrono::{DateTime, Utc};
use solstone_core_system_health::{FilesystemHealthLogSource, FilesystemSegmentSource};
use solstone_core_talent_config::read_talent_overrides;

use crate::{
    BacklogViewReader, CacheStatus, DayScanRequest, DocumentWriter, FilesystemDayCacheWriter,
    JournalStatsError, backlog::degraded_backlog_view, cli, document::UnreadableDay,
    document::assemble_document, scan_day_with_cache, tokens::scan_tokens,
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
        Ok((day_count, diagnostics)) => {
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
                stdout: format!(
                    "Wrote stats for {day_count} day(s) to {}/stats.json\n",
                    journal_root.display()
                ),
                stderr,
                exit_code: 0,
            }
        }
        Err(error) => CliRun {
            stdout: String::new(),
            stderr: format!("{error}\n"),
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
) -> Result<(usize, Vec<String>), String> {
    let days = solstone_core_journal_io::day_dirs(journal_root)
        .map_err(|error| format!("Error enumerating journal days: {error}"))?;
    let mut days = days.into_iter().collect::<Vec<_>>();
    days.sort_by(|left, right| left.0.cmp(&right.0));

    let segments = FilesystemSegmentSource;
    let health = FilesystemHealthLogSource::new(journal_root);
    let cache_writer = FilesystemDayCacheWriter;
    let talent_overrides = read_talent_overrides(journal_root)
        .map_err(|message| JournalStatsError::Validation(message).to_string())?;
    // Journal-scoped prerequisites are resolved once, before any day is
    // scanned.  Their failure is a property of the journal's configuration,
    // not of whichever day happened to sort first, and it must not be reported
    // against a day or contained as if that day were damaged.
    solstone_core_system::daily_coverage::daily_configs(
        journal_root,
        system_talent_root,
        apps_root,
    )
    .map_err(|message| format!("Error loading daily talent configuration: {message}"))?;
    let mut scans = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut evidence_unreadable_days = Vec::new();
    // Each day publishes its own cache as it is scanned, so a run cut short by
    // its phase budget leaves the days it reached warm and the next run starts
    // further along.  ⚠ Before containment the loop aborted on the first
    // damaged day, so a full cold walk of a large corpus has never been
    // exercised; it converges across runs rather than in one.
    for (day, _) in days {
        // One day's damaged bytes cannot discard every other day's statistics.
        // A day that cannot be scanned contributes nothing and is named with
        // its cause; no cache is published for it, so a later run re-attempts
        // it rather than reading a cache that would outlive the repair.
        match scan_day_with_cache(
            DayScanRequest {
                journal_root,
                day: &day,
                now,
                system_talent_root,
                apps_root,
                talent_overrides: talent_overrides.as_ref(),
                segment_source: &segments,
                health_source: &health,
                cache_writer: &cache_writer,
            },
            use_cache,
        ) {
            Ok(outcome) => {
                if debug && let CacheStatus::SaveFailed { message } = &outcome.cache_status {
                    diagnostics.push(format!("Day cache save failed for {day}: {message}"));
                }
                scans.insert(day, outcome.scan);
            }
            Err(error) => evidence_unreadable_days.push(UnreadableDay {
                day: day.clone(),
                cause: error.to_string(),
            }),
        }
    }

    let backlog = backlog_reader
        .read_backlog_view(journal_root, now)
        .unwrap_or_else(|_| degraded_backlog_view());
    let tokens = scan_tokens(journal_root, now, use_cache, &mut diagnostics);
    let document = assemble_document(&scans, tokens, backlog, now, evidence_unreadable_days);
    document
        .validate()
        .map_err(|error| format!("Error validating stats.json: {error}"))?;
    document_writer
        .write_document(&journal_root.join("stats.json"), &document)
        .map_err(|error| format!("Error writing stats.json: {error}"))?;
    if !debug {
        diagnostics.clear();
    }
    Ok((document.day_count, diagnostics))
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
