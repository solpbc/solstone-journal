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
mod run;
mod scan;
mod talents;
mod tokens;

#[cfg(test)]
#[path = "../tests/day_scan.rs"]
mod day_scan;

#[cfg(test)]
#[path = "../tests/journal_stats.rs"]
mod journal_stats;

use std::{collections::BTreeMap, path::Path};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value};
use solstone_core_system_health::{HealthLogSource, SegmentSource};

pub use activities::estimate_duration_minutes;
pub use backlog::{BacklogViewReader, FilesystemBacklogViewReader};
pub use cache::{
    DayCacheWriter, FilesystemDayCacheWriter, bounded_input_mtime, load_fresh_day_cache,
};
pub use document::StatsDocument;
pub use document_writer::{DocumentWriter, FilesystemDocumentWriter};
pub use error::JournalStatsError;
pub use migrations::{StatsTopicMigrationReport, migrate_stats_topic_keys};
pub use model::{
    ActivityTotals, CacheStatus, DayScan, DayStats, HeatmapData, SCHEMA_VERSION, ScanDayOutcome,
};
pub use run::{CliRun, run_cli};

/// Read token usage keyed by the token-file filename day without writing
/// its optional cache sidecar. A day is the local calendar day of the write.
pub fn scan_token_usage_by_day(
    journal_root: &Path,
    now: DateTime<Utc>,
) -> BTreeMap<String, BTreeMap<String, BTreeMap<String, i64>>> {
    let mut diagnostics = Vec::new();
    tokens::scan_tokens(journal_root, now, false, &mut diagnostics).by_day
}

/// Inputs for one cache-aware per-day journal statistics scan.
pub struct DayScanRequest<'a, S, H, W> {
    pub journal_root: &'a Path,
    pub day: &'a str,
    pub now: DateTime<Utc>,
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
    NaiveDate::parse_from_str(request.day, "%Y%m%d")
        .map_err(|_| JournalStatsError::InvalidDay(request.day.to_owned()))?;
    let day_dir =
        solstone_core_journal_io::day_path(request.journal_root, Some(request.day), false)?;
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

    let scan = scan::compute_day(&request, &day_dir)?;
    if !use_cache {
        return Ok(ScanDayOutcome {
            scan,
            cache_status: CacheStatus::NotSavedNoCache,
        });
    }
    let cache_path = day_dir.join("stats.json");
    let cache_status = match cache::save_day_cache(request.cache_writer, &cache_path, &scan) {
        Ok(()) => CacheStatus::Saved,
        Err(error) => CacheStatus::SaveFailed {
            message: error.to_string(),
        },
    };
    Ok(ScanDayOutcome { scan, cache_status })
}
