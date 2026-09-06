// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

/// Boundary for the otherwise unreachable CPython host-timezone fallback.
pub trait HostTimezoneSource {
    fn usable_iana_key(&self) -> Option<String>;
}

/// Production host timezone source.
pub struct ProductionHostTimezoneSource;

impl HostTimezoneSource for ProductionHostTimezoneSource {
    fn usable_iana_key(&self) -> Option<String> {
        // docs/PORTING.md#owner-timezone: CPython's `astimezone()` normally
        // supplies a fixed-offset timezone without a usable `.key`, so this
        // host branch is dead in production but injectable for fixtures.
        None
    }
}

/// Return the host-local calendar date for an instant, without consulting owner config.
///
/// This is the injectable equivalent of Python's no-argument `astimezone()` and
/// `date.today()` paths. It intentionally uses only the host and UTC fallbacks.
pub fn host_local_date(now_utc: DateTime<Utc>, host: &dyn HostTimezoneSource) -> chrono::NaiveDate {
    host.usable_iana_key()
        .and_then(|key| key.parse::<Tz>().ok())
        .map(|timezone| now_utc.with_timezone(&timezone).date_naive())
        .unwrap_or_else(|| now_utc.date_naive())
}

#[cfg(test)]
mod tests {
    use super::{HostTimezoneSource, host_local_date};
    use chrono::{TimeZone, Utc};

    struct FixtureHost(Option<&'static str>);

    impl HostTimezoneSource for FixtureHost {
        fn usable_iana_key(&self) -> Option<String> {
            self.0.map(str::to_owned)
        }
    }

    #[test]
    fn host_local_date_uses_the_host_key_across_midnight() {
        let instant = Utc.with_ymd_and_hms(2026, 3, 2, 1, 30, 0).unwrap();
        assert_eq!(
            host_local_date(instant, &FixtureHost(Some("America/Los_Angeles"))),
            chrono::NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()
        );
    }

    #[test]
    fn host_local_date_uses_utc_without_a_usable_host_key() {
        let instant = Utc.with_ymd_and_hms(2026, 3, 2, 1, 30, 0).unwrap();
        let expected = chrono::NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
        assert_eq!(host_local_date(instant, &FixtureHost(None)), expected);
        assert_eq!(
            host_local_date(instant, &FixtureHost(Some("Not/AZone"))),
            expected
        );
    }
}
