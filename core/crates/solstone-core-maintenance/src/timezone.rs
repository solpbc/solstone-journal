// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{DateTime, Local, NaiveDate, Utc};

/// Boundary for the host's local calendar, injectable for fixtures.
pub trait HostTimezoneSource {
    /// The host-local calendar date at `now_utc`.
    fn local_date(&self, now_utc: DateTime<Utc>) -> NaiveDate;
}

/// Production host timezone source: the host's own local zone, the same zone
/// the journal's day directories and the removal approval are dated in.
pub struct ProductionHostTimezoneSource;

impl HostTimezoneSource for ProductionHostTimezoneSource {
    fn local_date(&self, now_utc: DateTime<Utc>) -> NaiveDate {
        now_utc.with_timezone(&Local).date_naive()
    }
}

/// Return the host-local calendar date for an instant, without consulting owner config.
pub fn host_local_date(now_utc: DateTime<Utc>, host: &dyn HostTimezoneSource) -> NaiveDate {
    host.local_date(now_utc)
}

/// Fixture host pinned to one IANA zone.
#[cfg(test)]
pub(crate) struct FixtureHost(pub &'static str);

#[cfg(test)]
impl HostTimezoneSource for FixtureHost {
    fn local_date(&self, now_utc: DateTime<Utc>) -> NaiveDate {
        let zone: chrono_tz::Tz = self.0.parse().expect("fixture zone parses");
        now_utc.with_timezone(&zone).date_naive()
    }
}

#[cfg(test)]
mod tests {
    use super::{FixtureHost, host_local_date};
    use chrono::{NaiveDate, TimeZone, Utc};

    #[test]
    fn host_local_date_uses_the_host_zone_across_midnight() {
        // 01:30 UTC on Mar 2 is still the evening of Mar 1 west of UTC.
        let instant = Utc.with_ymd_and_hms(2026, 3, 2, 1, 30, 0).unwrap();
        assert_eq!(
            host_local_date(instant, &FixtureHost("America/Los_Angeles")),
            NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()
        );
        assert_eq!(
            host_local_date(instant, &FixtureHost("UTC")),
            NaiveDate::from_ymd_opt(2026, 3, 2).unwrap()
        );
    }
}
