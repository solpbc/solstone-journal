// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Daily-schedule hook stages.

use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use serde_json::{Map, Value};
use solstone_core_format::segment::{segment_key, segment_start_and_end_seconds};
use solstone_core_journal_io::{PathOrDay, iter_segments};

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
    let anchor = solstone_core_indexer::daily_evidence::journal_today(&context.journal);
    let activity_spans = generate_span_summary(&context.journal, days, anchor).map_err(|e| {
        RuntimeOutcome::StageFailed(stage_error("build", "daily_schedule", prepared, e))
    })?;
    Ok(PrePostState::DailySchedule(DailySchedulePreState {
        activity_spans,
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
    apply_result_cancellable(
        journal,
        output,
        &solstone_core_generate::Cancellation::never(),
    )
}

pub(crate) fn apply_result_cancellable(
    journal: &std::path::Path,
    output: &str,
    cancellation: &solstone_core_generate::Cancellation,
) -> Result<(), String> {
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
    solstone_core_system::schedule::mutate_schedule_entries(
        &journal.join("config/schedules.json"),
        |raw| {
            if cancellation.is_cancelled() {
                return solstone_core_system::schedule::ScheduleMutation {
                    changed: false,
                    value: Err("cancelled".to_owned()),
                };
            }
            raw.insert("daily_time".to_owned(), Value::String(primary.to_owned()));
            solstone_core_system::schedule::ScheduleMutation {
                changed: true,
                value: Ok(()),
            }
        },
    )
    .map_err(|error| error.to_string())?
}

fn generate_span_summary(
    journal: &std::path::Path,
    days: i64,
    end_date: NaiveDate,
) -> Result<String, String> {
    if !(1..=366).contains(&days) {
        return Err("daily schedule lookback outside 1..=366".to_owned());
    }
    let start_date = end_date - Duration::days(days - 1);
    let mut current = start_date;
    let mut lines = Vec::new();
    let mut days_with_data = 0;
    while current <= end_date {
        let day = current.format("%Y%m%d").to_string();
        let spans = build_spans(segment_ranges(journal, &day)?);
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
        return Ok("No activity data found for the past week.".to_owned());
    }
    Ok(format!(
        "Activity windows for the past {days} days ({days_with_data} days with data):\n\n{}",
        lines.join("\n")
    ))
}

fn segment_ranges(
    journal: &std::path::Path,
    day: &str,
) -> Result<Vec<(NaiveDateTime, NaiveDateTime)>, String> {
    let anchor = NaiveDate::from_ymd_opt(2000, 1, 1).expect("fixed date is valid");
    // Calendar notes say what the owner planned, not when they were active.
    let mut ranges = iter_segments(journal, PathOrDay::Day(day))
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|segment| !solstone_core_segment::is_calendar_segment(journal, segment))
        .filter_map(|segment| parse_segment(segment.key(), anchor))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|(start, _)| *start);
    Ok(ranges)
}

/// A segment's window on its own day: the end is clamped to the day's final
/// second, so a long segment never reads as activity into the next morning.
fn parse_segment(key: &str, anchor: NaiveDate) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let key = segment_key(key)?;
    let (times, end_seconds) = segment_start_and_end_seconds(&key)?;
    let start = anchor.and_hms_opt(
        u32::from(times.hour),
        u32::from(times.minute),
        u32::from(times.second),
    )?;
    let end = anchor.and_hms_opt(0, 0, 0)? + Duration::seconds(i64::try_from(end_seconds).ok()?);
    Some((start, end))
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
            span_end = end;
        } else {
            span_end = span_end.max(end);
        }
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
    fn calendar_segments_are_not_activity_windows() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        std::fs::create_dir_all(root.join("streams")).unwrap();
        std::fs::write(
            root.join("streams/phone_calendar.json"),
            r#"{"name":"phone_calendar","kind":"observer","host":null,"platform":null,"created_at":0,"last_day":null,"last_segment":null,"seq":1,"cid":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","source":"calendar"}"#,
        )
        .unwrap();
        for (stream, key) in [
            ("phone_calendar", "094000_300"),
            ("phone_calendar", "094500_300"),
            ("phone_calendar", "095000_300"),
            ("mic", "140000_300"),
            ("mic", "140500_300"),
            ("mic", "141000_300"),
        ] {
            let dir = root.join("chronicle/20261019").join(stream).join(key);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("note.jsonl"), "{}\n").unwrap();
        }
        let ranges = segment_ranges(root, "20261019").unwrap();
        let starts = ranges
            .iter()
            .map(|(start, _)| start.format("%H:%M").to_string())
            .collect::<Vec<_>>();
        assert_eq!(starts, ["14:00", "14:05", "14:10"]);
        let summary =
            generate_span_summary(root, 1, NaiveDate::from_ymd_opt(2026, 10, 19).unwrap()).unwrap();
        assert!(summary.contains("14:00 - 14:15"), "{summary}");
        assert!(!summary.contains("09:40"), "{summary}");
    }

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
    fn a_long_segment_stays_on_its_own_day_and_never_shortens_a_span() {
        let day = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
        let long = parse_segment("201347_42324", day).unwrap();
        assert_eq!(long.0, day.and_hms_opt(20, 13, 47).unwrap());
        assert_eq!(long.1, day.and_hms_opt(23, 59, 59).unwrap());

        let inside = parse_segment("210000_300", day).unwrap();
        assert_eq!(build_spans(vec![long, inside]), vec![long]);
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
