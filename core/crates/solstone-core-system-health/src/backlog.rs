// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded cross-day backlog fold for thinking health.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{
    BACKLOG_STATE_COMPLETE, BACKLOG_STATE_PENDING, BACKLOG_STATE_STUCK, BACKLOG_STATE_UNKNOWN,
    BacklogDay, BacklogError, BacklogUnit, BacklogView, CappedDailySummary, CappedDailyUnit,
    DailyUnit, DeterministicFailure, HealthError, HealthLogSource, NO_SENSE_COMPLETE_AGED_MS,
    REASON_CATCHUP_BACKOFF, REASON_CORRUPT_RAW, REASON_FAILING_STEP,
    REASON_SEGMENT_REPAIR_DEGRADED, REASON_SEGMENT_REPAIR_PROGRESSING, REASON_SEGMENT_REPAIR_STUCK,
    REASON_SEGMENT_REPAIR_UNKNOWN, SEGMENT_REPAIR_STATUS_DEGRADED,
    SEGMENT_REPAIR_STATUS_PROGRESSING, SEGMENT_REPAIR_STATUS_STUCK, SEGMENT_REPAIR_STATUS_UNKNOWN,
    STUCK_FAIL_THRESHOLD, SegmentInput, SegmentProgress, SegmentRepairSummary, SegmentSource,
    TerminalEvent, TerminalState, TerminalUnit, WHY_CORRUPT_RAW, WHY_FAILED, WHY_NEVER_ATTEMPTED,
    WHY_NO_SENSE_COMPLETE_AGED, WHY_SENSED_NOT_THOUGHT, classify_segment_completion,
    day_is_complete, lookup_segment_progress, read_backoff_summary,
    read_daily_deterministic_failures, read_segment_progress, read_segment_repair_attempted,
    read_segment_repair_summary, read_terminal_states, scan_day, segment_fully_sensed,
    segment_fully_thought,
};

/// Return a bounded, read-only cross-day processing backlog report.
pub fn read_backlog_view<H: HealthLogSource, S: SegmentSource>(
    health_source: &H,
    segment_source: &S,
    journal: &Path,
    window: usize,
    now: DateTime<Utc>,
) -> Result<BacklogView, HealthError> {
    let mut days = solstone_core_journal_io::day_dirs(journal)?
        .into_keys()
        .collect::<Vec<_>>();
    days.sort_by(|left, right| right.cmp(left));
    days.truncate(window);

    let mut backlog_days = Vec::new();
    let mut errors = Vec::new();
    let mut malformed_line_count = 0;
    for day in days {
        let repair = read_segment_repair_summary(journal, &day);
        let repair_attempted = read_segment_repair_attempted(journal, &day);
        match day_is_complete(journal, &day) {
            Ok(true) => match complete_backlog_day(health_source, &day, repair.as_ref()) {
                Ok((day_value, malformed)) => {
                    malformed_line_count += malformed;
                    backlog_days.push(day_value);
                }
                Err(error) => backlog_days.push(unknown_day(&day, "capped_daily", error)),
            },
            Ok(false) => {
                let terminals = match read_terminal_states(health_source, &day, false) {
                    Ok(value) => value,
                    Err(error) => {
                        let failed = unknown_day(&day, "terminal_states", error);
                        errors.push(failed.error.clone().expect("unknown day has error"));
                        backlog_days.push(failed);
                        continue;
                    }
                };
                malformed_line_count += terminals.malformed_line_count;

                let progress = match read_segment_progress(health_source, &day) {
                    Ok(value) => value,
                    Err(error) => {
                        let failed = unknown_day(&day, "segment_completion", error);
                        errors.push(failed.error.clone().expect("unknown day has error"));
                        backlog_days.push(failed);
                        continue;
                    }
                };
                malformed_line_count += progress.malformed_line_count;
                let scanned = match scan_day(segment_source, journal, &day, now) {
                    Ok((_, _, segments)) => segments,
                    Err(error) => {
                        let failed = unknown_day(&day, "segment_completion", error);
                        errors.push(failed.error.clone().expect("unknown day has error"));
                        backlog_days.push(failed);
                        continue;
                    }
                };
                let inputs = scanned
                    .iter()
                    .cloned()
                    .map(SegmentInput::from)
                    .collect::<Vec<_>>();
                let completion = classify_segment_completion(&inputs, &progress.value);
                let stream_updated_ms = stream_updated_ms(journal, &day);
                let mut why = segment_backlog_units(
                    journal,
                    &day,
                    &scanned,
                    &progress.value,
                    &terminals.value,
                    stream_updated_ms,
                    repair_attempted,
                    now.timestamp_millis(),
                );
                why.extend(non_segment_failed_units(
                    &terminals.value,
                    stream_updated_ms,
                ));
                let backoff = read_backoff_summary(journal, &day);
                let segment_depth = completion.not_sensed + completion.not_thought;
                let mut reason = if why
                    .iter()
                    .any(|unit| unit.why == WHY_CORRUPT_RAW && unit.stuck)
                {
                    Some(REASON_CORRUPT_RAW.to_owned())
                } else if why.iter().any(|unit| unit.stuck) {
                    Some(REASON_FAILING_STEP.to_owned())
                } else {
                    None
                };
                let representative = representative_reason_unit(&why);
                let mut reason_code = representative.and_then(|unit| unit.reason_code.clone());
                let representative_provider = representative.and_then(|unit| unit.provider.clone());
                let representative_model = representative.and_then(|unit| unit.model.clone());
                if reason.is_none() && backoff.is_some() {
                    reason = Some(REASON_CATCHUP_BACKOFF.to_owned());
                    reason_code = Some(REASON_CATCHUP_BACKOFF.to_owned());
                }
                let state = if why.iter().any(|unit| unit.stuck) || backoff.is_some() {
                    BACKLOG_STATE_STUCK
                } else if segment_depth > 0 || !why.is_empty() {
                    BACKLOG_STATE_PENDING
                } else {
                    BACKLOG_STATE_COMPLETE
                };
                let (state, reason, reason_code, error) = escalate_for_repair(
                    state.to_owned(),
                    reason,
                    reason_code,
                    None,
                    &day,
                    repair.as_ref(),
                );
                backlog_days.push(BacklogDay {
                    day,
                    state,
                    segments: segment_depth,
                    units: why.len(),
                    not_sensed: completion.not_sensed,
                    why,
                    reason,
                    reason_code,
                    provider: representative_provider,
                    model: representative_model,
                    error,
                    backoff,
                    segment_repair: repair,
                    capped_daily: None,
                });
            }
            Err(error) => backlog_days.push(unknown_day(&day, "day_is_complete", error)),
        }
    }

    let pending_days = backlog_days
        .iter()
        .filter(|day| day.state == BACKLOG_STATE_PENDING)
        .count();
    let stuck_days = backlog_days
        .iter()
        .filter(|day| day.state == BACKLOG_STATE_STUCK)
        .count();
    let oldest_pending_day = backlog_days
        .iter()
        .filter(|day| {
            matches!(
                day.state.as_str(),
                BACKLOG_STATE_PENDING | BACKLOG_STATE_STUCK
            )
        })
        .map(|day| day.day.clone())
        .min();
    let degraded = !errors.is_empty()
        || backlog_days
            .iter()
            .any(|day| day.state == BACKLOG_STATE_UNKNOWN);
    Ok(BacklogView {
        window,
        days: backlog_days,
        pending_days,
        stuck_days,
        oldest_pending_day,
        errors,
        degraded,
        malformed_line_count,
    })
}

fn complete_backlog_day<H: HealthLogSource>(
    health_source: &H,
    day: &str,
    repair: Option<&SegmentRepairSummary>,
) -> Result<(BacklogDay, usize), HealthError> {
    let daily = read_daily_deterministic_failures(health_source, day)?;
    let capped_daily = capped_daily_summary(daily.value);
    let (state, reason, reason_code, error) = escalate_for_repair(
        BACKLOG_STATE_COMPLETE.to_owned(),
        None,
        None,
        None,
        day,
        repair,
    );
    Ok((
        BacklogDay {
            day: day.to_owned(),
            state,
            segments: 0,
            units: 0,
            not_sensed: 0,
            why: Vec::new(),
            reason,
            reason_code,
            provider: None,
            model: None,
            error,
            backoff: None,
            segment_repair: repair.cloned(),
            capped_daily,
        },
        daily.malformed_line_count,
    ))
}

fn capped_daily_summary(
    failures: BTreeMap<DailyUnit, DeterministicFailure>,
) -> Option<CappedDailySummary> {
    let capped = failures
        .into_iter()
        .filter(|(_, failure)| failure_capped(Some(&failure.reason_code), failure.count))
        .collect::<Vec<_>>();
    let count = capped.len();
    let ((unit, failure), _) = capped
        .into_iter()
        .map(|item| {
            let key = (
                item.0.name.clone(),
                item.0.facet.clone().unwrap_or_default(),
            );
            (item, key)
        })
        .min_by(|left, right| left.1.cmp(&right.1))?;
    Some(CappedDailySummary {
        count,
        unit: CappedDailyUnit {
            name: unit.name,
            facet: unit.facet,
            reason_code: failure.reason_code,
            count: failure.count,
        },
    })
}

fn unknown_day(day: &str, stage: &str, error: HealthError) -> BacklogDay {
    BacklogDay {
        day: day.to_owned(),
        state: BACKLOG_STATE_UNKNOWN.to_owned(),
        segments: 0,
        units: 0,
        not_sensed: 0,
        why: Vec::new(),
        reason: None,
        reason_code: None,
        provider: None,
        model: None,
        error: Some(BacklogError {
            day: day.to_owned(),
            stage: stage.to_owned(),
            message: error.to_string(),
        }),
        backoff: None,
        segment_repair: None,
        capped_daily: None,
    }
}

fn stream_updated_ms(journal: &Path, day: &str) -> Option<i64> {
    match solstone_core_journal_io::read_health_marker(
        journal,
        day,
        solstone_core_journal_io::HealthMarkerKind::Stream,
    )
    .ok()?
    {
        solstone_core_journal_io::HealthMarkerState::Absent => None,
        solstone_core_journal_io::HealthMarkerState::LegacyEmpty { modified }
        | solstone_core_journal_io::HealthMarkerState::MalformedNonEmpty { modified }
        | solstone_core_journal_io::HealthMarkerState::Versioned { modified, .. } => {
            Some(system_time_ms(modified))
        }
    }
}

fn system_time_ms(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

#[allow(clippy::too_many_arguments)]
fn segment_backlog_units(
    journal: &Path,
    day: &str,
    segments: &[crate::DaySegment],
    progress: &BTreeMap<crate::SegmentIdentity, SegmentProgress>,
    terminal_states: &BTreeMap<TerminalUnit, TerminalState>,
    stream_updated_ms: Option<i64>,
    repair_attempted: bool,
    now_ms: i64,
) -> Vec<BacklogUnit> {
    let mut why = Vec::new();
    for segment in segments {
        if !segment_fully_sensed(&segment.data_state) {
            for (modality, state) in &segment.data_state.0 {
                if state != "failed" {
                    continue;
                }
                let Some((reason, failed_at_ms)) =
                    read_failed_marker(journal, day, &segment.stream, &segment.key, modality)
                else {
                    continue;
                };
                if reason != "marker_corrupt"
                    || stream_updated_ms.is_some_and(|updated| updated > failed_at_ms)
                {
                    continue;
                }
                why.push(BacklogUnit {
                    mode: "segment".to_owned(),
                    name: modality.clone(),
                    facet: None,
                    stream: Some(segment.stream.clone()),
                    segment: Some(segment.key.clone()),
                    why: WHY_CORRUPT_RAW.to_owned(),
                    reason_code: None,
                    provider: None,
                    model: None,
                    trailing_fail_count: 0,
                    last_fail_ts: Some(failed_at_ms),
                    stuck: true,
                });
            }
            continue;
        }
        let segment_progress = lookup_segment_progress(progress, &segment.stream, &segment.key);
        match segment_fully_thought(segment_progress) {
            crate::ThoughtVerdict::Complete => continue,
            crate::ThoughtVerdict::NoSenseComplete => {
                if !repair_attempted
                    && stream_updated_ms.is_some_and(|updated| {
                        now_ms.saturating_sub(updated) >= NO_SENSE_COMPLETE_AGED_MS
                    })
                {
                    why.push(pending_unit(
                        "sense",
                        &segment.stream,
                        &segment.key,
                        WHY_NO_SENSE_COMPLETE_AGED,
                        stream_updated_ms,
                    ));
                }
            }
            crate::ThoughtVerdict::Floor(name) => {
                let unit = terminal_unit_for_segment(&name, &segment.stream, &segment.key);
                if let Some(state) = terminal_states
                    .get(&unit)
                    .filter(|state| state.latest_event == TerminalEvent::Fail)
                {
                    why.push(failed_backlog_unit(&unit, state, stream_updated_ms));
                } else if segment_progress
                    .is_some_and(|progress| progress.dispatched.contains(&name))
                {
                    why.push(pending_unit(
                        &name,
                        &segment.stream,
                        &segment.key,
                        WHY_SENSED_NOT_THOUGHT,
                        None,
                    ));
                } else {
                    why.push(pending_unit(
                        &name,
                        &segment.stream,
                        &segment.key,
                        WHY_NEVER_ATTEMPTED,
                        None,
                    ));
                }
            }
            crate::ThoughtVerdict::Dispatched(name) => {
                let unit = terminal_unit_for_segment(&name, &segment.stream, &segment.key);
                if let Some(state) = terminal_states
                    .get(&unit)
                    .filter(|state| state.latest_event == TerminalEvent::Fail)
                {
                    why.push(failed_backlog_unit(&unit, state, stream_updated_ms));
                } else {
                    why.push(pending_unit(
                        &name,
                        &segment.stream,
                        &segment.key,
                        WHY_SENSED_NOT_THOUGHT,
                        None,
                    ));
                }
            }
        }
    }
    why
}

fn read_failed_marker(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    modality: &str,
) -> Option<(String, i64)> {
    let day_dir = journal.join("chronicle").join(day);
    let segment_dir = if stream == solstone_core_journal_io::DEFAULT_STREAM {
        day_dir.join(segment)
    } else {
        day_dir.join(stream).join(segment)
    };
    let marker = segment_dir.join(format!(".analyze_failed_{modality}"));
    let data = serde_json::from_slice::<Value>(&fs::read(marker).ok()?).ok()?;
    let object = data.as_object()?;
    let reason = object.get("reason")?.as_str()?.to_owned();
    let failed_at = object.get("failed_at")?.as_str()?;
    let failed_at_ms = DateTime::parse_from_rfc3339(failed_at)
        .ok()?
        .timestamp_millis();
    Some((reason, failed_at_ms))
}

fn terminal_unit_for_segment(name: &str, stream: &str, segment: &str) -> TerminalUnit {
    TerminalUnit {
        mode: "segment".to_owned(),
        name: name.to_owned(),
        facet: None,
        stream: Some(stream.to_owned()),
        segment: Some(segment.to_owned()),
        activity: None,
    }
}

fn pending_unit(
    name: &str,
    stream: &str,
    segment: &str,
    why: &str,
    last_fail_ts: Option<i64>,
) -> BacklogUnit {
    BacklogUnit {
        mode: "segment".to_owned(),
        name: name.to_owned(),
        facet: None,
        stream: Some(stream.to_owned()),
        segment: Some(segment.to_owned()),
        why: why.to_owned(),
        reason_code: None,
        provider: None,
        model: None,
        trailing_fail_count: 0,
        last_fail_ts,
        stuck: false,
    }
}

fn failed_backlog_unit(
    unit: &TerminalUnit,
    state: &TerminalState,
    stream_updated_ms: Option<i64>,
) -> BacklogUnit {
    BacklogUnit {
        mode: unit.mode.clone(),
        name: unit.name.clone(),
        facet: unit.facet.clone(),
        stream: unit.stream.clone(),
        segment: unit.segment.clone(),
        why: WHY_FAILED.to_owned(),
        reason_code: state.reason_code.clone(),
        provider: state.provider.clone(),
        model: state.model.clone(),
        trailing_fail_count: state.trailing_fail_count,
        last_fail_ts: state.last_fail_ts,
        stuck: is_stuck(state, stream_updated_ms),
    }
}

fn is_stuck(state: &TerminalState, stream_updated_ms: Option<i64>) -> bool {
    state.latest_event == TerminalEvent::Fail
        && state.trailing_fail_count >= STUCK_FAIL_THRESHOLD
        && state.last_fail_ts.is_some()
        && stream_updated_ms
            .is_some_and(|updated| updated <= state.last_fail_ts.unwrap_or_default())
}

fn non_segment_failed_units(
    terminal_states: &BTreeMap<TerminalUnit, TerminalState>,
    stream_updated_ms: Option<i64>,
) -> Vec<BacklogUnit> {
    let mut states = terminal_states.iter().collect::<Vec<_>>();
    states.sort_by(|left, right| {
        let (left_unit, _) = left;
        let (right_unit, _) = right;
        (
            &left_unit.mode,
            &left_unit.name,
            left_unit.facet.as_deref().unwrap_or(""),
            left_unit.activity.as_deref().unwrap_or(""),
        )
            .cmp(&(
                &right_unit.mode,
                &right_unit.name,
                right_unit.facet.as_deref().unwrap_or(""),
                right_unit.activity.as_deref().unwrap_or(""),
            ))
    });
    states
        .into_iter()
        .filter(|(unit, state)| {
            unit.segment.is_none()
                && matches!(unit.mode.as_str(), "daily" | "activity" | "flush")
                && state.latest_event == TerminalEvent::Fail
        })
        .map(|(unit, state)| failed_backlog_unit(unit, state, stream_updated_ms))
        .collect()
}

fn representative_reason_unit(why: &[BacklogUnit]) -> Option<&BacklogUnit> {
    why.iter()
        .filter(|unit| unit.why == WHY_FAILED && unit.reason_code.is_some())
        .min_by(|left, right| {
            (
                &left.mode,
                &left.name,
                left.facet.as_deref().unwrap_or(""),
                left.stream.as_deref().unwrap_or(""),
                left.segment.as_deref().unwrap_or(""),
            )
                .cmp(&(
                    &right.mode,
                    &right.name,
                    right.facet.as_deref().unwrap_or(""),
                    right.stream.as_deref().unwrap_or(""),
                    right.segment.as_deref().unwrap_or(""),
                ))
        })
}

fn escalate_for_repair(
    mut state: String,
    mut reason: Option<String>,
    mut reason_code: Option<String>,
    mut error: Option<BacklogError>,
    day: &str,
    repair: Option<&SegmentRepairSummary>,
) -> (String, Option<String>, Option<String>, Option<BacklogError>) {
    let Some(repair) = repair else {
        return (state, reason, reason_code, error);
    };
    let (repair_state, repair_reason) = match repair.status.as_str() {
        SEGMENT_REPAIR_STATUS_DEGRADED => (BACKLOG_STATE_PENDING, REASON_SEGMENT_REPAIR_DEGRADED),
        SEGMENT_REPAIR_STATUS_PROGRESSING => {
            (BACKLOG_STATE_PENDING, REASON_SEGMENT_REPAIR_PROGRESSING)
        }
        SEGMENT_REPAIR_STATUS_STUCK => (BACKLOG_STATE_STUCK, REASON_SEGMENT_REPAIR_STUCK),
        SEGMENT_REPAIR_STATUS_UNKNOWN => (BACKLOG_STATE_UNKNOWN, REASON_SEGMENT_REPAIR_UNKNOWN),
        _ => unreachable!("catchup-state reader emitted an unknown repair status"),
    };
    if state_severity(repair_state) > state_severity(&state) {
        state = repair_state.to_owned();
    }
    if reason_code.is_none() {
        reason = Some(repair_reason.to_owned());
        reason_code = Some(repair_reason.to_owned());
    }
    if repair.status == SEGMENT_REPAIR_STATUS_UNKNOWN && error.is_none() {
        error = Some(BacklogError {
            day: day.to_owned(),
            stage: "segment_repair".to_owned(),
            message: "segment-repair state unreadable".to_owned(),
        });
    }
    (state, reason, reason_code, error)
}

fn state_severity(state: &str) -> usize {
    match state {
        BACKLOG_STATE_COMPLETE => 0,
        BACKLOG_STATE_PENDING => 1,
        BACKLOG_STATE_STUCK => 2,
        BACKLOG_STATE_UNKNOWN => 3,
        _ => 0,
    }
}

fn failure_capped(reason_code: Option<&str>, count: usize) -> bool {
    let cap = match reason_code {
        Some("model_not_found" | "provider_request_rejected") => 1,
        Some("schema_invalid") => 3,
        Some(
            "agent_stuck"
            | "context_window_exceeded"
            | "max_turns_exhausted"
            | "no_output"
            | "non_responsive"
            | "token_budget_exceeded"
            | "wall_clock_exceeded",
        ) => 2,
        _ => return false,
    };
    count >= cap
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use chrono::{DateTime, Utc};
    use tempfile::tempdir;

    use super::{is_stuck, segment_backlog_units};
    use crate::{
        DataStateMap, DaySegment, SegmentProgress, TerminalEvent, TerminalState, WHY_CORRUPT_RAW,
    };

    fn failed_terminal_state() -> TerminalState {
        TerminalState {
            latest_event: TerminalEvent::Fail,
            latest_ts: 4_000,
            last_real_complete_ts: None,
            trailing_fail_count: 3,
            deterministic_fail_count: 0,
            last_fail_ts: Some(4_000),
            use_id: None,
            state: None,
            reason_code: None,
            provider: None,
            model: None,
            oldest_trailing_fail_ts: Some(2_000),
        }
    }

    #[test]
    fn absent_stream_marker_never_makes_a_failed_terminal_unit_stuck() {
        assert!(!is_stuck(&failed_terminal_state(), None));
    }

    #[test]
    fn corrupt_raw_marker_is_stuck_without_a_stream_marker() {
        let temporary = tempdir().unwrap();
        let root = temporary.path();
        let day = "20990101";
        let segment = "120000_60";
        let directory = root.join("chronicle").join(day).join(segment);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(".analyze_failed_screen"),
            r#"{"reason":"marker_corrupt","failed_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let segments = vec![DaySegment {
            key: segment.to_owned(),
            stream: "_default".to_owned(),
            start: "12:00".to_owned(),
            end: "12:01".to_owned(),
            types: vec!["screen".to_owned()],
            data_state: DataStateMap(BTreeMap::from([("screen".to_owned(), "failed".to_owned())])),
        }];
        let why = segment_backlog_units(
            root,
            day,
            &segments,
            &BTreeMap::<_, SegmentProgress>::new(),
            &BTreeMap::new(),
            None,
            false,
            DateTime::<Utc>::from_timestamp_millis(1_800_000_000_000)
                .unwrap()
                .timestamp_millis(),
        );
        assert_eq!(why.len(), 1);
        assert_eq!(why[0].why, WHY_CORRUPT_RAW);
        assert!(why[0].stuck);
    }
}
