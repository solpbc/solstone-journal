// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jiff::SignedDuration;
use jiff::civil::{DateTime, Time};
use serde_json::Value;
use solstone_core_format::segment::segment_parse;

use crate::error::GrabFailure;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SegmentWindow {
    pub start: DateTime,
    pub end: DateTime,
}

pub(crate) fn segment_window(day: &str, segment: &str) -> Result<SegmentWindow, GrabFailure> {
    segment_window_in(solstone_core_cli_copy::ENGLISH, day, segment)
}

pub(crate) fn segment_window_in(
    locale: &str,
    day: &str,
    segment: &str,
) -> Result<SegmentWindow, GrabFailure> {
    let invalid_key = || {
        GrabFailure::runtime(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.invalid.segment.key",
            &[
                ("segment", solstone_core_cli_copy::Value::String(segment)),
                (
                    "format",
                    solstone_core_cli_copy::Value::String("HHMMSS_LEN"),
                ),
            ],
        ))
    };
    let date = solstone_core_time::parse_day(day).ok_or_else(invalid_key)?;
    let times = segment_parse(segment).ok_or_else(invalid_key)?;
    let (_, duration) = segment.split_once('_').ok_or_else(invalid_key)?;
    if duration.is_empty() || !duration.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_key());
    }
    let seconds = duration.parse::<i64>().map_err(|_| invalid_key())?;
    let start = date.to_datetime(
        Time::new(
            i8::try_from(times.hour).expect("segment_parse validates clock"),
            i8::try_from(times.minute).expect("segment_parse validates clock"),
            i8::try_from(times.second).expect("segment_parse validates clock"),
            0,
        )
        .expect("segment_parse validates clock"),
    );
    let candidate = start
        .checked_add(SignedDuration::from_secs(seconds))
        .map_err(|_| invalid_key())?;
    let end = if candidate.date() > date {
        date.at(23, 59, 59, 0)
    } else {
        candidate
    };
    Ok(SegmentWindow { start, end })
}

#[cfg(test)]
pub(crate) fn frame_abs_time(start: DateTime, timestamp: &Value) -> Result<String, GrabFailure> {
    frame_abs_time_in(solstone_core_cli_copy::ENGLISH, start, timestamp)
}

pub(crate) fn frame_abs_time_in(
    locale: &str,
    start: DateTime,
    timestamp: &Value,
) -> Result<String, GrabFailure> {
    let seconds = python_seconds(locale, timestamp)?;
    if !seconds.is_finite() {
        return Err(GrabFailure::runtime(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.timestamp.finite",
            &[],
        )));
    }
    // Python timedelta rounds fractional seconds to the nearest microsecond using
    // ties-to-even. f64::round_ties_even makes the same contract explicit.
    let micros = (seconds * 1_000_000.0).round_ties_even();
    if !(i64::MIN as f64..=i64::MAX as f64).contains(&micros) {
        return Err(GrabFailure::runtime(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.timestamp.range",
            &[],
        )));
    }
    let value = start
        .checked_add(SignedDuration::from_micros(micros as i64))
        .map_err(|_| {
            GrabFailure::runtime(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.grab.error.timestamp.range",
                &[],
            ))
        })?;
    if value.subsec_nanosecond() == 0 {
        Ok(value.strftime("%Y-%m-%dT%H:%M:%S").to_string())
    } else {
        Ok(value.strftime("%Y-%m-%dT%H:%M:%S%.6f").to_string())
    }
}

fn python_seconds(locale: &str, timestamp: &Value) -> Result<f64, GrabFailure> {
    let invalid = || {
        GrabFailure::runtime(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.grab.error.timestamp.invalid",
            &[],
        ))
    };
    let value = match timestamp {
        Value::Null => 0.0,
        Value::Bool(false) => 0.0,
        Value::Bool(true) => 1.0,
        Value::Number(number) => number.as_f64().ok_or_else(invalid)?,
        Value::String(value) if value.is_empty() => 0.0,
        Value::Array(items) if items.is_empty() => 0.0,
        Value::Object(values) if values.is_empty() => 0.0,
        Value::String(value) => value.parse::<f64>().map_err(|_| invalid())?,
        _ => return Err(invalid()),
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use solstone_core_time::civil_display;

    use super::{frame_abs_time, segment_window};

    #[test]
    fn segment_end_clamps_but_frame_time_rolls_over() {
        let window = segment_window("20260809", "235900_300").unwrap();
        assert_eq!(civil_display(window.start), "2026-08-09 23:59:00");
        assert_eq!(civil_display(window.end), "2026-08-09 23:59:59");
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
        for timestamp in [json!([]), json!({})] {
            assert_eq!(
                frame_abs_time(start, &timestamp).unwrap(),
                "2026-08-09T12:00:00"
            );
        }
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
