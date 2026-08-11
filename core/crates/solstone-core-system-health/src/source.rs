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
    day_is_complete_with_metadata(journal, day, modified)
}

fn day_is_complete_with_metadata<F>(
    journal: &std::path::Path,
    day: &str,
    metadata: F,
) -> Result<bool, HealthError>
where
    F: Fn(&std::path::Path) -> Result<std::time::SystemTime, HealthError>,
{
    let day_path = solstone_core_journal_io::day_path(journal, Some(day), false)?;
    let stream = day_path.join("health/stream.updated");
    if !stream.is_file() {
        return Ok(true);
    }
    let daily = day_path.join("health/daily.updated");
    if !daily.is_file() {
        return Ok(false);
    }
    let stream_modified = metadata(&stream)?;
    let daily_modified = metadata(&daily)?;
    Ok(stream_modified <= daily_modified)
}

fn modified(path: &std::path::Path) -> Result<std::time::SystemTime, HealthError> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| HealthError::Metadata {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::day_is_complete_with_metadata;
    use crate::HealthError;

    #[test]
    fn metadata_errors_after_marker_presence_propagate() {
        let temporary = tempdir().unwrap();
        let health = temporary.path().join("chronicle/20990202/health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("stream.updated"), "stream\n").unwrap();
        fs::write(health.join("daily.updated"), "daily\n").unwrap();

        let error = day_is_complete_with_metadata(temporary.path(), "20990202", |path| {
            Err(HealthError::Metadata {
                path: path.to_path_buf(),
                message: "metadata unavailable".to_owned(),
            })
        })
        .unwrap_err();
        assert!(matches!(error, HealthError::Metadata { .. }));
    }
}
