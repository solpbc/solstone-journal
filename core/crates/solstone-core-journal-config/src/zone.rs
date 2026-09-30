// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's time zone: one answer for "today" and for reading a wall clock.

use std::path::Path;

pub use chrono_tz::Tz;

use crate::read::read_journal_config;

/// The zone the journal keeps the owner's days in: `identity.timezone` when it
/// names a real IANA zone, otherwise this computer's own zone. Setup fills the
/// setting from the browser that ran it, and the owner can change it.
pub fn owner_zone(journal: &Path) -> Tz {
    configured_zone(journal).unwrap_or_else(host_zone)
}

/// This computer's zone, or UTC on a host that names none. A `TZ` naming a
/// real zone wins, as it does for the system clock; otherwise the zone the
/// system is set to.
pub fn host_zone() -> Tz {
    std::env::var("TZ")
        .ok()
        .and_then(|name| parse_zone(name.trim_start_matches(':')))
        .or_else(|| {
            iana_time_zone::get_timezone()
                .ok()
                .and_then(|name| parse_zone(&name))
        })
        .unwrap_or(Tz::UTC)
}

/// The IANA zone `name` names, ignoring surrounding whitespace; `None` for
/// anything that is not one.
pub fn parse_zone(name: &str) -> Option<Tz> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    name.parse().ok()
}

/// Describe `zone` for human-readable display. Named zones (e.g. `America/New_York`)
/// produce place names ("New York"). Zones without places (`UTC`, `Etc/...`, `None`)
/// produce offset labels ("UTC", "UTC+9", "UTC-3:30").
pub fn zone_label(zone: Option<Tz>, offset_seconds: i32) -> String {
    zone.and_then(place_name)
        .unwrap_or_else(|| offset_label(offset_seconds))
}

fn place_name(zone: Tz) -> Option<String> {
    let name = zone.name();
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

fn configured_zone(journal: &Path) -> Option<Tz> {
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

    use chrono_tz::Tz;
    use serde_json::json;

    use super::{host_zone, owner_zone, parse_zone, zone_label};
    use crate::test_support::TempDir;

    #[test]
    fn zone_labels_use_places_when_available_and_offsets_otherwise() {
        assert_eq!(
            zone_label(Some(Tz::America__Argentina__Buenos_Aires), -10800),
            "Buenos Aires"
        );
        assert_eq!(zone_label(Some(Tz::Asia__Tokyo), 32400), "Tokyo");
        assert_eq!(zone_label(Some(Tz::UTC), 0), "UTC");
        assert_eq!(zone_label(Some(Tz::Etc__GMTMinus9), 32400), "UTC+9");
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
        assert_eq!(owner_zone(journal.path()), Tz::Asia__Tokyo);
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
    fn only_real_zone_names_parse() {
        assert_eq!(parse_zone("America/Denver"), Some(Tz::America__Denver));
        assert_eq!(parse_zone("UTC"), Some(Tz::UTC));
        for name in ["", "  ", "Mountain Standard Time", "../etc"] {
            assert_eq!(parse_zone(name), None, "{name}");
        }
    }
}
