// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use chrono::{DateTime, Utc};
use solstone_core_system_health::{
    BACKLOG_DEFAULT_WINDOW, BacklogView, FilesystemHealthLogSource, FilesystemSegmentSource,
    HealthError, read_backlog_view,
};

/// Reads the bounded journal processing backlog.
pub trait BacklogViewReader {
    fn read_backlog_view(
        &self,
        journal_root: &Path,
        now: DateTime<Utc>,
    ) -> Result<BacklogView, HealthError>;
}

/// Production backlog reader backed by the journal filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct FilesystemBacklogViewReader;

impl BacklogViewReader for FilesystemBacklogViewReader {
    fn read_backlog_view(
        &self,
        journal_root: &Path,
        now: DateTime<Utc>,
    ) -> Result<BacklogView, HealthError> {
        let health_source = FilesystemHealthLogSource::new(journal_root);
        read_backlog_view(
            &health_source,
            &FilesystemSegmentSource,
            journal_root,
            BACKLOG_DEFAULT_WINDOW,
            now,
        )
    }
}

pub(crate) fn degraded_backlog_view() -> BacklogView {
    BacklogView {
        window: BACKLOG_DEFAULT_WINDOW,
        days: Vec::new(),
        pending_days: 0,
        stuck_days: 0,
        oldest_pending_day: None,
        errors: Vec::new(),
        degraded: true,
        malformed_line_count: 0,
    }
}
