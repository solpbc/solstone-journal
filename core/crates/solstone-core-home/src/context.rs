// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};

/// Read-only inputs shared by home readers.
#[derive(Debug, Clone)]
pub struct HomeContext {
    pub journal_root: PathBuf,
    pub now_utc: DateTime<Utc>,
}

impl HomeContext {
    pub fn new(journal_root: impl Into<PathBuf>, now_utc: DateTime<Utc>) -> Self {
        Self {
            journal_root: journal_root.into(),
            now_utc,
        }
    }

    pub fn journal_root(&self) -> &Path {
        &self.journal_root
    }

    pub fn today(&self) -> String {
        self.now_utc.format("%Y%m%d").to_string()
    }

    pub fn yesterday(&self) -> String {
        (self.now_utc - Duration::days(1))
            .format("%Y%m%d")
            .to_string()
    }

    pub fn now_ms(&self) -> i64 {
        self.now_utc.timestamp_millis()
    }
}
