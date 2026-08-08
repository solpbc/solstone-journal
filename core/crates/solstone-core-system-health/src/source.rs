// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;

use crate::HealthError;

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
