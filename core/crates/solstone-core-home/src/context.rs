// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use chrono::{DateTime, Days, FixedOffset, Local, NaiveDate, Timelike, Utc};

/// Read-only inputs shared by home readers.
///
/// Home carries two different coordinates and must not conflate them. `now_utc`
/// is the *instant*: every timestamp, age and millisecond comparison is computed
/// from it in UTC. `day_offset` is the journal's *day* coordinate — the local
/// offset the chronicle directories, the timeline and every other app already
/// use — and every `YYYYMMDD` day and wall-clock hour is derived through it.
/// Formatting the instant as a day is the defect this type exists to prevent.
#[derive(Debug, Clone)]
pub struct HomeContext {
    pub journal_root: PathBuf,
    pub now_utc: DateTime<Utc>,
    day_offset: FixedOffset,
}

impl HomeContext {
    /// Build a context whose day coordinate is the host's local day.
    pub fn new(journal_root: impl Into<PathBuf>, now_utc: DateTime<Utc>) -> Self {
        let day_offset = *now_utc.with_timezone(&Local).offset();
        Self::with_day_offset(journal_root, now_utc, day_offset)
    }

    /// Build a context with an explicit day coordinate, so a test can pin the
    /// local day without depending on the host's zone.
    pub fn with_day_offset(
        journal_root: impl Into<PathBuf>,
        now_utc: DateTime<Utc>,
        day_offset: FixedOffset,
    ) -> Self {
        Self {
            journal_root: journal_root.into(),
            now_utc,
            day_offset,
        }
    }

    pub fn journal_root(&self) -> &Path {
        &self.journal_root
    }

    /// The instant expressed in the journal's day coordinate.
    pub fn now_local(&self) -> DateTime<FixedOffset> {
        self.now_utc.with_timezone(&self.day_offset)
    }

    pub fn local_date(&self) -> NaiveDate {
        self.now_local().date_naive()
    }

    pub fn local_hour(&self) -> u32 {
        self.now_local().hour()
    }

    pub fn today(&self) -> String {
        self.local_date().format("%Y%m%d").to_string()
    }

    pub fn yesterday(&self) -> String {
        self.local_date()
            .checked_sub_days(Days::new(1))
            .unwrap_or_else(|| self.local_date())
            .format("%Y%m%d")
            .to_string()
    }

    pub fn now_ms(&self) -> i64 {
        self.now_utc.timestamp_millis()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn mountain() -> FixedOffset {
        FixedOffset::west_opt(6 * 3600).expect("mountain daylight offset")
    }

    #[test]
    fn evening_local_day_does_not_roll_over_with_the_utc_day() {
        // 2026-09-05 21:30 MDT is already 2026-09-06 03:30 UTC.
        let context = HomeContext::with_day_offset(
            "/journal",
            Utc.with_ymd_and_hms(2026, 9, 6, 3, 30, 0).unwrap(),
            mountain(),
        );
        assert_eq!(context.today(), "20260905");
        assert_eq!(context.yesterday(), "20260904");
        assert_eq!(context.local_hour(), 21);
        assert_eq!(
            context.now_ms(),
            Utc.with_ymd_and_hms(2026, 9, 6, 3, 30, 0)
                .unwrap()
                .timestamp_millis(),
            "the instant stays UTC",
        );
    }

    #[test]
    fn the_day_coordinate_still_tracks_utc_when_the_offset_is_zero() {
        let context = HomeContext::with_day_offset(
            "/journal",
            Utc.with_ymd_and_hms(2026, 9, 6, 3, 30, 0).unwrap(),
            FixedOffset::east_opt(0).unwrap(),
        );
        assert_eq!(context.today(), "20260906");
        assert_eq!(context.yesterday(), "20260905");
        assert_eq!(context.local_hour(), 3);
    }

    #[test]
    fn yesterday_crosses_a_month_boundary_by_calendar_day() {
        let context = HomeContext::with_day_offset(
            "/journal",
            Utc.with_ymd_and_hms(2026, 9, 1, 5, 0, 0).unwrap(),
            mountain(),
        );
        assert_eq!(context.today(), "20260831");
        assert_eq!(context.yesterday(), "20260830");
    }
}
