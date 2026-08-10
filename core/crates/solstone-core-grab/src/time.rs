// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use serde_json::Value;
use solstone_core_format::segment::segment_parse;

use crate::error::GrabFailure;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SegmentWindow {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
}

pub(crate) fn segment_window(day: &str, segment: &str) -> Result<SegmentWindow, GrabFailure> {
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|_| {
        GrabFailure::runtime(format!("segment {segment} is not a valid HHMMSS_LEN key"))
    })?;
    let times = segment_parse(segment).ok_or_else(|| {
        GrabFailure::runtime(format!("segment {segment} is not a valid HHMMSS_LEN key"))
    })?;
    let (_, duration) = segment.split_once('_').ok_or_else(|| {
        GrabFailure::runtime(format!("segment {segment} is not a valid HHMMSS_LEN key"))
    })?;
    if duration.is_empty() || !duration.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(GrabFailure::runtime(format!(
            "segment {segment} is not a valid HHMMSS_LEN key"
        )));
    }
    let seconds = duration.parse::<i64>().map_err(|_| {
        GrabFailure::runtime(format!("segment {segment} is not a valid HHMMSS_LEN key"))
    })?;
    let start = date.and_time(
        NaiveTime::from_hms_opt(
            u32::from(times.hour),
            u32::from(times.minute),
            u32::from(times.second),
        )
        .expect("segment_parse validates clock"),
    );
    let candidate = start
        .checked_add_signed(Duration::seconds(seconds))
        .ok_or_else(|| {
            GrabFailure::runtime(format!("segment {segment} is not a valid HHMMSS_LEN key"))
        })?;
    let end = if candidate.date() > date {
        date.and_hms_opt(23, 59, 59).expect("valid clamp time")
    } else {
        candidate
    };
    Ok(SegmentWindow { start, end })
}

pub(crate) fn frame_abs_time(
    start: NaiveDateTime,
    timestamp: &Value,
) -> Result<String, GrabFailure> {
    let seconds = python_seconds(timestamp)?;
    if !seconds.is_finite() {
        return Err(GrabFailure::runtime("timestamp must be finite"));
    }
    // Python timedelta rounds fractional seconds to the nearest microsecond using
    // ties-to-even. f64::round_ties_even makes the same contract explicit.
    let micros = (seconds * 1_000_000.0).round_ties_even();
    if !(i64::MIN as f64..=i64::MAX as f64).contains(&micros) {
        return Err(GrabFailure::runtime("timestamp is out of range"));
    }
    let value = start
        .checked_add_signed(Duration::microseconds(micros as i64))
        .ok_or_else(|| GrabFailure::runtime("timestamp is out of range"))?;
    if value.nanosecond() == 0 {
        Ok(value.format("%Y-%m-%dT%H:%M:%S").to_string())
    } else {
        Ok(value.format("%Y-%m-%dT%H:%M:%S%.6f").to_string())
    }
}

fn python_seconds(timestamp: &Value) -> Result<f64, GrabFailure> {
    let value = match timestamp {
        Value::Null => 0.0,
        Value::Bool(false) => 0.0,
        Value::Bool(true) => 1.0,
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| GrabFailure::runtime("invalid timestamp"))?,
        Value::String(value) if value.is_empty() => 0.0,
        Value::String(value) => value
            .parse::<f64>()
            .map_err(|_| GrabFailure::runtime("invalid timestamp"))?,
        _ => return Err(GrabFailure::runtime("invalid timestamp")),
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{frame_abs_time, segment_window};

    #[test]
    fn segment_end_clamps_but_frame_time_rolls_over() {
        let window = segment_window("20260809", "235900_300").unwrap();
        assert_eq!(window.start.to_string(), "2026-08-09 23:59:00");
        assert_eq!(window.end.to_string(), "2026-08-09 23:59:59");
        assert_eq!(
            frame_abs_time(window.start, &json!(90)).unwrap(),
            "2026-08-10T00:00:30"
        );
    }

    #[test]
    fn frame_time_matches_python_isoformat_shape() {
        let start = segment_window("20260809", "120000_1").unwrap().start;
        assert_eq!(
            frame_abs_time(start, &json!(null)).unwrap(),
            "2026-08-09T12:00:00"
        );
        assert_eq!(
            frame_abs_time(start, &json!(0.1)).unwrap(),
            "2026-08-09T12:00:00.100000"
        );
        assert_eq!(
            frame_abs_time(start, &json!("1.000001")).unwrap(),
            "2026-08-09T12:00:01.000001"
        );
    }

    #[test]
    fn segment_duration_requires_ascii_digits() {
        assert!(segment_window("20260809", "120000_+300").is_err());
        assert!(segment_window("20260809", "120000_-300").is_err());
    }
}
