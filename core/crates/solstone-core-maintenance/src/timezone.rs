// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{DateTime, Days, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value};

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

/// Resolve `identity.timezone`, then an injectable host IANA key, then UTC.
pub fn resolve_owner_timezone(config: &Map<String, Value>, host: &dyn HostTimezoneSource) -> Tz {
    configured_timezone(config)
        .or_else(|| host.usable_iana_key().and_then(|key| key.parse().ok()))
        .unwrap_or(chrono_tz::UTC)
}

/// Return the host-local calendar date for an instant, without consulting owner config.
///
/// This is the injectable equivalent of Python's no-argument `astimezone()` and
/// `date.today()` paths. It intentionally uses only the host and UTC fallbacks;
/// owner-configured timezones belong to timeline rollups, not health maintenance.
pub fn host_local_date(now_utc: DateTime<Utc>, host: &dyn HostTimezoneSource) -> chrono::NaiveDate {
    host.usable_iana_key()
        .and_then(|key| key.parse::<Tz>().ok())
        .map(|timezone| now_utc.with_timezone(&timezone).date_naive())
        .unwrap_or_else(|| now_utc.date_naive())
}

/// Return yesterday's local date for a rollup invocation instant.
pub fn default_rollup_day(now_utc: DateTime<Utc>, timezone: Tz) -> String {
    now_utc
        .with_timezone(&timezone)
        .date_naive()
        .checked_sub_days(Days::new(1))
        .expect("timezone date has a prior day")
        .format("%Y-%m-%d")
        .to_string()
}

fn configured_timezone(config: &Map<String, Value>) -> Option<Tz> {
    config
        .get("identity")
        .and_then(Value::as_object)
        .and_then(|identity| identity.get("timezone"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|timezone| !timezone.is_empty())
        .and_then(|timezone| timezone.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::{
        HostTimezoneSource, ProductionHostTimezoneSource, default_rollup_day, host_local_date,
        resolve_owner_timezone,
    };
    use chrono::{TimeZone, Utc};
    use chrono_tz::Tz;
    use serde_json::{Map, Value, json};

    struct FixtureHost(Option<&'static str>);

    impl HostTimezoneSource for FixtureHost {
        fn usable_iana_key(&self) -> Option<String> {
            self.0.map(str::to_owned)
        }
    }

    #[test]
    fn configured_iana_timezone_wins_over_host() {
        let config = object(json!({"identity": {"timezone": "America/Denver"}}));
        assert_eq!(
            resolve_owner_timezone(&config, &FixtureHost(Some("Europe/London"))),
            chrono_tz::America::Denver
        );
    }

    #[test]
    fn configured_defects_fall_back_to_the_injected_host() {
        for configured in [json!(""), json!("Not/AZone"), Value::Null] {
            let config = object(json!({"identity": {"timezone": configured}}));
            assert_eq!(
                resolve_owner_timezone(&config, &FixtureHost(Some("Asia/Tokyo"))),
                chrono_tz::Asia::Tokyo
            );
        }
    }

    #[test]
    fn absent_or_unparseable_host_timezone_falls_back_to_utc() {
        assert_eq!(
            resolve_owner_timezone(&Map::new(), &ProductionHostTimezoneSource),
            chrono_tz::UTC
        );
        assert_eq!(
            resolve_owner_timezone(&Map::new(), &FixtureHost(Some("Not/AZone"))),
            chrono_tz::UTC
        );
    }

    #[test]
    fn default_rollup_day_uses_the_resolved_local_date() {
        let instant = Utc.with_ymd_and_hms(2026, 3, 2, 1, 30, 0).unwrap();
        let timezone: Tz = chrono_tz::America::Los_Angeles;
        assert_eq!(default_rollup_day(instant, timezone), "2026-02-28");
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

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }
}
