// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{
    Datelike, Duration as ChronoDuration, Local, LocalResult, NaiveDateTime, TimeZone, Timelike,
};
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
pub fn hour_mark(now: NaiveDateTime) -> NaiveDateTime {
    now.date()
        .and_hms_opt(now.hour(), 0, 0)
        .expect("valid hour")
}

/// Recompute the current daily mark from local calendar fields.
pub fn daily_mark(now: NaiveDateTime, daily_time: Option<&str>) -> NaiveDateTime {
    let Some((hour, minute)) = daily_time.and_then(parse_daily_time) else {
        return now.date().and_hms_opt(0, 0, 0).expect("midnight");
    };
    let today = now
        .date()
        .and_hms_opt(hour, minute, 0)
        .expect("valid configured time");
    if now >= today {
        today
    } else {
        today - ChronoDuration::days(1)
    }
}

/// Recompute the current weekly mark from local calendar fields.
pub fn weekly_mark(
    now: NaiveDateTime,
    weekly_day: u32,
    weekly_time: Option<&str>,
) -> NaiveDateTime {
    let (hour, minute) = weekly_time.and_then(parse_daily_time).unwrap_or((3, 0));
    let days_since = (now.weekday().num_days_from_monday() + 7 - weekly_day) % 7;
    let date = now.date() - ChronoDuration::days(i64::from(days_since));
    let mark = date
        .and_hms_opt(hour, minute, 0)
        .expect("valid configured time");
    if now >= mark {
        mark
    } else {
        mark - ChronoDuration::weeks(1)
    }
}

/// Match Python `_is_due`: marks are strict, elapsed minute windows are inclusive.
pub fn is_due(
    entry: &ScheduleEntry,
    state_entry: Option<&Value>,
    config: &ScheduleConfig,
    now: ScheduleNow,
) -> bool {
    let Some(last_run) = state_entry
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("last_run"))
        .and_then(Value::as_f64)
    else {
        return true;
    };
    let Some(last) = local_from_epoch(last_run) else {
        return true;
    };
    match entry.every.as_str() {
        "hourly" => last < hour_mark(now.local),
        "daily" => last < daily_mark(now.local, config.daily_time.as_deref()),
        "weekly" => {
            let day = config
                .weekly_day
                .as_deref()
                .and_then(parse_weekly_day)
                .unwrap_or(6);
            last < weekly_mark(now.local, day, config.weekly_time.as_deref())
        }
        every => minute_interval(every).is_some_and(|minutes| {
            last <= now.local - ChronoDuration::minutes(minutes.max(5) as i64)
        }),
    }
}

/// Compute the next due epoch milliseconds independently from the due predicate.
pub(crate) fn compute_next_run(
    entry: &ScheduleEntry,
    state_entry: Option<&Value>,
    config: &ScheduleConfig,
    now: ScheduleNow,
) -> i64 {
    let due = is_due(entry, state_entry, config, now);
    let next = match entry.every.as_str() {
        "hourly" => {
            let mark = hour_mark(now.local);
            if due {
                mark
            } else {
                mark + ChronoDuration::hours(1)
            }
        }
        "daily" => {
            let mark = daily_mark(now.local, config.daily_time.as_deref());
            if due {
                mark
            } else {
                mark + ChronoDuration::days(1)
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
                mark + ChronoDuration::weeks(1)
            }
        }
        every => match minute_interval(every) {
            Some(_) if due => now.local,
            Some(minutes) => state_entry
                .and_then(Value::as_object)
                .and_then(|entry| entry.get("last_run"))
                .and_then(Value::as_f64)
                .and_then(local_from_epoch)
                .map(|last| last + ChronoDuration::minutes(minutes.max(5) as i64))
                .unwrap_or(now.local),
            None => now.local,
        },
    };
    local_to_epoch_millis(next).unwrap_or(now.unix_millis)
}

pub(crate) fn current_marks(
    config: &ScheduleConfig,
    now: NaiveDateTime,
) -> (NaiveDateTime, NaiveDateTime, NaiveDateTime) {
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

fn parse_daily_time(raw: &str) -> Option<(u32, u32)> {
    let (hour, minute) = raw.split_once(':')?;
    if minute.contains(':') {
        return None;
    }
    let hour = hour.parse().ok()?;
    let minute = minute.parse().ok()?;
    (hour <= 23 && minute <= 59).then_some((hour, minute))
}

fn local_from_epoch(value: f64) -> Option<NaiveDateTime> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return None;
    }
    let seconds = value.floor() as i64;
    let nanos = ((value - seconds as f64) * 1_000_000_000.0) as u32;
    Local
        .timestamp_opt(seconds, nanos)
        .single()
        .map(|value| value.naive_local())
}

fn local_to_epoch_millis(value: NaiveDateTime) -> Option<i64> {
    match Local.from_local_datetime(&value) {
        LocalResult::Single(value) => Some(value.timestamp_millis()),
        LocalResult::Ambiguous(first, _) => Some(first.timestamp_millis()),
        LocalResult::None => None,
    }
}

pub(crate) fn state_entry<'a>(state: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    state.get(name)
}
