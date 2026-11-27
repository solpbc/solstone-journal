// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Clock-aligned schedule state and submission primitives.

mod bound;
mod caps;
mod completion;
mod config;
mod due;
mod engine;
mod report;
mod status;
mod submission;

use std::path::PathBuf;

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use thiserror::Error;

pub use bound::mutate_schedule_entries_at_root_checked;
pub use caps::baseline_cap_contributions;
pub use completion::record_manual_completion;
pub use config::{
    ConfigDiagnostic, PreparedDailyTime, ScheduleConfig, ScheduleEntry, ScheduleMutation,
    add_missing_schedule_entries, configured_weekly_day_name, enabled_schedule_entry,
    initialize_schedule_config, mutate_schedule_entries, prepare_daily_time, publish_daily_time,
    publish_daily_time_checked, read_enabled_schedule_entry, register_default_entries,
    remove_schedule_entry, set_schedule_metadata, validate_schedule_entry,
};
pub use due::{daily_mark, hour_mark, is_due, weekly_mark};
pub use engine::{CatchUpReport, CheckReport, ScheduleEngine};
pub use report::{
    ScheduleReport, ScheduleReportRow, build_schedule_report, build_schedule_report_in,
};
pub use status::ScheduleStatus;
pub use submission::ScheduleSubmissionSink;

/// Caller-observed local wall time, its Unix timestamp, and the zone both are
/// read in.
///
/// Schedule decisions use `local`, the wall time in `zone`; stored run times are
/// read back in the same `zone`, so a schedule never compares two clocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleNow {
    pub local: DateTime,
    pub unix_millis: i64,
    pub zone: TimeZone,
}

impl ScheduleNow {
    /// `now` as a wall time in `zone`, the journal's owner zone in production.
    pub fn in_zone(now: Timestamp, zone: TimeZone) -> Self {
        Self {
            local: now.to_zoned_datetime(zone.clone()).datetime(),
            unix_millis: now.as_millisecond(),
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
