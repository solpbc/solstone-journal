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

use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;
use thiserror::Error;

pub use caps::baseline_cap_contributions;
pub use config::{
    ConfigDiagnostic, PreparedDailyTime, ScheduleConfig, ScheduleEntry, ScheduleMutation,
    add_missing_schedule_entries, configured_weekly_day_name, initialize_schedule_config,
    mutate_schedule_entries, prepare_daily_time, publish_daily_time, read_enabled_schedule_entry,
    register_default_entries, remove_schedule_entry, set_schedule_metadata,
};
pub use due::{daily_mark, hour_mark, is_due, weekly_mark};
pub use engine::{CatchUpReport, CheckReport, ScheduleEngine};
pub use report::{ScheduleReport, ScheduleReportRow, build_schedule_report};
pub use status::ScheduleStatus;
pub use submission::ScheduleSubmissionSink;

/// Caller-observed local wall time, its Unix timestamp, and the zone both are
/// read in.
///
/// Schedule decisions use `local`, the wall time in `zone`; stored run times are
/// read back in the same `zone`, so a schedule never compares two clocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleNow {
    pub local: NaiveDateTime,
    pub unix_millis: i64,
    pub zone: Tz,
}

impl ScheduleNow {
    /// `now` as a wall time in `zone`, the journal's owner zone in production.
    pub fn in_zone(now: DateTime<Utc>, zone: Tz) -> Self {
        Self {
            local: now.with_timezone(&zone).naive_local(),
            unix_millis: now.timestamp_millis(),
            zone,
        }
    }
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
