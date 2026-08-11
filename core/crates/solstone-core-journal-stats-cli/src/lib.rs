// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-day journal statistics scan and cache.

mod activities;
mod cache;
mod error;
mod fold;
mod model;
mod scan;
mod talents;

#[cfg(test)]
#[path = "../tests/day_scan.rs"]
mod day_scan;

use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use solstone_core_system_health::{HealthLogSource, SegmentSource};

pub use cache::{DayCacheWriter, FilesystemDayCacheWriter};
pub use error::JournalStatsError;
pub use model::{
    ActivityTotals, CacheStatus, DayScan, DayStats, HeatmapData, SCHEMA_VERSION, ScanDayOutcome,
};

/// Inputs for one cache-aware per-day journal statistics scan.
pub struct DayScanRequest<'a, S, H, W> {
    pub journal_root: &'a Path,
    pub day: &'a str,
    pub now: DateTime<Utc>,
    pub system_talent_root: &'a Path,
    pub apps_root: &'a Path,
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
    if let Some(scan) = cache::load_fresh_day_cache(&day_dir)? {
        return Ok(ScanDayOutcome {
            scan,
            cache_status: CacheStatus::Hit,
        });
    }

    let scan = scan::compute_day(&request, &day_dir)?;
    let cache_path = day_dir.join("stats.json");
    let cache_status = match cache::save_day_cache(request.cache_writer, &cache_path, &scan) {
        Ok(()) => CacheStatus::Saved,
        Err(error) => CacheStatus::SaveFailed {
            message: error.to_string(),
        },
    };
    Ok(ScanDayOutcome { scan, cache_status })
}
