// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Duration, NaiveDate};

use crate::event::HealthEvent;
use crate::read::read_day_records;
use crate::vocabulary::{
    CAP, DETERMINISTIC_FAILURE_REASON_CODES, MIN_SPAN_MS, SEGMENT_NONGATING_TALENTS,
};
use crate::{
    CompletedUnit, CompletionActivity, CompletionSegment, CompletionsSince, DailyUnit,
    DeterministicFailure, FoldRead, HealthError, HealthLogSource, RunLogRecord, TerminalEvent,
    TerminalState, TerminalUnit,
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
    Ok(FoldRead {
        value: fold_terminal_records(
            scanned
                .value
                .into_iter()
                .map(|record| (day.to_owned(), record)),
            scope_to_day.then_some(day),
        ),
        malformed_line_count: scanned.malformed_line_count,
    })
}

fn group_observed_terminals(
    input: impl IntoIterator<Item = (String, RunLogRecord)>,
    scoped_day: Option<&str>,
) -> BTreeMap<TerminalUnit, Vec<ObservedTerminal>> {
    let mut records: BTreeMap<TerminalUnit, Vec<ObservedTerminal>> = BTreeMap::new();
    let mut sequence = 0;
    for (partition_day, record) in input {
        let event = match &record.event {
            HealthEvent::TalentComplete(_) => TerminalEvent::Complete,
            HealthEvent::TalentFail(_) => TerminalEvent::Fail,
            _ => continue,
        };
        let Some(payload) = record.event.payload() else {
            continue;
        };
        let source_day = payload.day.as_deref().unwrap_or(&partition_day);
        if scoped_day.is_some_and(|day| source_day != day) {
            continue;
        }
        let (Some(mode), Some(name)) = (payload.mode.clone(), payload.name.clone()) else {
            continue;
        };
        sequence += 1;
        let unit = TerminalUnit {
            day: source_day.to_owned(),
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
    for terminals in records.values_mut() {
        terminals.sort_by_key(|item| (item.ts, item.sequence));
    }
    records
}

pub(crate) fn fold_terminal_records(
    input: impl IntoIterator<Item = (String, RunLogRecord)>,
    scoped_day: Option<&str>,
) -> BTreeMap<TerminalUnit, TerminalState> {
    let records = group_observed_terminals(input, scoped_day);
    records
        .into_iter()
        .map(|(unit, terminals)| {
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
        .collect()
}

pub fn is_floor_talent_capped<S: HealthLogSource>(
    source: &S,
    day: &str,
    stream: Option<&str>,
    segment: &str,
    name: &str,
) -> Result<FoldRead<bool>, HealthError> {
    let capped = read_capped_segment_talents(source, day)?;
    let unit = TerminalUnit {
        day: day.to_owned(),
        mode: "segment".to_owned(),
        name: name.to_owned(),
        facet: None,
        stream: stream.map(str::to_owned),
        segment: Some(segment.to_owned()),
        activity: None,
    };
    Ok(FoldRead {
        value: capped.value.contains(&unit),
        malformed_line_count: capped.malformed_line_count,
    })
}

/// Every capped segment talent on one day, from one read of its logs. A caller
/// checking many segments reads this once rather than once per segment.
pub fn read_capped_segment_talents<S: HealthLogSource>(
    source: &S,
    day: &str,
) -> Result<FoldRead<BTreeSet<TerminalUnit>>, HealthError> {
    let failures = read_segment_talent_failures(source, day)?;
    Ok(FoldRead {
        value: failures.value.capped,
        malformed_line_count: failures.malformed_line_count,
    })
}

/// A segment talent that keeps failing on the same work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatedFailure {
    /// The reason its latest counted failure gave.
    pub reason_code: String,
    pub latest_ts: i64,
}

/// What one read of a day's logs says about segment talents that keep failing.
#[derive(Debug, Clone, Default)]
pub struct SegmentTalentFailures {
    /// The floor cap: any failure but a confidential-access or attestation
    /// outage counts.
    pub capped: BTreeSet<TerminalUnit>,
    /// Talents to set aside. Only a failure about the request itself counts,
    /// so an outage or a dropped connection never sets a recording's work aside.
    pub repeated: BTreeMap<TerminalUnit, RepeatedFailure>,
    /// When each segment whose latest attempt at a step it waits on failed may
    /// be tried again, by stream and segment key.
    pub segment_retry_at: BTreeMap<(Option<String>, String), i64>,
    /// When the day's daily work may be tried again, if every daily talent
    /// still failing failed for a reason that says nothing about its request.
    pub daily_retry_at: Option<i64>,
}

/// A failed attempt rests this long, doubling with each failure in a row...
const FIRST_REST_MS: i64 = 5 * 60 * 1000;
/// ...up to this. A failure can be billed, and nothing else bounds one that is
/// never counted, so this sets what it can cost. It also sets how late a step
/// that kept failing through an outage can start once the outage ends.
pub const LONGEST_REST_MS: i64 = 30 * 60 * 1000;

fn rest_after_failures(failures: usize) -> i64 {
    let doublings = failures.saturating_sub(1).min(8) as u32;
    FIRST_REST_MS
        .saturating_mul(1 << doublings)
        .min(LONGEST_REST_MS)
}

/// The unit's failures in a row, the latest one's time and its reason, if its
/// latest attempt failed.
fn latest_failures(terminals: &[ObservedTerminal]) -> Option<(usize, i64, Option<&str>)> {
    let latest = terminals.last()?;
    (latest.event == TerminalEvent::Fail).then(|| {
        let failures = terminals
            .iter()
            .rev()
            .take_while(|item| item.event == TerminalEvent::Fail)
            .count();
        (failures, latest.ts, latest.reason_code.as_deref())
    })
}

/// Request-specific failures beyond the deterministic set: the answer ran out
/// of room or could not be used.
const UNUSABLE_ANSWER_REASON_CODES: &[&str] = &[
    "context_budget_exceeded",
    "context_fitted_overflow",
    "context_preserved_overflow",
    "incomplete_json_length",
    "incomplete_text_length",
    "provider_response_invalid",
    "schema_validation_failed",
    "thinking_consumed_budget",
];

/// A failure the same request would meet again. Anything else, including an
/// unknown cause, may be the lane or the moment, and keeps its turn.
fn about_the_request(reason: &str) -> bool {
    (is_deterministic(reason) || UNUSABLE_ANSWER_REASON_CODES.contains(&reason))
        && !solstone_core_generate::is_blocking_reason(reason)
}

pub fn read_segment_talent_failures<S: HealthLogSource>(
    source: &S,
    day: &str,
) -> Result<FoldRead<SegmentTalentFailures>, HealthError> {
    let scanned = read_day_records(source, day)?;
    let grouped = group_observed_terminals(
        scanned
            .value
            .into_iter()
            .map(|record| (day.to_owned(), record)),
        None,
    );
    let mut failures = SegmentTalentFailures::default();
    let mut daily_about_the_request = false;
    for (unit, terminals) in grouped {
        let latest = latest_failures(&terminals);
        if unit.mode == "daily" {
            if let Some((count, ts, reason)) = latest {
                daily_about_the_request |= reason.is_some_and(about_the_request);
                let retry_at = ts.saturating_add(rest_after_failures(count));
                failures.daily_retry_at = failures.daily_retry_at.max(Some(retry_at));
            }
            continue;
        }
        if unit.mode != "segment" {
            continue;
        }
        if let (Some((count, ts, _)), Some(segment), false) = (
            latest,
            unit.segment.clone(),
            SEGMENT_NONGATING_TALENTS.contains(&unit.name.as_str()),
        ) {
            let retry_at = ts.saturating_add(rest_after_failures(count));
            let at = failures
                .segment_retry_at
                .entry((unit.stream.clone(), segment))
                .or_default();
            *at = retry_at.max(*at);
        }
        let repeated =
            trailing_failures_repeated(&terminals, |code| !code.is_some_and(about_the_request));
        if trailing_failures_repeated(&terminals, |code| {
            code.is_some_and(|code| {
                code == "confidential_access_ended"
                    || solstone_core_generate::is_attestation_family_reason(code)
            })
        })
        .is_some()
        {
            failures.capped.insert(unit.clone());
        }
        if let Some(repeated) = repeated {
            failures.repeated.insert(unit, repeated);
        }
    }
    if daily_about_the_request {
        failures.daily_retry_at = None;
    }
    Ok(FoldRead {
        value: failures,
        malformed_line_count: scanned.malformed_line_count,
    })
}

/// At least `CAP` trailing failures spanning `MIN_SPAN_MS`, not counting the
/// excluded reasons, which neither count nor end the run.
fn trailing_failures_repeated(
    terminals: &[ObservedTerminal],
    excluded: impl Fn(Option<&str>) -> bool,
) -> Option<RepeatedFailure> {
    let mut count = 0;
    let mut latest = None;
    let mut oldest_counted_ts = None;
    for terminal in terminals.iter().rev() {
        if terminal.event != TerminalEvent::Fail {
            break;
        }
        if excluded(terminal.reason_code.as_deref()) {
            continue;
        }
        count += 1;
        latest.get_or_insert(terminal);
        oldest_counted_ts = Some(terminal.ts);
    }
    let latest = latest?;
    (count >= CAP && oldest_counted_ts.is_some_and(|oldest| latest.ts - oldest >= MIN_SPAN_MS))
        .then(|| RepeatedFailure {
            reason_code: latest
                .reason_code
                .clone()
                .unwrap_or_else(|| "talent_failed".to_owned()),
            latest_ts: latest.ts,
        })
}

pub fn read_completed_units<S: HealthLogSource>(
    source: &S,
    day: &str,
) -> Result<FoldRead<BTreeSet<CompletedUnit>>, HealthError> {
    let states = read_terminal_states(source, day, true)?;
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
    // Fold both log partitions together before selecting completions: work may
    // finish after midnight, and a later failure must supersede its earlier success.
    let mut input = Vec::new();
    for scan_day in [previous.as_str(), day] {
        let scanned = read_day_records(source, scan_day)?;
        malformed_line_count += scanned.malformed_line_count;
        input.extend(
            scanned
                .value
                .into_iter()
                .map(|record| (scan_day.to_owned(), record)),
        );
    }
    for (unit, state) in fold_terminal_records(input, None) {
        let Some(ts) = state.last_real_complete_ts else {
            continue;
        };
        if state.latest_event != TerminalEvent::Complete || ts <= since_ms {
            continue;
        }
        if let Some(segment) = unit.segment.filter(|value| !value.is_empty()) {
            segments
                .entry((unit.day, unit.stream, segment))
                .and_modify(|current| *current = (*current).max(ts))
                .or_insert(ts);
        } else if let Some(activity) = unit.activity.filter(|value| !value.is_empty()) {
            activities
                .entry((unit.day, unit.facet, activity))
                .and_modify(|current| *current = (*current).max(ts))
                .or_insert(ts);
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
    let states = read_terminal_states(source, day, true)?;
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::FilesystemHealthLogSource;
    use crate::vocabulary::MIN_SPAN_MS;

    #[test]
    fn attestation_family_membership_and_predicates() {
        let codes = solstone_core_generate::contract()["reason_codes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["code"].as_str())
            .filter(|code| code.starts_with("attestation_"))
            .collect::<Vec<_>>();
        assert!(!codes.is_empty());
        assert!(codes.contains(&"attestation_failed"));
        assert!(codes.contains(&"attestation_not_yet_verified"));
        assert!(codes.contains(&"attestation_stale"));

        for code in &codes {
            assert!(solstone_core_generate::is_attestation_family_reason(code));
            assert!(!solstone_core_system::daily_coverage::daily_failure_capped(
                code, 4
            ));
            assert!(!is_deterministic(code));
        }
        assert!(!solstone_core_generate::is_attestation_family_reason(
            "attestation_unreachable"
        ));
    }

    fn write_run_log(root: &Path, day: &str, name: &str, content: &str) {
        let dir = root.join("chronicle").join(day).join("health");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn a_failed_attempt_rests_longer_each_time_up_to_a_limit() {
        let minutes = |failures| rest_after_failures(failures) / 60_000;
        assert_eq!([1, 2, 3, 4, 5, 500].map(minutes), [5, 10, 20, 30, 30, 30]);
    }

    fn failures_of(log: &str) -> SegmentTalentFailures {
        let temp = TempDir::new().unwrap();
        write_run_log(temp.path(), "20260810", "a.jsonl", log);
        read_segment_talent_failures(&FilesystemHealthLogSource::new(temp.path()), "20260810")
            .unwrap()
            .value
    }

    fn terminal(event: &str, mode: &str, name: &str, ts: i64, reason: &str) -> String {
        let segment = if mode == "segment" {
            r#""stream":"audio","segment":"120000_60","#
        } else {
            ""
        };
        format!(
            r#"{{"event":"{event}","ts":{ts},"mode":"{mode}",{segment}"name":"{name}","day":"20260810","reason_code":"{reason}"}}"#
        ) + "\n"
    }

    #[test]
    fn a_segment_rests_on_its_latest_failures_and_a_success_ends_them() {
        let key = (Some("audio".to_owned()), "120000_60".to_owned());
        let minute = 60_000;
        let fail = |ts| terminal("talent.fail", "segment", "sense", ts, "network_unreachable");
        let failed_twice = fail(0) + &fail(minute);
        assert_eq!(
            failures_of(&failed_twice).segment_retry_at[&key],
            minute + 10 * minute
        );
        let recovered =
            failed_twice + &terminal("talent.complete", "segment", "sense", 2 * minute, "");
        assert!(!failures_of(&recovered).segment_retry_at.contains_key(&key));
    }

    #[test]
    fn a_day_rests_short_only_when_no_failure_is_about_its_request() {
        let down = terminal(
            "talent.fail",
            "daily",
            "morning_briefing",
            7,
            "network_unreachable",
        );
        assert_eq!(failures_of(&down).daily_retry_at, Some(7 + FIRST_REST_MS));
        let own = terminal("talent.fail", "daily", "weekly", 8, "schema_invalid");
        assert_eq!(failures_of(&(down + &own)).daily_retry_at, None);
    }

    #[test]
    fn floor_talent_capping_excludes_attestation_family() {
        let day = "20260810";
        let stream = "audio";
        let segment = "120000_60";
        let name = "documents";

        // Five or more family-reason fails spanning at least MIN_SPAN_MS: not capped
        let temp = TempDir::new().unwrap();
        let log = (0..5)
            .map(|i| {
                format!(
                    r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"attestation_failed"}}"#,
                    1000 + i * MIN_SPAN_MS
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        write_run_log(temp.path(), day, "run.jsonl", &log);
        assert!(
            !is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );

        // Same timestamps and count with schema_invalid or attestation_unreachable: capped
        let temp = TempDir::new().unwrap();
        let log = (0..5)
            .map(|i| {
                format!(
                    r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"schema_invalid"}}"#,
                    1000 + i * MIN_SPAN_MS
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        write_run_log(temp.path(), day, "run.jsonl", &log);
        assert!(
            is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );

        let temp = TempDir::new().unwrap();
        let log = (0..5)
            .map(|i| {
                format!(
                    r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"attestation_unreachable"}}"#,
                    1000 + i * MIN_SPAN_MS
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        write_run_log(temp.path(), day, "run.jsonl", &log);
        assert!(
            is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );

        // Same shape with no reason_code: capped
        let temp = TempDir::new().unwrap();
        let log = (0..5)
            .map(|i| {
                format!(
                    r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents"}}"#,
                    1000 + i * MIN_SPAN_MS
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        write_run_log(temp.path(), day, "run.jsonl", &log);
        assert!(
            is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );

        // Five non-family fails inside ten minutes (< MIN_SPAN_MS), then family fails >= MIN_SPAN_MS after first: not capped
        let temp = TempDir::new().unwrap();
        let mut lines = (0..5)
            .map(|i| {
                format!(
                    r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"schema_invalid"}}"#,
                    1000 + i * 60_000 // inside 4 minutes
                )
            })
            .collect::<Vec<_>>();
        lines.push(format!(
            r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"attestation_failed"}}"#,
            1000 + MIN_SPAN_MS + 100_000
        ));
        write_run_log(temp.path(), day, "run.jsonl", &lines.join("\n"));
        assert!(
            !is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );

        // Five non-family fails spanning MIN_SPAN_MS, then family fails: family fails do not end the run, so capped
        let temp = TempDir::new().unwrap();
        let mut lines = (0..5)
            .map(|i| {
                format!(
                    r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"schema_invalid"}}"#,
                    1000 + i * MIN_SPAN_MS
                )
            })
            .collect::<Vec<_>>();
        lines.extend((0..3).map(|i| {
            format!(
                r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"attestation_failed"}}"#,
                1000 + 5 * MIN_SPAN_MS + i * 60_000
            )
        }));
        write_run_log(temp.path(), day, "run.jsonl", &lines.join("\n"));
        assert!(
            is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );

        // A talent.complete ends the run: fails before it do not count
        // Five family fails after completion are not capped
        let temp = TempDir::new().unwrap();
        let mut lines = vec![
            r#"{"ts":500,"event":"talent.complete","mode":"segment","stream":"audio","segment":"120000_60","name":"documents"}"#.to_owned(),
        ];
        lines.extend((0..5).map(|i| {
            format!(
                r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"attestation_failed"}}"#,
                1000 + i * MIN_SPAN_MS
            )
        }));
        write_run_log(temp.path(), day, "run.jsonl", &lines.join("\n"));
        assert!(
            !is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );

        // Five non-family fails after completion spanning >= MIN_SPAN_MS are capped
        let temp = TempDir::new().unwrap();
        let mut lines = vec![
            r#"{"ts":500,"event":"talent.complete","mode":"segment","stream":"audio","segment":"120000_60","name":"documents"}"#.to_owned(),
        ];
        lines.extend((0..5).map(|i| {
            format!(
                r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"schema_invalid"}}"#,
                1000 + i * MIN_SPAN_MS
            )
        }));
        write_run_log(temp.path(), day, "run.jsonl", &lines.join("\n"));
        assert!(
            is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );

        // Five non-family fails spanning >= MIN_SPAN_MS, then a talent.complete with later ts, then one non-family fail: not capped
        let temp = TempDir::new().unwrap();
        let mut lines = (0..5)
            .map(|i| {
                format!(
                    r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"schema_invalid"}}"#,
                    1000 + i * MIN_SPAN_MS
                )
            })
            .collect::<Vec<_>>();
        lines.push(format!(
            r#"{{"ts":{},"event":"talent.complete","mode":"segment","stream":"audio","segment":"120000_60","name":"documents"}}"#,
            1000 + 5 * MIN_SPAN_MS
        ));
        lines.push(format!(
            r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"schema_invalid"}}"#,
            1000 + 6 * MIN_SPAN_MS
        ));
        write_run_log(temp.path(), day, "run.jsonl", &lines.join("\n"));
        assert!(
            !is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );
    }

    #[test]
    fn floor_talent_capping_excludes_confidential_access_ended() {
        let day = "20260810";
        let stream = "audio";
        let segment = "120000_60";
        let name = "documents";

        let temp = TempDir::new().unwrap();
        let log = (0..5)
            .map(|i| {
                format!(
                    r#"{{"ts":{},"event":"talent.fail","mode":"segment","stream":"audio","segment":"120000_60","name":"documents","reason_code":"confidential_access_ended"}}"#,
                    1000 + i * MIN_SPAN_MS
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        write_run_log(temp.path(), day, "run.jsonl", &log);
        assert!(
            !is_floor_talent_capped(
                &FilesystemHealthLogSource::new(temp.path()),
                day,
                Some(stream),
                segment,
                name
            )
            .unwrap()
            .value
        );
    }
}
