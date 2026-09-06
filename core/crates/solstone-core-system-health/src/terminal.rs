// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Duration, NaiveDate};

use crate::event::HealthEvent;
use crate::read::read_day_records;
use crate::vocabulary::{CAP, DETERMINISTIC_FAILURE_REASON_CODES, MIN_SPAN_MS};
use crate::{
    CompletedUnit, CompletionActivity, CompletionSegment, CompletionsSince, DailyUnit,
    DeterministicFailure, FoldRead, HealthError, HealthLogSource, TerminalEvent, TerminalState,
    TerminalUnit,
};

#[derive(Debug, Clone)]
struct ObservedTerminal {
    ts: i64,
    sequence: usize,
    event: TerminalEvent,
    use_id: Option<String>,
    state: Option<String>,
    reason_code: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    cache_hit: bool,
}

pub fn read_terminal_states<S: HealthLogSource>(
    source: &S,
    day: &str,
    scope_to_day: bool,
) -> Result<FoldRead<BTreeMap<TerminalUnit, TerminalState>>, HealthError> {
    let scanned = read_day_records(source, day)?;
    let mut records: BTreeMap<TerminalUnit, Vec<ObservedTerminal>> = BTreeMap::new();
    let mut sequence = 0;
    for record in scanned.value {
        let event = match &record.event {
            HealthEvent::TalentComplete(_) => TerminalEvent::Complete,
            HealthEvent::TalentFail(_) => TerminalEvent::Fail,
            _ => continue,
        };
        let Some(payload) = record.event.payload() else {
            continue;
        };
        if scope_to_day
            && payload
                .day
                .as_deref()
                .is_some_and(|record_day| record_day != day)
        {
            continue;
        }
        let (Some(mode), Some(name)) = (payload.mode.clone(), payload.name.clone()) else {
            continue;
        };
        sequence += 1;
        let unit = TerminalUnit {
            mode,
            name,
            facet: payload.facet.clone(),
            stream: payload.stream.clone(),
            segment: payload.segment.clone(),
            activity: payload.activity.clone(),
        };
        records.entry(unit).or_default().push(ObservedTerminal {
            ts: record.ts,
            sequence,
            event,
            use_id: payload.use_id.clone(),
            state: payload.state.clone(),
            reason_code: payload.reason_code.clone(),
            provider: payload.provider.clone(),
            model: payload.model.clone(),
            cache_hit: payload.cache_hit == Some(true),
        });
    }
    let states = records
        .into_iter()
        .map(|(unit, mut terminals)| {
            terminals.sort_by_key(|item| (item.ts, item.sequence));
            let latest = terminals.last().expect("terminal list is non-empty");
            let last_real_complete_ts = terminals
                .iter()
                .filter(|item| item.event == TerminalEvent::Complete && !item.cache_hit)
                .map(|item| item.ts)
                .max();
            let mut trailing_fail_count = 0;
            let mut oldest_trailing_fail_ts = None;
            for terminal in terminals.iter().rev() {
                if terminal.event != TerminalEvent::Fail {
                    break;
                }
                trailing_fail_count += 1;
                oldest_trailing_fail_ts = Some(terminal.ts);
            }
            let deterministic_fail_count = terminals
                .iter()
                .rev()
                .take_while(|item| item.event != TerminalEvent::Complete)
                .filter(|item| {
                    item.event == TerminalEvent::Fail
                        && item.reason_code.as_deref().is_some_and(is_deterministic)
                })
                .count();
            let last_fail = terminals
                .iter()
                .rev()
                .find(|item| item.event == TerminalEvent::Fail);
            (
                unit,
                TerminalState {
                    latest_event: latest.event,
                    latest_ts: latest.ts,
                    last_real_complete_ts,
                    trailing_fail_count,
                    deterministic_fail_count,
                    last_fail_ts: last_fail.map(|item| item.ts),
                    use_id: last_fail.and_then(|item| item.use_id.clone()),
                    state: last_fail.and_then(|item| item.state.clone()),
                    reason_code: last_fail.and_then(|item| item.reason_code.clone()),
                    provider: last_fail.and_then(|item| item.provider.clone()),
                    model: last_fail.and_then(|item| item.model.clone()),
                    oldest_trailing_fail_ts,
                },
            )
        })
        .collect();
    Ok(FoldRead {
        value: states,
        malformed_line_count: scanned.malformed_line_count,
    })
}

pub fn is_floor_talent_capped<S: HealthLogSource>(
    source: &S,
    day: &str,
    stream: Option<&str>,
    segment: &str,
    name: &str,
) -> Result<FoldRead<bool>, HealthError> {
    let states = read_terminal_states(source, day, false)?;
    let unit = TerminalUnit {
        mode: "segment".to_owned(),
        name: name.to_owned(),
        facet: None,
        stream: stream.map(str::to_owned),
        segment: Some(segment.to_owned()),
        activity: None,
    };
    let capped = states.value.get(&unit).is_some_and(|state| {
        state.trailing_fail_count >= CAP
            && state
                .oldest_trailing_fail_ts
                .zip(state.last_fail_ts)
                .is_some_and(|(oldest, latest)| latest - oldest >= MIN_SPAN_MS)
    });
    Ok(FoldRead {
        value: capped,
        malformed_line_count: states.malformed_line_count,
    })
}

pub fn read_completed_units<S: HealthLogSource>(
    source: &S,
    day: &str,
) -> Result<FoldRead<BTreeSet<CompletedUnit>>, HealthError> {
    let states = read_terminal_states(source, day, false)?;
    let units = states
        .value
        .into_iter()
        .filter_map(|(unit, state)| {
            (unit.segment.is_none()
                && unit.activity.is_none()
                && state.latest_event == TerminalEvent::Complete)
                .then_some(CompletedUnit {
                    mode: unit.mode,
                    name: unit.name,
                    facet: unit.facet,
                })
        })
        .collect();
    Ok(FoldRead {
        value: units,
        malformed_line_count: states.malformed_line_count,
    })
}

pub fn read_completed_since<S: HealthLogSource>(
    source: &S,
    day: &str,
    since_ms: i64,
) -> Result<FoldRead<CompletionsSince>, HealthError> {
    let current = NaiveDate::parse_from_str(day, "%Y%m%d")
        .map_err(|_| HealthError::InvalidDay(day.to_owned()))?;
    let previous = (current - Duration::days(1)).format("%Y%m%d").to_string();
    let mut segments: BTreeMap<(String, Option<String>, String), i64> = BTreeMap::new();
    let mut activities: BTreeMap<(String, Option<String>, String), i64> = BTreeMap::new();
    let mut malformed_line_count = 0;
    for scan_day in [day, previous.as_str()] {
        let states = read_terminal_states(source, scan_day, false)?;
        malformed_line_count += states.malformed_line_count;
        for (unit, state) in states.value {
            let Some(ts) = state.last_real_complete_ts else {
                continue;
            };
            if state.latest_event != TerminalEvent::Complete || ts <= since_ms {
                continue;
            }
            if let Some(segment) = unit.segment.filter(|value| !value.is_empty()) {
                segments
                    .entry((scan_day.to_owned(), unit.stream, segment))
                    .and_modify(|current| *current = (*current).max(ts))
                    .or_insert(ts);
            } else if let Some(activity) = unit.activity.filter(|value| !value.is_empty()) {
                activities
                    .entry((scan_day.to_owned(), unit.facet, activity))
                    .and_modify(|current| *current = (*current).max(ts))
                    .or_insert(ts);
            }
        }
    }
    let mut segment_values = segments
        .into_iter()
        .map(|((day, stream, segment), ts)| CompletionSegment {
            day,
            stream,
            segment,
            ts,
        })
        .collect::<Vec<_>>();
    segment_values.sort_by_key(|item| {
        (
            item.ts,
            item.day.clone(),
            item.stream.clone().unwrap_or_default(),
            item.segment.clone(),
        )
    });
    let mut activity_values = activities
        .into_iter()
        .map(|((day, facet, activity), ts)| CompletionActivity {
            day,
            facet,
            activity,
            ts,
        })
        .collect::<Vec<_>>();
    activity_values.sort_by_key(|item| {
        (
            item.ts,
            item.day.clone(),
            item.facet.clone().unwrap_or_default(),
            item.activity.clone(),
        )
    });
    Ok(FoldRead {
        value: CompletionsSince {
            segments: segment_values,
            activities: activity_values,
        },
        malformed_line_count,
    })
}

pub fn read_daily_deterministic_failures<S: HealthLogSource>(
    source: &S,
    day: &str,
) -> Result<FoldRead<BTreeMap<DailyUnit, DeterministicFailure>>, HealthError> {
    let states = read_terminal_states(source, day, false)?;
    let failures = states
        .value
        .into_iter()
        .filter_map(|(unit, state)| {
            (unit.mode == "daily"
                && unit.segment.is_none()
                && unit.activity.is_none()
                && state.latest_event == TerminalEvent::Fail)
                .then_some((unit, state))
        })
        .filter_map(|(unit, state)| {
            let reason = state
                .reason_code
                .filter(|reason| is_deterministic(reason))?;
            Some((
                DailyUnit {
                    name: unit.name,
                    facet: unit.facet,
                },
                DeterministicFailure {
                    count: state.deterministic_fail_count,
                    reason_code: reason,
                },
            ))
        })
        .collect();
    Ok(FoldRead {
        value: failures,
        malformed_line_count: states.malformed_line_count,
    })
}

fn is_deterministic(reason: &str) -> bool {
    DETERMINISTIC_FAILURE_REASON_CODES.contains(&reason)
}
