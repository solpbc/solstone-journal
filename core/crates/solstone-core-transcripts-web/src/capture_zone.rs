// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The zone a segment's wall clock was keyed in, as its device reported it.
//!
//! A segment's `day` and `HHMMSS` are the recording device's own wall clock.
//! When the device reported which zone that was, the segment page names it
//! wherever it differs from the zone this journal shows other times in.

use jiff::tz::TimeZone;
use serde_json::{Value, json};
use solstone_core_callosum::ReportedZone;

use std::path::Path;

use crate::attach::TranscriptSegment;

/// Describe `reported` for the segment page, measured against `host`, the
/// zone the journal renders its own times in. `Value::Null` when the device
/// reported nothing usable.
pub(crate) fn capture_zone_view(
    day: &str,
    key: &str,
    reported: Option<ReportedZone>,
    host: &TimeZone,
) -> Value {
    let Some(reported) = reported else {
        return Value::Null;
    };
    let Some(view) = solstone_core_journal_config::capture_zone_view(
        day,
        key,
        reported.tz.as_deref(),
        reported.utc_offset_seconds,
        host,
    ) else {
        return Value::Null;
    };
    json!({
        "tz": view.tz,
        "utc_offset_seconds": view.utc_offset_seconds,
        "label": view.label,
        "differs": view.differs,
    })
}

/// The other clocks a day's segments were keyed on, for one line under the
/// day heading. `Value::Null` when no segment differs from the home clock.
pub(crate) fn day_zones(
    root: &Path,
    day: &str,
    segments: &[TranscriptSegment],
    home: &TimeZone,
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
            solstone_core_callosum::read_reported_zone(root, &dir),
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use solstone_core_callosum::ReportedZone;

    use super::{capture_zone_view, summarize_day_zones};

    fn zone(tz: Option<&str>, offset: Option<i32>) -> Option<ReportedZone> {
        Some(ReportedZone {
            tz: tz.map(str::to_owned),
            utc_offset_seconds: offset,
        })
    }

    #[test]
    fn a_day_names_each_other_clock_once_and_says_when_all_of_it_was_away() {
        let home = &solstone_core_time::zone("America/Denver").expect("zone");
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
