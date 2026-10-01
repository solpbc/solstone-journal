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
use solstone_core_journal_config::zone_label;

use std::path::Path;

use crate::attach::TranscriptSegment;

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
        "label": zone_label(zone, offset),
        "differs": host_offset != offset,
    })
}

/// The other clocks a day's segments were keyed on, for one line under the
/// day heading. `Value::Null` when no segment differs from the home clock.
pub(crate) fn day_zones<H: TimeZone>(
    root: &Path,
    day: &str,
    segments: &[TranscriptSegment],
    home: &H,
) -> Value {
    let views = segments.iter().map(|segment| {
        let dir = root
            .join("chronicle")
            .join(crate::segment_media::segment_rel(
                day,
                &segment.stream,
                &segment.key,
            ));
        capture_zone_view(
            day,
            &segment.key,
            solstone_core_callosum::read_reported_zone(&dir),
            home,
        )
    });
    summarize_day_zones(views, segments.len())
}

/// Each differing zone once, in the order its first segment appears, and
/// whether every segment of the day was keyed on another clock. `place` says
/// the label names a place ("Tokyo") rather than an offset ("UTC+9").
fn summarize_day_zones(views: impl Iterator<Item = Value>, total: usize) -> Value {
    let mut zones: Vec<Value> = Vec::new();
    let mut differing = 0usize;
    for view in views {
        if view.get("differs").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        differing += 1;
        let Some(label) = view.get("label").and_then(Value::as_str) else {
            continue;
        };
        if zones.iter().any(|zone| zone["label"] == label) {
            continue;
        }
        let place = view
            .get("tz")
            .and_then(Value::as_str)
            .is_some_and(|tz| tz.contains('/') && !tz.starts_with("Etc/"));
        zones.push(json!({"label": label, "place": place}));
    }
    if zones.is_empty() {
        return Value::Null;
    }
    json!({"zones": zones, "all": differing == total})
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

#[cfg(test)]
mod tests {
    use chrono_tz::Tz;
    use serde_json::json;
    use solstone_core_callosum::ReportedZone;

    use serde_json::Value;

    use super::{capture_zone_view, summarize_day_zones};

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

    #[test]
    fn a_day_names_each_other_clock_once_and_says_when_all_of_it_was_away() {
        let home = &Tz::America__Denver;
        let tokyo = || {
            capture_zone_view(
                "20260930",
                "143000_300",
                zone(Some("Asia/Tokyo"), Some(32400)),
                home,
            )
        };
        let at_home = capture_zone_view(
            "20260930",
            "090000_300",
            zone(Some("America/Denver"), Some(-21600)),
            home,
        );
        let offset_only = capture_zone_view("20260930", "160000_300", zone(None, Some(3600)), home);

        let part = summarize_day_zones(
            vec![at_home.clone(), tokyo(), tokyo(), offset_only].into_iter(),
            4,
        );
        assert_eq!(part["all"], json!(false));
        assert_eq!(
            part["zones"],
            json!([{"label": "Tokyo", "place": true}, {"label": "UTC+1", "place": false}])
        );

        let away = summarize_day_zones(vec![tokyo(), tokyo()].into_iter(), 2);
        assert_eq!(away["all"], json!(true));

        assert!(summarize_day_zones(vec![at_home, Value::Null].into_iter(), 2).is_null());
    }
}
