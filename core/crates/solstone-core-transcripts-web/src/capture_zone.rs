// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The zone a segment's wall clock was keyed in, as its device reported it.
//!
//! A segment's `day` and `HHMMSS` are the recording device's own wall clock.
//! When the device reported which zone that was, the segment page names it
//! wherever it differs from the zone this journal shows other times in.

use chrono::{NaiveDateTime, Offset, TimeDelta, TimeZone};
use chrono_tz::Tz;
use serde_json::{Value, json};
use solstone_core_callosum::ReportedZone;

/// Describe `reported` for the segment page, measured against `host`, the
/// zone the journal renders its own times in. `Value::Null` when the device
/// reported nothing usable.
pub(crate) fn capture_zone_view<H: TimeZone>(
    day: &str,
    key: &str,
    reported: Option<ReportedZone>,
    host: &H,
) -> Value {
    let Some(reported) = reported else {
        return Value::Null;
    };
    let Some(wall) = segment_wall_start(day, key) else {
        return Value::Null;
    };
    let zone = reported
        .tz
        .as_deref()
        .and_then(|name| name.parse::<Tz>().ok());
    // The device's own offset is what keyed the segment; the zone name only
    // fills in when the device sent no offset.
    let Some(offset) = reported.utc_offset_seconds.or_else(|| {
        zone.and_then(|zone| zone.offset_from_local_datetime(&wall).earliest())
            .map(|offset| offset.fix().local_minus_utc())
    }) else {
        return Value::Null;
    };
    let start = wall - TimeDelta::seconds(i64::from(offset));
    let host_offset = host
        .offset_from_utc_datetime(&start)
        .fix()
        .local_minus_utc();
    json!({
        "tz": zone.map(|zone| zone.name()),
        "utc_offset_seconds": offset,
        "label": zone.and_then(place_name).unwrap_or_else(|| offset_label(offset)),
        "differs": host_offset != offset,
    })
}

fn segment_wall_start(day: &str, key: &str) -> Option<NaiveDateTime> {
    let hhmmss = key.get(..6)?;
    let digits =
        |text: &str, len: usize| text.len() == len && text.bytes().all(|b| b.is_ascii_digit());
    if !digits(day, 8) || !digits(hhmmss, 6) {
        return None;
    }
    NaiveDateTime::parse_from_str(&format!("{day}{hhmmss}"), "%Y%m%d%H%M%S").ok()
}

/// The place an IANA zone is named for: `America/New_York` reads "New York".
/// Zones named for no place (`UTC`, `Etc/GMT-9`) read as their offset.
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

#[cfg(test)]
mod tests {
    use chrono_tz::Tz;
    use serde_json::json;
    use solstone_core_callosum::ReportedZone;

    use super::capture_zone_view;

    fn zone(tz: Option<&str>, offset: Option<i32>) -> Option<ReportedZone> {
        Some(ReportedZone {
            tz: tz.map(str::to_owned),
            utc_offset_seconds: offset,
        })
    }

    #[test]
    fn a_phone_in_tokyo_differs_from_a_journal_in_denver() {
        assert_eq!(
            capture_zone_view(
                "20260929",
                "211400_300",
                zone(Some("Asia/Tokyo"), Some(32400)),
                &Tz::America__Denver,
            ),
            json!({"tz": "Asia/Tokyo", "utc_offset_seconds": 32400, "label": "Tokyo", "differs": true})
        );
    }

    #[test]
    fn the_same_zone_as_the_journal_does_not_differ() {
        let view = capture_zone_view(
            "20260929",
            "090000_300",
            zone(Some("America/Denver"), Some(-21600)),
            &Tz::America__Denver,
        );
        assert_eq!(view["differs"], json!(false));
        assert_eq!(view["label"], json!("Denver"));
    }

    #[test]
    fn the_offset_alone_is_enough_and_is_compared_at_the_segment_start() {
        // Edmonton shares Denver's offset, so a device there does not differ.
        let summer = capture_zone_view(
            "20260715",
            "120000_300",
            zone(None, Some(-21600)),
            &Tz::America__Denver,
        );
        assert_eq!(summer["differs"], json!(false));
        assert_eq!(summer["label"], json!("UTC-6"));
        // In January Denver is at -7, so the same -6 now differs.
        let winter = capture_zone_view(
            "20260115",
            "120000_300",
            zone(None, Some(-21600)),
            &Tz::America__Denver,
        );
        assert_eq!(winter["differs"], json!(true));
    }

    #[test]
    fn a_zone_name_alone_fills_in_the_offset_for_that_day() {
        let view = capture_zone_view(
            "20260115",
            "083000_300",
            zone(Some("Asia/Kolkata"), None),
            &Tz::America__Denver,
        );
        assert_eq!(view["utc_offset_seconds"], json!(19800));
        assert_eq!(view["label"], json!("Kolkata"));
        assert_eq!(view["differs"], json!(true));
    }

    #[test]
    fn places_read_as_places_and_placeless_zones_as_offsets() {
        let label = |tz: &str, offset: i32| {
            capture_zone_view(
                "20260929",
                "120000_300",
                zone(Some(tz), Some(offset)),
                &Tz::UTC,
            )["label"]
                .clone()
        };
        assert_eq!(
            label("America/Argentina/Buenos_Aires", -10800),
            json!("Buenos Aires")
        );
        assert_eq!(label("UTC", 0), json!("UTC"));
        assert_eq!(label("Etc/GMT-9", 32400), json!("UTC+9"));
        assert_eq!(label("Not/A_Zone", -12600), json!("UTC-3:30"));
    }

    #[test]
    fn nothing_usable_is_null() {
        assert!(capture_zone_view("20260929", "120000_300", None, &Tz::UTC).is_null());
        assert!(
            capture_zone_view(
                "20260929",
                "120000_300",
                zone(Some("Not/A_Zone"), None),
                &Tz::UTC
            )
            .is_null()
        );
        assert!(
            capture_zone_view("2026092", "120000_300", zone(None, Some(0)), &Tz::UTC).is_null()
        );
    }
}
