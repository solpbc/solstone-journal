// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-day journal statistics scan and cache.

mod activities;
mod backlog;
mod cache;
mod cli;
mod document;
mod document_writer;
mod error;
mod fold;
mod migrations;
mod model;
mod publication;
mod run;
mod scan;
mod tokens;

#[cfg(test)]
#[path = "../tests/day_scan.rs"]
mod day_scan;

#[cfg(test)]
#[path = "../tests/journal_stats.rs"]
mod journal_stats;

use std::{collections::BTreeMap, path::Path};

use jiff::Timestamp;
use serde_json::{Map, Value};
use solstone_core_system_health::{HealthLogSource, SegmentSource};

pub use activities::estimate_duration_minutes;
pub use backlog::{BacklogViewReader, FilesystemBacklogViewReader};
pub use cache::{
    DayCacheWriter, FilesystemDayCacheWriter, bounded_input_mtime, load_fresh_day_cache,
};
pub use document::StatsDocument;
pub use document_writer::{DocumentWriter, FilesystemDocumentWriter};
pub use error::{JournalStatsError, StatsCause, display_saved_cause};
pub use migrations::{StatsTopicMigrationReport, migrate_stats_topic_keys};
pub use model::{
    ActivityTotals, CacheStatus, DayScan, DayStats, HeatmapData, SCHEMA_VERSION, ScanDayOutcome,
};
pub use run::{CliRun, run_cli, run_cli_with_cache, run_cli_with_cache_checked};

/// Read token usage keyed by the token-file filename day without writing
/// its optional cache sidecar. A day is the local calendar day of the write.
pub fn scan_token_usage_by_day(
    journal_root: &Path,
    now: Timestamp,
) -> BTreeMap<String, BTreeMap<String, BTreeMap<String, i64>>> {
    let mut diagnostics = Vec::new();
    tokens::scan_tokens(journal_root, now, false, &mut diagnostics).by_day
}

/// Inputs for one cache-aware per-day journal statistics scan.
pub struct DayScanRequest<'a, S, H, W> {
    pub journal_root: &'a Path,
    pub day: &'a str,
    pub now: Timestamp,
    pub system_talent_root: &'a Path,
    pub apps_root: &'a Path,
    pub talent_overrides: Option<&'a Map<String, Value>>,
    pub segment_source: &'a S,
    pub health_source: &'a H,
    pub cache_writer: &'a W,
}

/// Scan one chronicle day, reusing a fresh cache when available.
pub fn scan_day<S, H, W>(
    request: DayScanRequest<'_, S, H, W>,
) -> Result<ScanDayOutcome, JournalStatsError>
where
    S: SegmentSource,
    H: HealthLogSource,
    W: DayCacheWriter,
{
    scan_day_with_cache(request, true)
}

pub(crate) fn scan_day_with_cache<S, H, W>(
    request: DayScanRequest<'_, S, H, W>,
    use_cache: bool,
) -> Result<ScanDayOutcome, JournalStatsError>
where
    S: SegmentSource,
    H: HealthLogSource,
    W: DayCacheWriter,
{
    scan_day_with_checkpoint(request, use_cache, None)
}

pub(crate) fn scan_day_with_checkpoint<S, H, W>(
    request: DayScanRequest<'_, S, H, W>,
    use_cache: bool,
    checkpoint: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<ScanDayOutcome, JournalStatsError>
where
    S: SegmentSource,
    H: HealthLogSource,
    W: DayCacheWriter,
{
    if let Some(checkpoint) = checkpoint {
        checkpoint().map_err(JournalStatsError::Validation)?;
    }
    solstone_core_time::parse_day(request.day)
        .ok_or_else(|| JournalStatsError::InvalidDay(request.day.to_owned()))?;
    let day_dir = solstone_core_journal_io::day_path(request.journal_root, request.day, false)?;
    if !day_dir.is_dir() {
        return Ok(ScanDayOutcome {
            scan: DayScan::default(),
            cache_status: CacheStatus::NotSavedMissingDay,
        });
    }
    if use_cache && let Some(scan) = cache::load_fresh_day_cache(&day_dir)? {
        return Ok(ScanDayOutcome {
            scan,
            cache_status: CacheStatus::Hit,
        });
    }

    let unreadable_inputs = if use_cache {
        let fingerprint = cache::unreadable_inputs_fingerprint(request.journal_root, request.day)?;
        if let Some(cause) = cache::load_unreadable_day(&day_dir, &fingerprint) {
            return Err(cause);
        }
        Some(fingerprint)
    } else {
        None
    };
    let scan = match scan::compute_day(&request, &day_dir) {
        Ok(scan) => scan,
        Err(error) => {
            // Only a failure raised by the day's evidence read is remembered.
            // Its text can also carry a read error there, which then waits for
            // an input or the build to change; every other failure is retried.
            if let (Some(fingerprint), JournalStatsError::Validation(_)) =
                (unreadable_inputs, &error)
            {
                // Best effort: an unsaved failure is only scanned again next run.
                if let Some(checkpoint) = checkpoint {
                    let _ = cache::save_unreadable_day_checked(
                        &day_dir,
                        &fingerprint,
                        &error,
                        checkpoint,
                    );
                } else {
                    let _ = cache::save_unreadable_day(&day_dir, &fingerprint, &error);
                }
            }
            return Err(error);
        }
    };
    if !use_cache {
        return Ok(ScanDayOutcome {
            scan,
            cache_status: CacheStatus::NotSavedNoCache,
        });
    }
    let cache_path = day_dir.join("stats.json");
    let saved = if let Some(checkpoint) = checkpoint {
        request
            .cache_writer
            .write_day_cache_checked(&cache_path, &scan, checkpoint)
    } else {
        cache::save_day_cache(request.cache_writer, &cache_path, &scan)
    };
    let cache_status = match saved {
        Ok(()) => CacheStatus::Saved,
        Err(error) => CacheStatus::SaveFailed {
            message: error.render(solstone_core_cli_copy::cli_locale()),
        },
    };
    Ok(ScanDayOutcome { scan, cache_status })
}
