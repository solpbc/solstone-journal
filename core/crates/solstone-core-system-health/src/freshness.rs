// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Summary freshness, age formatting, template constants, and verdict composition.

use chrono::{DateTime, Duration, Utc};
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

pub const BACKLOG_FRESHNESS_MAX_AGE_HOURS: i64 = 36;
pub const FUTURE_SKEW_TOLERANCE_MS: i64 = 300_000;

/// No usable backlog summary (none written yet, unreadable, or degraded). Nothing is
/// checking in this state, so the verdict says what is known: it is unclear.
pub const VERDICT_UNCLEAR_NOW: &str = "it's unclear whether your journal is caught up right now.";
pub const VERDICT_ALL_CAUGHT_UP: &str = "your journal's all caught up.";
pub const VERDICT_CAUGHT_UP: &str = "your journal's caught up.";
pub const VERDICT_STUCK_ONLY_SINGULAR: &str = "caught up except 1 day that needs a hand.";
pub const VERDICT_STUCK_ONLY_PLURAL: &str = "caught up except {n} days that need a hand.";
pub const VERDICT_PENDING_ONLY_SINGULAR: &str = "1 day is still catching up.";
pub const VERDICT_PENDING_ONLY_PLURAL: &str = "{n} days are still catching up.";
pub const VERDICT_MIXED_STUCK_SINGULAR: &str = "1 day needs a hand";
pub const VERDICT_MIXED_STUCK_PLURAL: &str = "{n} days need a hand";
pub const VERDICT_MIXED_PENDING_SINGULAR: &str = "1 more day is still catching up";
pub const VERDICT_MIXED_PENDING_PLURAL: &str = "{n} more days are still catching up";
pub const VERDICT_AGE_UNKNOWN: &str =
    "it's unclear whether your journal is caught up; the last update age is unknown.";
pub const VERDICT_AGE_STALE_TEMPLATE: &str =
    "it's unclear whether your journal is caught up; the last update was {age} ago.";

pub const UNFINISHED_TEMPLATE_ONE: &str = "an activity from {day} couldn't finish processing";
pub const UNFINISHED_TEMPLATE_MANY_ONE_DAY: &str =
    "{n} activities from {day} couldn't finish processing";
pub const UNFINISHED_TEMPLATE_MANY_DAYS: &str =
    "{n} activities couldn't finish processing. oldest: {day}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryFreshness {
    Fresh,
    Stale,
    Unknown,
}

impl SummaryFreshness {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
        }
    }
}

impl Serialize for SummaryFreshness {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

pub fn parse_summary_time(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .ok()
        .map(|time| time.with_timezone(&Utc))
        .or_else(|| {
            trimmed
                .parse::<chrono::NaiveDateTime>()
                .ok()
                .map(|time| DateTime::from_naive_utc_and_offset(time, Utc))
        })
}

pub fn summary_freshness(generated_at: Option<&str>, now: DateTime<Utc>) -> SummaryFreshness {
    let Some(raw) = generated_at else {
        return SummaryFreshness::Unknown;
    };
    let Some(generated) = parse_summary_time(raw) else {
        return SummaryFreshness::Unknown;
    };
    if generated - now > Duration::milliseconds(FUTURE_SKEW_TOLERANCE_MS) {
        return SummaryFreshness::Unknown;
    }
    if now - generated > Duration::hours(BACKLOG_FRESHNESS_MAX_AGE_HOURS) {
        return SummaryFreshness::Stale;
    }
    SummaryFreshness::Fresh
}

pub fn format_summary_age(delta: Duration) -> String {
    let seconds = delta.num_seconds().max(0);
    let minutes = (seconds / 60).max(1);
    let hours = seconds / 3600;
    let days = seconds / 86400;
    if hours < 1 {
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    } else if hours < 48 {
        format!("{hours} hour{}", if hours == 1 { "" } else { "s" })
    } else {
        format!("{days} day{}", if days == 1 { "" } else { "s" })
    }
}

pub fn select_unfinished_template(activities: usize, day_count: usize) -> Option<&'static str> {
    if activities == 0 {
        None
    } else if activities == 1 {
        Some(UNFINISHED_TEMPLATE_ONE)
    } else if day_count <= 1 {
        Some(UNFINISHED_TEMPLATE_MANY_ONE_DAY)
    } else {
        Some(UNFINISHED_TEMPLATE_MANY_DAYS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnfinishedActivitiesAggregate {
    pub activities: usize,
    pub day_count: usize,
    pub oldest_day: Option<String>,
}

pub fn aggregate_unfinished_from_days(
    days: &[crate::types::BacklogDay],
) -> UnfinishedActivitiesAggregate {
    let mut total_activities = 0;
    let mut day_count = 0;
    let mut oldest_day: Option<String> = None;

    for day in days {
        if let Some(unfinished) = &day.unfinished_activities {
            let count = unfinished.activities;
            if count > 0 {
                total_activities += count;
                day_count += 1;
                match &oldest_day {
                    Some(current) if current.as_str() <= day.day.as_str() => {}
                    _ => oldest_day = Some(day.day.clone()),
                }
            }
        }
    }

    UnfinishedActivitiesAggregate {
        activities: total_activities,
        day_count,
        oldest_day,
    }
}

pub fn aggregate_unfinished_activities(
    backlog: &Map<String, Value>,
) -> UnfinishedActivitiesAggregate {
    let mut total_activities = 0;
    let mut day_count = 0;
    let mut oldest_day: Option<String> = None;

    if let Some(days) = backlog.get("days").and_then(Value::as_array) {
        for day in days {
            if let Some(unfinished) = day.get("unfinished_activities").and_then(Value::as_object) {
                let count = unfinished
                    .get("activities")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                if count > 0 {
                    total_activities += count;
                    day_count += 1;
                    if let Some(day_str) = day.get("day").and_then(Value::as_str) {
                        match &oldest_day {
                            Some(current) if current.as_str() <= day_str => {}
                            _ => oldest_day = Some(day_str.to_owned()),
                        }
                    }
                }
            }
        }
    }

    UnfinishedActivitiesAggregate {
        activities: total_activities,
        day_count,
        oldest_day,
    }
}

fn parse_count(value: Option<&Value>) -> usize {
    let Some(value) = value else {
        return 0;
    };
    match value {
        Value::Bool(b) => {
            if *b {
                1
            } else {
                0
            }
        }
        Value::Number(n) => n.as_u64().unwrap_or(0) as usize,
        Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BacklogStatusEvaluation {
    pub verdict: String,
    pub freshness: SummaryFreshness,
    pub pending_days: usize,
    pub oldest_pending_day: Option<String>,
    pub unfinished_activities: UnfinishedActivitiesAggregate,
}

pub fn evaluate_backlog_status(
    backlog: Option<&Map<String, Value>>,
    generated_at: Option<&str>,
    now: DateTime<Utc>,
) -> BacklogStatusEvaluation {
    let Some(backlog) = backlog else {
        return BacklogStatusEvaluation {
            verdict: VERDICT_UNCLEAR_NOW.to_owned(),
            freshness: SummaryFreshness::Unknown,
            pending_days: 0,
            oldest_pending_day: None,
            unfinished_activities: UnfinishedActivitiesAggregate {
                activities: 0,
                day_count: 0,
                oldest_day: None,
            },
        };
    };

    let freshness = summary_freshness(generated_at, now);

    if backlog.get("degraded") == Some(&Value::Bool(true)) {
        return BacklogStatusEvaluation {
            verdict: VERDICT_UNCLEAR_NOW.to_owned(),
            freshness,
            pending_days: 0,
            oldest_pending_day: None,
            unfinished_activities: UnfinishedActivitiesAggregate {
                activities: 0,
                day_count: 0,
                oldest_day: None,
            },
        };
    }
    match freshness {
        SummaryFreshness::Unknown => BacklogStatusEvaluation {
            verdict: VERDICT_AGE_UNKNOWN.to_owned(),
            freshness,
            pending_days: 0,
            oldest_pending_day: None,
            unfinished_activities: UnfinishedActivitiesAggregate {
                activities: 0,
                day_count: 0,
                oldest_day: None,
            },
        },
        SummaryFreshness::Stale => {
            let age_str = parse_summary_time(generated_at.unwrap_or(""))
                .map(|t| format_summary_age(now - t))
                .unwrap_or_else(|| "unknown".to_owned());
            let verdict = format!(
                "it's unclear whether your journal is caught up; the last update was {age_str} ago."
            );
            BacklogStatusEvaluation {
                verdict,
                freshness,
                pending_days: 0,
                oldest_pending_day: None,
                unfinished_activities: UnfinishedActivitiesAggregate {
                    activities: 0,
                    day_count: 0,
                    oldest_day: None,
                },
            }
        }
        SummaryFreshness::Fresh => {
            let unfinished = aggregate_unfinished_activities(backlog);
            let pending = parse_count(backlog.get("pending_days"));
            let stuck = parse_count(backlog.get("stuck_days"));
            let oldest_pending_day = backlog
                .get("oldest_pending_day")
                .and_then(Value::as_str)
                .map(|s| s.to_owned());

            let verdict = if pending == 0 && stuck == 0 {
                if unfinished.activities > 0 {
                    VERDICT_CAUGHT_UP.to_owned()
                } else {
                    VERDICT_ALL_CAUGHT_UP.to_owned()
                }
            } else if stuck > 0 && pending == 0 {
                if stuck == 1 {
                    VERDICT_STUCK_ONLY_SINGULAR.to_owned()
                } else {
                    format!("caught up except {stuck} days that need a hand.")
                }
            } else if stuck == 0 {
                if pending == 1 {
                    VERDICT_PENDING_ONLY_SINGULAR.to_owned()
                } else {
                    format!("{pending} days are still catching up.")
                }
            } else {
                let stuck_str = if stuck == 1 {
                    VERDICT_MIXED_STUCK_SINGULAR.to_owned()
                } else {
                    format!("{stuck} days need a hand")
                };
                let pending_str = if pending == 1 {
                    VERDICT_MIXED_PENDING_SINGULAR.to_owned()
                } else {
                    format!("{pending} more days are still catching up")
                };
                format!("{stuck_str}. {pending_str}.")
            };

            BacklogStatusEvaluation {
                verdict,
                freshness,
                pending_days: pending,
                oldest_pending_day,
                unfinished_activities: unfinished,
            }
        }
    }
}
