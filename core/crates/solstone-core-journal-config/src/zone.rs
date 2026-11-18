// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's time zone: one answer for "today" and for reading a wall clock.

use std::path::Path;

use jiff::Timestamp;
pub use jiff::tz::TimeZone;

use crate::read::read_journal_config;

/// The zone the journal keeps the owner's days in: `identity.timezone` when it
/// names a real IANA zone, otherwise this computer's own zone. Setup fills the
/// setting from the browser that ran it, and the owner can change it.
pub fn owner_zone(journal: &Path) -> TimeZone {
    configured_zone(journal).unwrap_or_else(host_zone)
}

/// The owner's day (`YYYYMMDD`) that `at` falls on, in [`owner_zone`]: for a
/// time the owner reads as a date, such as when the trash erases something.
pub fn owner_day(journal: &Path, at: Timestamp) -> String {
    solstone_core_time::day_key(owner_zone(journal).to_datetime(at).date())
}

/// This computer's zone, or UTC on a host that names none. A `TZ` naming a
/// real zone wins, as it does for the system clock; otherwise the zone the
/// system is set to.
pub fn host_zone() -> TimeZone {
    std::env::var("TZ")
        .ok()
        .and_then(|name| parse_zone(name.trim_start_matches(':')))
        .or_else(|| {
            iana_time_zone::get_timezone()
                .ok()
                .and_then(|name| parse_zone(&name))
        })
        .unwrap_or(TimeZone::UTC)
}

/// The zone an import source fixed at its first import, which placed its windows.
/// `None` when it has no such record, or one that can't be read.
#[must_use]
pub fn placed_zone(journal: &std::path::Path, source: &str) -> Option<TimeZone> {
    let bytes = std::fs::read(journal.join(format!("imports/.{source}-zone.json"))).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    parse_zone(value.get("zone")?.as_str()?)
}

/// The IANA zone `name` names, ignoring surrounding whitespace; `None` for
/// anything that is not one. The name must be written as the time zone
/// database writes it (`America/Denver`, not `america/denver`).
pub fn parse_zone(name: &str) -> Option<TimeZone> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    solstone_core_time::zone(name)
}

/// The IANA name of `zone`, when it has one.
#[must_use]
pub fn zone_name(zone: &TimeZone) -> Option<&str> {
    zone.iana_name()
}

/// Describe `zone` for human-readable display. Named zones (e.g. `America/New_York`)
/// produce place names ("New York"). Zones without places (`UTC`, `Etc/...`, `None`)
/// produce offset labels ("UTC", "UTC+9", "UTC-3:30").
pub fn zone_label(zone: Option<&TimeZone>, offset_seconds: i32) -> String {
    zone.and_then(place_name)
        .unwrap_or_else(|| offset_label(offset_seconds))
}

fn place_name(zone: &TimeZone) -> Option<String> {
    let name = zone.iana_name()?;
    if name.starts_with("Etc/") {
        return None;
    }
    let (_, place) = name.rsplit_once('/')?;
    Some(place.replace('_', " "))
}

fn offset_label(offset: i32) -> String {
    if offset == 0 {
        return "UTC".to_owned();
    }
    let sign = if offset < 0 { '-' } else { '+' };
    let minutes = offset.unsigned_abs() / 60;
    let (hours, minutes) = (minutes / 60, minutes % 60);
    if minutes == 0 {
        format!("UTC{sign}{hours}")
    } else {
        format!("UTC{sign}{hours}:{minutes:02}")
    }
}

fn configured_zone(journal: &Path) -> Option<TimeZone> {
    let config = read_journal_config(journal).ok()?.config?;
    config
        .get("identity")?
        .get("timezone")?
        .as_str()
        .and_then(parse_zone)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use jiff::tz::TimeZone;
    use serde_json::json;

    use super::{host_zone, owner_day, owner_zone, parse_zone, zone_label};
    use crate::test_support::TempDir;

    #[test]
    fn zone_labels_use_places_when_available_and_offsets_otherwise() {
        let zone = |name: &str| TimeZone::get(name).unwrap();
        assert_eq!(
            zone_label(Some(&zone("America/Argentina/Buenos_Aires")), -10800),
            "Buenos Aires"
        );
        assert_eq!(zone_label(Some(&zone("Asia/Tokyo")), 32400), "Tokyo");
        assert_eq!(zone_label(Some(&TimeZone::UTC), 0), "UTC");
        assert_eq!(zone_label(Some(&zone("Etc/GMT-9")), 32400), "UTC+9");
        assert_eq!(zone_label(None, -12600), "UTC-3:30");
        assert_eq!(zone_label(None, 0), "UTC");
        assert_eq!(zone_label(None, 7200), "UTC+2");
    }

    fn journal_with(identity: serde_json::Value) -> TempDir {
        let journal = TempDir::new();
        fs::create_dir_all(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/journal.json"),
            json!({ "identity": identity }).to_string(),
        )
        .unwrap();
        journal
    }

    #[test]
    fn a_configured_zone_wins() {
        let journal = journal_with(json!({ "timezone": " Asia/Tokyo " }));
        assert_eq!(owner_zone(journal.path()).iana_name(), Some("Asia/Tokyo"));
    }

    #[test]
    fn an_empty_or_unknown_zone_falls_back_to_this_computer() {
        for timezone in [json!(""), json!("Mars/Olympus_Mons"), json!(7)] {
            let journal = journal_with(json!({ "timezone": timezone }));
            assert_eq!(owner_zone(journal.path()), host_zone(), "{timezone}");
        }
        let missing = TempDir::new();
        assert_eq!(owner_zone(missing.path()), host_zone());
    }

    #[test]
    fn the_owner_day_is_the_date_in_the_owner_zone_across_utc_midnight() {
        let journal = journal_with(json!({ "timezone": "America/Denver" }));
        let at = |text: &str| text.parse::<jiff::Timestamp>().unwrap();
        // 03:30 UTC on the 17th is still the evening of the 16th in Denver.
        assert_eq!(
            owner_day(journal.path(), at("2026-11-17T03:30:00Z")),
            "20261116"
        );
        assert_eq!(
            owner_day(journal.path(), at("2026-11-17T07:30:00Z")),
            "20261117"
        );
        // The year turns in the owner's zone, not in UTC.
        assert_eq!(
            owner_day(journal.path(), at("2027-01-01T05:00:00Z")),
            "20261231"
        );
    }

    #[test]
    fn only_real_zone_names_parse() {
        let name =
            |name: &str| parse_zone(name).and_then(|zone| zone.iana_name().map(str::to_owned));
        assert_eq!(name("America/Denver").as_deref(), Some("America/Denver"));
        assert_eq!(name(" UTC ").as_deref(), Some("UTC"));
        assert_eq!(name("US/Mountain").as_deref(), Some("US/Mountain"));
        for name in [
            "",
            "  ",
            "Mountain Standard Time",
            "../etc",
            "america/denver",
            "/etc/localtime",
        ] {
            assert!(parse_zone(name).is_none(), "{name}");
        }
    }
}
