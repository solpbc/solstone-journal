// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{
    Datelike, Duration as ChronoDuration, LocalResult, NaiveDateTime, TimeZone, Timelike,
};
use chrono_tz::Tz;
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
    now: ScheduleNow,
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
    now: ScheduleNow,
) -> Option<NaiveDateTime> {
    let mark = period_mark(entry, config, now.local)?;
    let state = state_entry?.as_object()?;
    if state
        .get("last_status")
        .and_then(Value::as_str)
        .is_none_or(|status| matches!(status, "ok" | "empty"))
    {
        return None;
    }
    let last = local_from_epoch(state.get("last_run")?.as_f64()?, now.zone)?;
    if last < mark {
        return None;
    }
    let previous = state
        .get("previous_run")
        .and_then(Value::as_f64)
        .and_then(|value| local_from_epoch(value, now.zone));
    if previous.is_some_and(|previous| previous >= mark) {
        return None;
    }
    last.checked_add_signed(ChronoDuration::seconds(FAILED_RUN_RETRY_DELAY_SECONDS))
}

fn period_mark(
    entry: &ScheduleEntry,
    config: &ScheduleConfig,
    now: NaiveDateTime,
) -> Option<NaiveDateTime> {
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

fn is_mark_due(
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
    let Some(last) = local_from_epoch(last_run, now.zone) else {
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
            now.local
                .checked_sub_signed(ChronoDuration::minutes(minutes.max(5) as i64))
                .is_some_and(|threshold| last <= threshold)
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
    let due = is_mark_due(entry, state_entry, config, now);
    if !due && let Some(retry) = retry_at(entry, state_entry, config, now) {
        return local_to_epoch_millis(retry, now.zone).unwrap_or(now.unix_millis);
    }
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
                .and_then(|last| local_from_epoch(last, now.zone))
                .and_then(|last| {
                    last.checked_add_signed(ChronoDuration::minutes(minutes.max(5) as i64))
                })
                .unwrap_or(now.local),
            None => now.local,
        },
    };
    local_to_epoch_millis(next, now.zone).unwrap_or(now.unix_millis)
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

pub(crate) fn parse_daily_time(raw: &str) -> Option<(u32, u32)> {
    let (hour, minute) = raw.split_once(':')?;
    if minute.contains(':') {
        return None;
    }
    let hour = hour.parse().ok()?;
    let minute = minute.parse().ok()?;
    (hour <= 23 && minute <= 59).then_some((hour, minute))
}

pub(crate) fn local_from_epoch(value: f64, zone: Tz) -> Option<NaiveDateTime> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return None;
    }
    let seconds = value.floor() as i64;
    let nanos = ((value - seconds as f64) * 1_000_000_000.0) as u32;
    zone.timestamp_opt(seconds, nanos)
        .single()
        .map(|value| value.naive_local())
}

fn local_to_epoch_millis(value: NaiveDateTime, zone: Tz) -> Option<i64> {
    match zone.from_local_datetime(&value) {
        LocalResult::Single(value) => Some(value.timestamp_millis()),
        LocalResult::Ambiguous(first, _) => Some(first.timestamp_millis()),
        LocalResult::None => None,
    }
}

pub(crate) fn state_entry<'a>(state: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    state.get(name)
}
