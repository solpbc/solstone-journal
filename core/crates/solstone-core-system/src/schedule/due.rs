// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span, Timestamp};
use serde_json::{Map, Value};

use super::config::{DEFAULT_MAINTENANCE_TIME, minute_interval, parse_weekly_day};
use super::{ScheduleConfig, ScheduleEntry, ScheduleNow};

/// Return the effective minute interval, applying Python's five-minute floor on use.
pub fn effective_every(every: &str) -> String {
    minute_interval(every)
        .map(|minutes| format!("{}m", minutes.max(5)))
        .unwrap_or_else(|| every.to_owned())
}

/// Recompute the current hourly mark from local calendar fields.
pub fn hour_mark(now: DateTime) -> DateTime {
    now.date().at(now.hour(), 0, 0, 0)
}

/// Recompute the current daily mark from local calendar fields.
pub fn daily_mark(now: DateTime, daily_time: Option<&str>) -> DateTime {
    let (hour, minute) = daily_clock(daily_time);
    let today = now.date().at(hour, minute, 0, 0);
    if now >= today {
        today
    } else {
        today - SignedDuration::from_hours(24)
    }
}

/// Recompute the current weekly mark from local calendar fields.
pub fn weekly_mark(now: DateTime, weekly_day: u32, weekly_time: Option<&str>) -> DateTime {
    let (hour, minute) = weekly_time.and_then(parse_daily_time).unwrap_or((3, 0));
    let from_monday = u32::from(now.weekday().to_monday_zero_offset().unsigned_abs());
    let days_since = (from_monday + 7 - weekly_day) % 7;
    let date = now.date() - Span::new().days(i64::from(days_since));
    let mark = date.at(hour, minute, 0, 0);
    if now >= mark {
        mark
    } else {
        mark - SignedDuration::from_hours(24 * 7)
    }
}

/// How long after a failed daily or weekly run its one retry waits.
pub const FAILED_RUN_RETRY_DELAY_SECONDS: i64 = 60 * 60;

/// Marks are strict and elapsed minute windows are inclusive. A daily or weekly
/// entry whose first run in its current period recorded a failure (any status
/// other than `ok`, `success`, or the no-input `empty`) is due again once, [`FAILED_RUN_RETRY_DELAY_SECONDS`] after that run
/// ended.
pub fn is_due(
    entry: &ScheduleEntry,
    state_entry: Option<&Value>,
    config: &ScheduleConfig,
    now: &ScheduleNow,
) -> bool {
    is_mark_due(entry, state_entry, config, now)
        || retry_at(entry, state_entry, config, now).is_some_and(|at| now.unix_millis >= at)
}

/// When a failed daily or weekly run's single retry falls due, if it has one.
///
/// Only the first run of a period earns a retry: completion keeps the run
/// before it as `previous_run`, so a retry that fails again (its previous run
/// is inside the period) waits for the next mark. Hourly and minute entries
/// already run again within the hour, so they never retry.
pub(crate) fn retry_at(
    entry: &ScheduleEntry,
    state_entry: Option<&Value>,
    config: &ScheduleConfig,
    now: &ScheduleNow,
) -> Option<i64> {
    let mark = period_mark(entry, config, now)?;
    let mark_ms = local_to_epoch_millis(mark, &now.zone)?;
    let state = state_entry?.as_object()?;
    if state
        .get("last_status")
        .and_then(Value::as_str)
        .is_none_or(|status| matches!(status, "ok" | "empty" | "success"))
    {
        return None;
    }
    if started_after_mark(entry, state_entry, mark_ms) {
        return None;
    }
    let run = epoch_millis(state.get("last_run")?.as_f64()?)?;
    if run < mark_ms {
        return None;
    }
    let previous = state
        .get("previous_run")
        .and_then(Value::as_f64)
        .and_then(epoch_millis);
    let previous_in_period = match state.get("previous_period").and_then(Value::as_str) {
        Some(previous) => Some(previous) == state.get("last_period").and_then(Value::as_str),
        None => previous.is_some_and(|previous| previous >= mark_ms),
    };
    if previous_in_period {
        return None;
    }
    run.checked_add(FAILED_RUN_RETRY_DELAY_SECONDS * 1000)
}

fn epoch_millis(seconds: f64) -> Option<i64> {
    let millis = seconds * 1000.0;
    (millis.is_finite() && millis >= i64::MIN as f64 && millis <= i64::MAX as f64)
        .then_some(millis.floor() as i64)
}

// After the clocks turn back, a wall time can look earlier than a mark
// that already passed at its first occurrence.
fn at_latest_period(mark: DateTime, days: i64, now: &ScheduleNow) -> DateTime {
    let next = mark + Span::new().days(days);
    if local_to_epoch_millis(next, &now.zone).is_some_and(|at| at <= now.unix_millis) {
        next
    } else {
        mark
    }
}

fn period_mark(
    entry: &ScheduleEntry,
    config: &ScheduleConfig,
    now: &ScheduleNow,
) -> Option<DateTime> {
    match entry.every.as_str() {
        "daily" => Some(at_latest_period(
            daily_mark(now.local, config.daily_time.as_deref()),
            1,
            now,
        )),
        "weekly" => {
            let (day, _) = effective_weekly_spec(config);
            Some(at_latest_period(
                weekly_mark(now.local, day, config.weekly_time.as_deref()),
                7,
                now,
            ))
        }
        _ => None,
    }
}

pub(crate) fn period_key(
    entry: &ScheduleEntry,
    config: &ScheduleConfig,
    now: &ScheduleNow,
) -> Option<String> {
    let date = period_mark(entry, config, now)?.date();
    if entry.every == "weekly" {
        let week = date.iso_week_date();
        Some(format!("{:04}-W{:02}", week.year(), week.week()))
    } else {
        Some(date.to_string())
    }
}

fn recorded_period_satisfies(
    entry: &ScheduleEntry,
    state: Option<&Value>,
    mark: DateTime,
) -> Option<bool> {
    let recorded = state?.get("last_period")?.as_str()?;
    if entry.every == "weekly" {
        let current = mark.date().iso_week_date();
        Some(parse_iso_week_key(recorded)? >= (current.year(), current.week()))
    } else {
        Some(recorded.parse::<jiff::civil::Date>().ok()? >= mark.date())
    }
}

fn start_after_ms(entry: &ScheduleEntry, state: Option<&Value>) -> Option<i64> {
    let edited = state
        .and_then(|row| row.get("start_after"))
        .and_then(Value::as_f64)
        .and_then(epoch_millis);
    entry.registered_at_ms.max(edited)
}

fn started_after_mark(entry: &ScheduleEntry, state: Option<&Value>, mark_ms: i64) -> bool {
    start_after_ms(entry, state).is_some_and(|start| start >= mark_ms)
}

fn last_observed_seconds(entry: &ScheduleEntry, state: Option<&Value>) -> Option<f64> {
    let completed = state
        .and_then(|row| row.get("last_run"))
        .and_then(Value::as_f64);
    let started = start_after_ms(entry, state).map(|ms| ms as f64 / 1000.0);
    match (completed, started) {
        (Some(last), Some(start)) => Some(last.max(start)),
        _ => completed.or(started),
    }
}

fn parse_iso_week_key(raw: &str) -> Option<(i16, i8)> {
    let (year, week) = raw.split_once("-W")?;
    let year: i16 = year.parse().ok()?;
    let week: i8 = week.parse().ok()?;
    if !(1..=53).contains(&week) {
        return None;
    }
    Some((year, week))
}

fn is_mark_due(
    entry: &ScheduleEntry,
    state_entry: Option<&Value>,
    config: &ScheduleConfig,
    now: &ScheduleNow,
) -> bool {
    let Some(last_run) = last_observed_seconds(entry, state_entry) else {
        return true;
    };
    let Some(last) = local_from_epoch(last_run, &now.zone) else {
        return true;
    };
    match entry.every.as_str() {
        "hourly" => last < hour_mark(now.local),
        "daily" | "weekly" => {
            let mark = period_mark(entry, config, now).expect("calendar cadence");
            let Some(mark_ms) = local_to_epoch_millis(mark, &now.zone) else {
                return false;
            };
            !started_after_mark(entry, state_entry, mark_ms)
                && !recorded_period_satisfies(entry, state_entry, mark)
                    .unwrap_or(last_run * 1000.0 >= mark_ms as f64)
        }
        every => minute_interval(every).is_some_and(|minutes| {
            now.local
                .checked_sub(SignedDuration::from_mins(minutes.max(5) as i64))
                .is_ok_and(|threshold| last <= threshold)
        }),
    }
}

/// Compute the next due epoch milliseconds independently from the due predicate.
pub(crate) fn compute_next_run(
    entry: &ScheduleEntry,
    state_entry: Option<&Value>,
    config: &ScheduleConfig,
    now: &ScheduleNow,
) -> i64 {
    let due = is_mark_due(entry, state_entry, config, now);
    if !due && let Some(retry) = retry_at(entry, state_entry, config, now) {
        return retry;
    }
    let next = match entry.every.as_str() {
        "hourly" => {
            let mark = hour_mark(now.local);
            if due {
                mark
            } else {
                mark + SignedDuration::from_hours(1)
            }
        }
        "daily" | "weekly" => {
            let mark = period_mark(entry, config, now).expect("calendar cadence");
            let days = if entry.every == "daily" { 1 } else { 7 };
            if due {
                mark
            } else {
                let next = mark + Span::new().days(days);
                if recorded_period_satisfies(entry, state_entry, next).unwrap_or(false) {
                    next + Span::new().days(days)
                } else {
                    next
                }
            }
        }
        every => match minute_interval(every) {
            Some(_) if due => now.local,
            Some(minutes) => last_observed_seconds(entry, state_entry)
                .and_then(|last| local_from_epoch(last, &now.zone))
                .and_then(|last| {
                    last.checked_add(SignedDuration::from_mins(minutes.max(5) as i64))
                        .ok()
                })
                .unwrap_or(now.local),
            None => now.local,
        },
    };
    local_to_epoch_millis(next, &now.zone).unwrap_or(now.unix_millis)
}

pub(crate) fn effective_daily_spec(config: &ScheduleConfig) -> (i8, i8) {
    daily_clock(config.daily_time.as_deref())
}

fn daily_clock(time: Option<&str>) -> (i8, i8) {
    time.and_then(parse_daily_time).unwrap_or_else(|| {
        parse_daily_time(DEFAULT_MAINTENANCE_TIME).expect("valid default maintenance time")
    })
}

pub(crate) fn effective_weekly_spec(config: &ScheduleConfig) -> (u32, (i8, i8)) {
    let day = config
        .weekly_day
        .as_deref()
        .and_then(parse_weekly_day)
        .unwrap_or(6);
    let time = config
        .weekly_time
        .as_deref()
        .and_then(parse_daily_time)
        .unwrap_or((3, 0));
    (day, time)
}

pub(crate) fn current_marks(
    config: &ScheduleConfig,
    now: &ScheduleNow,
) -> (DateTime, DateTime, DateTime) {
    let (day, _) = effective_weekly_spec(config);
    (
        hour_mark(now.local),
        at_latest_period(daily_mark(now.local, config.daily_time.as_deref()), 1, now),
        at_latest_period(
            weekly_mark(now.local, day, config.weekly_time.as_deref()),
            7,
            now,
        ),
    )
}

pub(crate) fn parse_daily_time(raw: &str) -> Option<(i8, i8)> {
    let (hour, minute) = raw.split_once(':')?;
    if minute.contains(':') {
        return None;
    }
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((i8::try_from(hour).ok()?, i8::try_from(minute).ok()?))
}

pub(crate) fn now_from_epoch(seconds: f64, zone: &TimeZone) -> Option<ScheduleNow> {
    Some(ScheduleNow::in_zone(
        Timestamp::from_millisecond(epoch_millis(seconds)?).ok()?,
        zone.clone(),
    ))
}

pub(crate) fn local_from_epoch(value: f64, zone: &TimeZone) -> Option<DateTime> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return None;
    }
    let seconds = value.floor() as i64;
    let nanos = ((value - seconds as f64) * 1_000_000_000.0) as u32;
    let nanos = i32::try_from(nanos).ok()?;
    Timestamp::new(seconds, nanos)
        .ok()
        .map(|value| zone.to_datetime(value))
}

fn local_to_epoch_millis(value: DateTime, zone: &TimeZone) -> Option<i64> {
    let ambiguous = zone.to_ambiguous_zoned_datetime(value);
    use jiff::tz::AmbiguousOffset;
    let instant = match ambiguous.offset() {
        // The first representable instant after a skipped mark is the clock transition itself.
        AmbiguousOffset::Gap { after, .. } => zone
            .following(after.to_timestamp(value).ok()?)
            .next()?
            .timestamp(),
        _ => ambiguous.earlier().ok()?.timestamp(),
    };
    Some(instant.as_millisecond())
}

pub(crate) fn state_entry<'a>(state: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    state.get(name)
}
