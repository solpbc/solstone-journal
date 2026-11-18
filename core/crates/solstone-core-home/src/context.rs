// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};

/// Read-only inputs shared by home readers.
///
/// Home carries two different coordinates and must not conflate them. `now_utc`
/// is the *instant*: every timestamp, age and millisecond comparison is computed
/// from it in UTC. `zone` is the journal's *day* coordinate, the owner zone, and
/// every `YYYYMMDD` day and wall-clock hour is derived through it, each at its
/// own offset. Formatting the instant as a day is the defect this type exists
/// to prevent.
#[derive(Debug, Clone)]
pub struct HomeContext {
    pub journal_root: PathBuf,
    pub now_utc: Timestamp,
    zone: TimeZone,
}

impl HomeContext {
    /// Build a context whose day coordinate is the journal's owner zone.
    pub fn new(journal_root: impl Into<PathBuf>, now_utc: Timestamp) -> Self {
        let journal_root = journal_root.into();
        let zone = solstone_core_journal_config::owner_zone(&journal_root);
        Self::with_zone(journal_root, now_utc, zone)
    }

    /// Build a context with an explicit day coordinate, so a test can pin the
    /// zone without depending on the journal's config or the host.
    pub fn with_zone(journal_root: impl Into<PathBuf>, now_utc: Timestamp, zone: TimeZone) -> Self {
        Self {
            journal_root: journal_root.into(),
            now_utc,
            zone,
        }
    }

    pub fn journal_root(&self) -> &Path {
        &self.journal_root
    }

    /// The journal's day coordinate itself, for placing a wall-clock time that
    /// was read out of a day directory back on the instant line.
    pub fn zone(&self) -> &TimeZone {
        &self.zone
    }

    /// The instant expressed in the journal's day coordinate, at the offset
    /// the owner zone has at that instant.
    pub fn now_local(&self) -> ZonedDateTime {
        solstone_core_time::at_offset(self.now_utc, self.zone.to_offset(self.now_utc))
    }

    pub fn local_date(&self) -> Date {
        self.now_local().date()
    }

    pub fn local_hour(&self) -> u32 {
        u32::try_from(self.now_local().hour()).unwrap_or(0)
    }

    pub fn today(&self) -> String {
        solstone_core_time::day_key(self.local_date())
    }

    pub fn yesterday(&self) -> String {
        let today = self.local_date();
        solstone_core_time::day_key(today.yesterday().unwrap_or(today))
    }

    pub fn now_ms(&self) -> i64 {
        self.now_utc.as_millisecond()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_time::utc_at;

    fn mountain() -> TimeZone {
        solstone_core_time::zone("America/Denver").unwrap()
    }

    #[test]
    fn evening_local_day_does_not_roll_over_with_the_utc_day() {
        // 2026-09-05 21:30 MDT is already 2026-09-06 03:30 UTC.
        let context = HomeContext::with_zone("/journal", utc_at(2026, 9, 6, 3, 30, 0), mountain());
        assert_eq!(context.today(), "20260905");
        assert_eq!(context.yesterday(), "20260904");
        assert_eq!(context.local_hour(), 21);
        assert_eq!(
            context.now_ms(),
            utc_at(2026, 9, 6, 3, 30, 0).as_millisecond(),
            "the instant stays UTC",
        );
    }

    #[test]
    fn the_day_coordinate_still_tracks_utc_when_the_offset_is_zero() {
        let context =
            HomeContext::with_zone("/journal", utc_at(2026, 9, 6, 3, 30, 0), TimeZone::UTC);
        assert_eq!(context.today(), "20260906");
        assert_eq!(context.yesterday(), "20260905");
        assert_eq!(context.local_hour(), 3);
    }

    #[test]
    fn yesterday_crosses_a_month_boundary_by_calendar_day() {
        let context = HomeContext::with_zone("/journal", utc_at(2026, 9, 1, 5, 0, 0), mountain());
        assert_eq!(context.today(), "20260831");
        assert_eq!(context.yesterday(), "20260830");
    }
}
