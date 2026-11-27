// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span, Timestamp};
use serde_json::{Map, Value};

use super::config::{minute_interval, parse_weekly_day};
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
    let Some((hour, minute)) = daily_time.and_then(parse_daily_time) else {
        return now.date().at(0, 0, 0, 0);
    };
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
/// but `ok` or the no-input `empty`) is due again once, [`FAILED_RUN_RETRY_DELAY_SECONDS`] after that run
/// ended.
pub fn is_due(
    entry: &ScheduleEntry,
    state_entry: Option<&Value>,
    config: &ScheduleConfig,
    now: &ScheduleNow,
) -> bool {
    is_mark_due(entry, state_entry, config, now)
        || retry_at(entry, state_entry, config, now).is_some_and(|at| now.local >= at)
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
) -> Option<DateTime> {
    let mark = period_mark(entry, config, now.local)?;
    let state = state_entry?.as_object()?;
    if state
        .get("last_status")
        .and_then(Value::as_str)
        .is_none_or(|status| matches!(status, "ok" | "empty"))
    {
        return None;
    }
    let last = local_from_epoch(state.get("last_run")?.as_f64()?, &now.zone)?;
    if last < mark {
        return None;
    }
    let previous = state
        .get("previous_run")
        .and_then(Value::as_f64)
        .and_then(|value| local_from_epoch(value, &now.zone));
    if previous.is_some_and(|previous| previous >= mark) {
        return None;
    }
    last.checked_add(SignedDuration::from_secs(FAILED_RUN_RETRY_DELAY_SECONDS))
        .ok()
}

fn period_mark(entry: &ScheduleEntry, config: &ScheduleConfig, now: DateTime) -> Option<DateTime> {
    match entry.every.as_str() {
        "daily" => Some(daily_mark(now, config.daily_time.as_deref())),
        "weekly" => {
            let day = config
                .weekly_day
                .as_deref()
                .and_then(parse_weekly_day)
                .unwrap_or(6);
            Some(weekly_mark(now, day, config.weekly_time.as_deref()))
        }
        _ => None,
    }
}

pub(crate) fn period_key(
    entry: &ScheduleEntry,
    config: &ScheduleConfig,
    local: DateTime,
) -> Option<String> {
    match entry.every.as_str() {
        "daily" => Some(
            daily_mark(local, config.daily_time.as_deref())
                .date()
                .to_string(),
        ),
        "weekly" => {
            let day = config
                .weekly_day
                .as_deref()
                .and_then(parse_weekly_day)
                .unwrap_or(6);
            let week = weekly_mark(local, day, config.weekly_time.as_deref())
                .date()
                .iso_week_date();
            Some(format!("{:04}-W{:02}", week.year(), week.week()))
        }
        _ => None,
    }
}

fn daily_period_satisfied(state_entry: Option<&Value>, mark_date: jiff::civil::Date) -> bool {
    state_entry
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("last_period"))
        .and_then(Value::as_str)
        .and_then(|raw| raw.parse::<jiff::civil::Date>().ok())
        .is_some_and(|recorded| recorded >= mark_date)
}

fn weekly_period_satisfied(
    state_entry: Option<&Value>,
    mark_week: jiff::civil::ISOWeekDate,
) -> bool {
    state_entry
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("last_period"))
        .and_then(Value::as_str)
        .and_then(parse_iso_week_key)
        .is_some_and(|recorded| recorded >= (mark_week.year(), mark_week.week()))
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
    let Some(last_run) = state_entry
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("last_run"))
        .and_then(Value::as_f64)
    else {
        return true;
    };
    let Some(last) = local_from_epoch(last_run, &now.zone) else {
        return true;
    };
    match entry.every.as_str() {
        "hourly" => last < hour_mark(now.local),
        "daily" => {
            let mark = daily_mark(now.local, config.daily_time.as_deref());
            last < mark && !daily_period_satisfied(state_entry, mark.date())
        }
        "weekly" => {
            let day = config
                .weekly_day
                .as_deref()
                .and_then(parse_weekly_day)
                .unwrap_or(6);
            let mark = weekly_mark(now.local, day, config.weekly_time.as_deref());
            last < mark && !weekly_period_satisfied(state_entry, mark.date().iso_week_date())
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
        return local_to_epoch_millis(retry, &now.zone).unwrap_or(now.unix_millis);
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
        "daily" => {
            let mark = daily_mark(now.local, config.daily_time.as_deref());
            if due {
                mark
            } else {
                let next = mark + SignedDuration::from_hours(24);
                if daily_period_satisfied(state_entry, next.date()) {
                    next + SignedDuration::from_hours(24)
                } else {
                    next
                }
            }
        }
        "weekly" => {
            let day = config
                .weekly_day
                .as_deref()
                .and_then(parse_weekly_day)
                .unwrap_or(6);
            let mark = weekly_mark(now.local, day, config.weekly_time.as_deref());
            if due {
                mark
            } else {
                let next = mark + SignedDuration::from_hours(24 * 7);
                if weekly_period_satisfied(state_entry, next.date().iso_week_date()) {
                    next + SignedDuration::from_hours(24 * 7)
                } else {
                    next
                }
            }
        }
        every => match minute_interval(every) {
            Some(_) if due => now.local,
            Some(minutes) => state_entry
                .and_then(Value::as_object)
                .and_then(|entry| entry.get("last_run"))
                .and_then(Value::as_f64)
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
    config
        .daily_time
        .as_deref()
        .and_then(parse_daily_time)
        .unwrap_or((0, 0))
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
    now: DateTime,
) -> (DateTime, DateTime, DateTime) {
    let weekly_day = config
        .weekly_day
        .as_deref()
        .and_then(parse_weekly_day)
        .unwrap_or(6);
    (
        hour_mark(now),
        daily_mark(now, config.daily_time.as_deref()),
        weekly_mark(now, weekly_day, config.weekly_time.as_deref()),
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
    // A wall time the clocks skipped has no instant; a repeated one is its
    // first occurrence.
    solstone_core_time::from_local(zone, value)
        .earliest()
        .map(|value| value.timestamp().as_millisecond())
}

pub(crate) fn state_entry<'a>(state: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    state.get(name)
}
