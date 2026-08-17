// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Daily-schedule hook stages.

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, NaiveTime};
use serde_json::{Map, Value};
use solstone_core_journal_io::{PathOrDay, iter_segments};

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq)]
pub struct DailySchedulePreState {
    activity_spans: String,
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let days = prepared
        .config
        .get("meta")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("lookback_days"))
        .and_then(Value::as_i64)
        .unwrap_or(7);
    Ok(PrePostState::DailySchedule(DailySchedulePreState {
        activity_spans: generate_span_summary(&context.journal, days),
    }))
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::DailySchedule(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "daily_schedule",
            prepared,
            "missing daily schedule state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([(
            "activity_spans".to_owned(),
            Value::String(state.activity_spans.clone()),
        )]),
    );
    Ok(())
}

pub fn parse(
    output: &str,
    _prepared: &PreparedTalent,
    _state: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.to_owned()))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    _state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "daily_schedule",
            prepared,
            "expected text output",
        ));
    };
    Ok(CommitPlan::Write(WriteIntent::DailySchedule {
        output,
        output_path: prepared
            .config
            .get("output_path")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }))
}

pub fn apply_result(journal: &std::path::Path, output: &str) -> Result<(), String> {
    // Preserve solstone/talent/daily_schedule.py:167-203: invalid output is ignored.
    let Ok(Value::Object(value)) = serde_json::from_str(output) else {
        return Ok(());
    };
    let Some(primary) = value.get("primary").and_then(Value::as_str) else {
        return Ok(());
    };
    if NaiveTime::parse_from_str(primary, "%H:%M").is_err() {
        return Ok(());
    }
    solstone_core_system::schedule::set_schedule_metadata(
        &journal.join("config/schedules.json"),
        &Map::from_iter([("daily_time".to_owned(), Value::String(primary.to_owned()))]),
    )
    .map_err(|error| error.to_string())
}

fn generate_span_summary(journal: &std::path::Path, days: i64) -> String {
    let end_date = Local::now().date_naive();
    let start_date = end_date - Duration::days(days - 1);
    let mut current = start_date;
    let mut lines = Vec::new();
    let mut days_with_data = 0;
    while current <= end_date {
        let day = current.format("%Y%m%d").to_string();
        let spans = build_spans(segment_ranges(journal, &day));
        if !spans.is_empty() {
            days_with_data += 1;
            lines.push(format!("{} ({}):", day, current.format("%A")));
            for (start, end) in spans {
                lines.push(format!(
                    "  {} - {} ({})",
                    start.format("%H:%M"),
                    end.format("%H:%M"),
                    format_duration(start, end)
                ));
            }
            lines.push(String::new());
        }
        current += Duration::days(1);
    }
    if days_with_data == 0 {
        return "No activity data found for the past week.".to_owned();
    }
    format!(
        "Activity windows for the past {days} days ({days_with_data} days with data):\n\n{}",
        lines.join("\n")
    )
}

fn segment_ranges(journal: &std::path::Path, day: &str) -> Vec<(NaiveDateTime, NaiveDateTime)> {
    let anchor = NaiveDate::from_ymd_opt(2000, 1, 1).expect("fixed date is valid");
    let mut ranges = iter_segments(journal, PathOrDay::Day(day))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|segment| parse_segment(&segment.key, anchor))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|(start, _)| *start);
    ranges
}

fn parse_segment(key: &str, anchor: NaiveDate) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let (clock, duration) = key.split_once('_')?;
    if clock.len() != 6 || !clock.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let seconds = duration.split('_').next()?.parse::<i64>().ok()?;
    let time = NaiveTime::parse_from_str(clock, "%H%M%S").ok()?;
    let start = anchor.and_time(time);
    Some((start, start + Duration::seconds(seconds)))
}

fn build_spans(
    segments: Vec<(NaiveDateTime, NaiveDateTime)>,
) -> Vec<(NaiveDateTime, NaiveDateTime)> {
    let Some((mut span_start, mut span_end)) = segments.first().copied() else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    for (start, end) in segments.into_iter().skip(1) {
        if (start - span_end).num_seconds() > 300 {
            if (span_end - span_start).num_minutes() >= 10 {
                spans.push((span_start, span_end));
            }
            span_start = start;
        }
        span_end = end;
    }
    if (span_end - span_start).num_minutes() >= 10 {
        spans.push((span_start, span_end));
    }
    spans
}

fn format_duration(start: NaiveDateTime, end: NaiveDateTime) -> String {
    let minutes = (end - start).num_seconds() / 60;
    let (hours, minutes) = (minutes / 60, minutes % 60);
    match (hours, minutes) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_groups_reference_segment_ranges() {
        // Derived from solstone/talent/daily_schedule.py:18-79.
        let day = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
        let first = parse_segment("090000_600", day).unwrap();
        let second = parse_segment("091100_600_audio", day).unwrap();
        assert_eq!(build_spans(vec![first, second]).len(), 1);
        assert!(parse_segment("not-a-segment", day).is_none());
    }

    #[test]
    fn primary_time_writes_only_schedule_metadata() {
        // Derived from solstone/talent/daily_schedule.py:167-203.
        let root = tempfile::tempdir().unwrap();
        apply_result(root.path(), r#"{"primary":"03:30"}"#).unwrap();
        let value: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value["daily_time"], "03:30");
        assert!(
            !root
                .path()
                .join("config/schedules.json")
                .join("daily_time")
                .exists()
        );
    }
}
