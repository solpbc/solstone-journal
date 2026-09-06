// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Clock-aligned schedule state and submission primitives.

mod caps;
mod completion;
mod config;
mod due;
mod engine;
mod report;
mod status;
mod submission;

use std::path::PathBuf;

use chrono::NaiveDateTime;
use thiserror::Error;

pub use caps::baseline_cap_contributions;
pub use config::{
    ConfigDiagnostic, ScheduleConfig, ScheduleEntry, ScheduleMutation,
    add_missing_schedule_entries, initialize_schedule_config, mutate_schedule_entries,
    read_enabled_schedule_entry, register_default_entries, remove_schedule_entry,
    set_schedule_metadata,
};
pub use due::{daily_mark, hour_mark, is_due, weekly_mark};
pub use engine::{CatchUpReport, CheckReport, ScheduleEngine};
pub use report::{ScheduleReport, ScheduleReportRow, build_schedule_report};
pub use status::ScheduleStatus;
pub use submission::ScheduleSubmissionSink;

/// Caller-observed local wall time and its corresponding Unix timestamp.
///
/// Schedule decisions deliberately use `local`, mirroring Python's naïve local
/// datetime behavior. The timestamp is retained for references and status output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleNow {
    pub local: NaiveDateTime,
    pub unix_millis: i64,
}

/// Failures at the schedule library boundary.
#[derive(Debug, Error)]
pub enum ScheduleError {
    #[error("schedule I/O failed: {0}")]
    Io(String),
    #[error("malformed schedules config at {path}")]
    MalformedConfig { path: PathBuf },
    #[error("schedule state at {path} must be a JSON object")]
    StateShape { path: PathBuf },
    #[error("unknown schedule metadata keys: {keys:?}")]
    UnknownMetadataKeys { keys: Vec<String> },
}
