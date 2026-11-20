// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Summary freshness, age formatting, template constants, and verdict composition.

use jiff::tz::Offset;
use jiff::{SignedDuration, Timestamp};
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};
use solstone_core_window_catalog::{ENGLISH, strings, text};

pub const BACKLOG_FRESHNESS_MAX_AGE_HOURS: i64 = 36;
pub const FUTURE_SKEW_TOLERANCE_MS: i64 = 300_000;

/// No usable backlog summary (none written yet, unreadable, or degraded). Nothing is
/// checking in this state, so the verdict says what is known: it is unclear.
const VERDICT_UNCLEAR_NOW_ID: &str = "journal.window.system.health.backlog.verdict.unclear.now";
const VERDICT_ALL_CAUGHT_UP_ID: &str = "journal.window.system.health.backlog.verdict.all.caught.up";
const VERDICT_CAUGHT_UP_ID: &str = "journal.window.system.health.backlog.verdict.caught.up";
const VERDICT_STUCK_ONLY_SINGULAR_ID: &str =
    "journal.window.system.health.backlog.verdict.stuck.only.singular";
const VERDICT_STUCK_ONLY_PLURAL_ID: &str =
    "journal.window.system.health.backlog.verdict.stuck.only.plural";
const VERDICT_PENDING_ONLY_SINGULAR_ID: &str =
    "journal.window.system.health.backlog.verdict.pending.only.singular";
const VERDICT_PENDING_ONLY_PLURAL_ID: &str =
    "journal.window.system.health.backlog.verdict.pending.only.plural";
const VERDICT_MIXED_STUCK_SINGULAR_ID: &str =
    "journal.window.system.health.backlog.verdict.mixed.stuck.singular";
const VERDICT_MIXED_STUCK_PLURAL_ID: &str =
    "journal.window.system.health.backlog.verdict.mixed.stuck.plural";
const VERDICT_MIXED_PENDING_SINGULAR_ID: &str =
    "journal.window.system.health.backlog.verdict.mixed.pending.singular";
const VERDICT_MIXED_PENDING_PLURAL_ID: &str =
    "journal.window.system.health.backlog.verdict.mixed.pending.plural";
const VERDICT_AGE_UNKNOWN_ID: &str = "journal.window.system.health.backlog.verdict.age.unknown";
const VERDICT_AGE_STALE_TEMPLATE_ID: &str =
    "journal.window.system.health.backlog.verdict.age.stale.template";

pub const UNFINISHED_TEMPLATE_ONE_ID: &str = "journal.window.system.health.unfinished.template.one";
pub const UNFINISHED_TEMPLATE_MANY_ONE_DAY_ID: &str =
    "journal.window.system.health.unfinished.template.many.one.day";
pub const UNFINISHED_TEMPLATE_MANY_DAYS_ID: &str =
    "journal.window.system.health.unfinished.template.many.days";

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

pub fn parse_summary_time(value: &str) -> Option<Timestamp> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    solstone_core_time::parse_rfc3339(trimmed)
        .ok()
        .map(|time| time.timestamp())
        .or_else(|| {
            solstone_core_time::parse_civil_datetime(trimmed)
                .ok()
                .and_then(|time| Offset::UTC.to_timestamp(time).ok())
        })
}

pub fn summary_freshness(generated_at: Option<&str>, now: Timestamp) -> SummaryFreshness {
    let Some(raw) = generated_at else {
        return SummaryFreshness::Unknown;
    };
    let Some(generated) = parse_summary_time(raw) else {
        return SummaryFreshness::Unknown;
    };
    if generated - now > SignedDuration::from_millis(FUTURE_SKEW_TOLERANCE_MS) {
        return SummaryFreshness::Unknown;
    }
    if now - generated > SignedDuration::from_hours(BACKLOG_FRESHNESS_MAX_AGE_HOURS) {
        return SummaryFreshness::Stale;
    }
    SummaryFreshness::Fresh
}

pub fn format_summary_age(delta: SignedDuration) -> String {
    format_summary_age_in(delta, ENGLISH)
}

fn format_summary_age_in(delta: SignedDuration, locale: &str) -> String {
    let seconds = delta.as_secs().max(0);
    let minutes = (seconds / 60).max(1);
    let hours = seconds / 3600;
    let days = seconds / 86400;
    if hours < 1 {
        if minutes == 1 {
            text(
                locale,
                "journal.window.system.health.backlog.age.minute.one",
            )
        } else {
            strings(
                locale,
                "journal.window.system.health.backlog.age.minute.many",
                [("n", &minutes.to_string())],
            )
        }
    } else if hours < 48 {
        if hours == 1 {
            text(locale, "journal.window.system.health.backlog.age.hour.one")
        } else {
            strings(
                locale,
                "journal.window.system.health.backlog.age.hour.many",
                [("n", &hours.to_string())],
            )
        }
    } else {
        if days == 1 {
            text(locale, "journal.window.system.health.backlog.age.day.one")
        } else {
            strings(
                locale,
                "journal.window.system.health.backlog.age.day.many",
                [("n", &days.to_string())],
            )
        }
    }
}

pub fn select_unfinished_template(activities: usize, day_count: usize) -> Option<String> {
    if activities == 0 {
        None
    } else if activities == 1 {
        Some(text(ENGLISH, UNFINISHED_TEMPLATE_ONE_ID))
    } else if day_count <= 1 {
        Some(text(ENGLISH, UNFINISHED_TEMPLATE_MANY_ONE_DAY_ID))
    } else {
        Some(text(ENGLISH, UNFINISHED_TEMPLATE_MANY_DAYS_ID))
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

pub fn backlog_unavailable() -> String {
    text(ENGLISH, VERDICT_UNCLEAR_NOW_ID)
}

pub fn evaluate_backlog_status(
    backlog: Option<&Map<String, Value>>,
    generated_at: Option<&str>,
    now: Timestamp,
) -> BacklogStatusEvaluation {
    evaluate_backlog_status_in(backlog, generated_at, now, ENGLISH)
}

pub fn evaluate_backlog_status_in(
    backlog: Option<&Map<String, Value>>,
    generated_at: Option<&str>,
    now: Timestamp,
    locale: &str,
) -> BacklogStatusEvaluation {
    let Some(backlog) = backlog else {
        return BacklogStatusEvaluation {
            verdict: text(locale, VERDICT_UNCLEAR_NOW_ID),
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
            verdict: text(locale, VERDICT_UNCLEAR_NOW_ID),
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
            verdict: text(locale, VERDICT_AGE_UNKNOWN_ID),
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
                .map(|t| format_summary_age_in(now - t, locale))
                .unwrap_or_else(|| {
                    text(locale, "journal.window.system.health.backlog.age.unknown")
                });
            let verdict = strings(locale, VERDICT_AGE_STALE_TEMPLATE_ID, [("age", &age_str)]);
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
                    text(locale, VERDICT_CAUGHT_UP_ID)
                } else {
                    text(locale, VERDICT_ALL_CAUGHT_UP_ID)
                }
            } else if stuck > 0 && pending == 0 {
                if stuck == 1 {
                    text(locale, VERDICT_STUCK_ONLY_SINGULAR_ID)
                } else {
                    strings(
                        locale,
                        VERDICT_STUCK_ONLY_PLURAL_ID,
                        [("n", &stuck.to_string())],
                    )
                }
            } else if stuck == 0 {
                if pending == 1 {
                    text(locale, VERDICT_PENDING_ONLY_SINGULAR_ID)
                } else {
                    strings(
                        locale,
                        VERDICT_PENDING_ONLY_PLURAL_ID,
                        [("n", &pending.to_string())],
                    )
                }
            } else {
                let stuck_str = if stuck == 1 {
                    text(locale, VERDICT_MIXED_STUCK_SINGULAR_ID)
                } else {
                    strings(
                        locale,
                        VERDICT_MIXED_STUCK_PLURAL_ID,
                        [("n", &stuck.to_string())],
                    )
                };
                let pending_str = if pending == 1 {
                    text(locale, VERDICT_MIXED_PENDING_SINGULAR_ID)
                } else {
                    strings(
                        locale,
                        VERDICT_MIXED_PENDING_PLURAL_ID,
                        [("n", &pending.to_string())],
                    )
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
