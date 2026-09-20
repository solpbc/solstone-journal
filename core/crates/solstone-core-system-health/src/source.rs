// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;

use crate::HealthError;

pub trait SegmentSource {
    fn segments(
        &self,
        journal: &std::path::Path,
        day: &str,
    ) -> Result<Vec<solstone_core_journal_io::Segment>, HealthError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FilesystemSegmentSource;

impl SegmentSource for FilesystemSegmentSource {
    fn segments(
        &self,
        journal: &std::path::Path,
        day: &str,
    ) -> Result<Vec<solstone_core_journal_io::Segment>, HealthError> {
        Ok(solstone_core_journal_io::iter_segments(
            journal,
            solstone_core_journal_io::PathOrDay::Day(day),
        )?)
    }
}

pub trait HealthLogSource {
    fn health_log_paths(&self, day: &str) -> Result<Vec<PathBuf>, HealthError>;
}

#[derive(Debug, Clone)]
pub struct FilesystemHealthLogSource {
    journal_root: PathBuf,
}

impl FilesystemHealthLogSource {
    pub fn new(journal_root: impl Into<PathBuf>) -> Self {
        Self {
            journal_root: journal_root.into(),
        }
    }

    fn health_dir(&self, day: &str) -> Result<PathBuf, HealthError> {
        NaiveDate::parse_from_str(day, "%Y%m%d")
            .map_err(|_| HealthError::InvalidDay(day.to_owned()))?;
        Ok(self.journal_root.join("chronicle").join(day).join("health"))
    }
}

impl HealthLogSource for FilesystemHealthLogSource {
    fn health_log_paths(&self, day: &str) -> Result<Vec<PathBuf>, HealthError> {
        let directory = self.health_dir(day)?;
        if !directory.is_dir() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&directory).map_err(|error| HealthError::Directory {
            path: directory.clone(),
            message: error.to_string(),
        })?;
        let mut paths = entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| HealthError::Directory {
                path: directory.clone(),
                message: error.to_string(),
            })?
            .into_iter()
            .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect::<Vec<_>>();
        paths.sort();
        Ok(paths)
    }
}

pub fn day_is_complete(journal: &std::path::Path, day: &str) -> Result<bool, HealthError> {
    #[cfg(not(unix))]
    {
        let _ = (journal, day);
        return Err(HealthError::CapabilityUnavailable {
            needed: "health-markers",
        });
    }
    #[cfg(unix)]
    {
        let coverage = solstone_core_system::daily_coverage::read_daily_coverage(journal, day);
        day_is_complete_with(journal, day, coverage.as_ref())
    }
}

/// [`day_is_complete`] over coverage the caller already computed.
///
/// Reading a day's coverage is the most expensive operation in this module —
/// it resolves every daily talent against every active facet and digests the
/// evidence each one declares.  A caller that needs the coverage anyway passes
/// it here rather than paying for it twice behind a boolean-sounding name.
pub fn day_is_complete_with(
    journal: &std::path::Path,
    day: &str,
    coverage: Result<&solstone_core_system::daily_coverage::DailyCoverage, &String>,
) -> Result<bool, HealthError> {
    #[cfg(not(unix))]
    {
        let _ = (journal, day, coverage);
        return Err(HealthError::CapabilityUnavailable {
            needed: "health-markers",
        });
    }
    #[cfg(unix)]
    {
        let _ = solstone_core_journal_io::day_path(journal, Some(day), false)?;
        // ⛔ The marker check comes first and returns early.  It replaces an
        // `&&` whose short-circuit was load-bearing: a day whose raw markers
        // are not both published is incomplete whatever its coverage says, and
        // consulting coverage anyway turns an unreadable day into a hard error
        // for callers -- `journal reprocess` among them -- that previously
        // never reached the coverage read at all.
        if !solstone_core_journal_io::day_marker_pair_status(journal, day)?.is_complete() {
            return Ok(false);
        }
        let coverage = coverage.map_err(|error| HealthError::Source(error.clone()))?;
        // ⚠ `is_current()`, deliberately.  "Complete" here means CERTIFIED, and
        // the complete path reports a day with hardcoded zeros without consulting
        // the health source at all -- only defensible for a day that has accepted
        // unit records.  Admitting unverified history here makes an unreadable
        // health log read as a clean day.  Keeping unverified history out of the
        // BACKLOG COUNTS is a different question, answered where those are taken.
        if !coverage.state.is_current() {
            return Ok(false);
        }
        let routing =
            crate::read_pending_facet_routing(&FilesystemHealthLogSource::new(journal), day)?;
        Ok(routing.value.is_empty() && routing.malformed_line_count == 0)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::day_is_complete;
    use crate::HealthError;

    #[test]
    fn marker_read_errors_propagate() {
        let temporary = tempdir().unwrap();
        let health = temporary.path().join("chronicle/20990202/health");
        fs::create_dir_all(&health).unwrap();
        fs::create_dir(health.join("stream.updated")).unwrap();

        let error = day_is_complete(temporary.path(), "20990202").unwrap_err();
        assert!(matches!(error, HealthError::HealthMarker(_)));
    }
}
