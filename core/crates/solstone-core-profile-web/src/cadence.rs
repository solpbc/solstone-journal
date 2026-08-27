// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Interaction cadence and active-profile scans.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde_json::{Map, Value};
use solstone_core_facets::{
    activity_value_or_empty, list_declared_facet_names, load_activity_records,
};

use crate::error::{ProfileError, ProfileResult};
use crate::types::{ActivitySourceRef, Cadence};

const CADENCE_WINDOW_DAYS: i64 = 90;
const RECENT_WINDOW_DAYS: i64 = 30;

pub(crate) fn compute_cadence(
    journal_root: &Path,
    entity_id: &str,
    include_mentions: bool,
    now: DateTime<Utc>,
) -> ProfileResult<(Cadence, Vec<ActivitySourceRef>)> {
    let roles = if include_mentions {
        ["attendee", "mentioned"].as_slice()
    } else {
        ["attendee"].as_slice()
    };
    let mut interaction_days = Vec::new();
    let mut sources = Vec::new();

    for (facet, day) in activity_window(journal_root, now, CADENCE_WINDOW_DAYS)? {
        let records = load_activity_records(journal_root, &facet, &day, false)
            .map_err(ProfileError::internal)?;
        for record in records {
            let record_id = activity_value_or_empty(record.get("id")).trim().to_owned();
            if record_id.is_empty() || !matches_participation(&record, entity_id, roles) {
                continue;
            }
            interaction_days.push(day.clone());
            sources.push(ActivitySourceRef {
                facet: facet.clone(),
                day: day.clone(),
                activity_id: record_id,
                field: "participation".to_owned(),
                created_at: record_created_at(&record),
            });
        }
    }

    if interaction_days.is_empty() {
        return Ok((
            Cadence {
                recent_interactions_count_30d: 0,
                last_seen: None,
                avg_interval_days: None,
                gone_quiet_since: None,
            },
            Vec::new(),
        ));
    }

    let distinct_days = interaction_days
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let last_seen = distinct_days
        .last()
        .expect("interaction days are non-empty")
        .clone();
    let recent_since = day_minus(now, RECENT_WINDOW_DAYS);
    let recent_interactions_count_30d = interaction_days
        .iter()
        .filter(|day| day.as_str() >= recent_since.as_str())
        .count() as i64;

    let (avg_interval_days, gone_quiet_since) = if distinct_days.len() >= 2 {
        let first = day_ordinal(
            distinct_days
                .first()
                .expect("at least two distinct interaction days"),
        );
        let last = day_ordinal(&last_seen);
        let average = f64::from(last - first) / (distinct_days.len() - 1) as f64;
        let quiet_gap = day_ordinal(&today_day(now)) - last;
        let gone_quiet = (f64::from(quiet_gap) > average * 2.0).then_some(i64::from(quiet_gap));
        (Some(average), gone_quiet)
    } else {
        (None, None)
    };

    sources.sort_by(|left, right| {
        (&left.day, &left.facet, &left.activity_id).cmp(&(
            &right.day,
            &right.facet,
            &right.activity_id,
        ))
    });
    Ok((
        Cadence {
            recent_interactions_count_30d,
            last_seen: Some(last_seen),
            avg_interval_days,
            gone_quiet_since,
        },
        sources,
    ))
}

pub(crate) fn list_active_entity_ids(
    journal_root: &Path,
    window_days: i64,
    now: DateTime<Utc>,
) -> ProfileResult<Vec<String>> {
    let mut entity_ids = BTreeSet::new();
    for (facet, day) in activity_window(journal_root, now, window_days)? {
        let records = load_activity_records(journal_root, &facet, &day, false)
            .map_err(ProfileError::internal)?;
        for record in records {
            let Some(participation) = record.get("participation").and_then(Value::as_array) else {
                continue;
            };
            for entry in participation {
                let Some(entry) = entry.as_object() else {
                    continue;
                };
                if entry.get("role").and_then(Value::as_str) != Some("attendee") {
                    continue;
                }
                if let Some(entity_id) = entry
                    .get("entity_id")
                    .and_then(Value::as_str)
                    .filter(|entity_id| !entity_id.is_empty())
                {
                    entity_ids.insert(entity_id.to_owned());
                }
            }
        }
    }
    Ok(entity_ids.into_iter().collect())
}

fn activity_window(
    journal_root: &Path,
    now: DateTime<Utc>,
    window_days: i64,
) -> ProfileResult<Vec<(String, String)>> {
    if window_days <= 0 {
        return Ok(Vec::new());
    }
    let facets = list_declared_facet_names(journal_root).map_err(ProfileError::internal)?;
    let today = now.date_naive();
    let mut window = Vec::with_capacity(
        usize::try_from(window_days)
            .ok()
            .and_then(|days| days.checked_mul(facets.len()))
            .unwrap_or(0),
    );
    for offset in (0..window_days).rev() {
        let day = (today - Duration::days(offset))
            .format("%Y%m%d")
            .to_string();
        for facet in &facets {
            window.push((facet.clone(), day.clone()));
        }
    }
    Ok(window)
}

fn matches_participation(record: &Map<String, Value>, entity_id: &str, roles: &[&str]) -> bool {
    record
        .get("participation")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                let Some(entry) = entry.as_object() else {
                    return false;
                };
                entry.get("entity_id").and_then(Value::as_str) == Some(entity_id)
                    && entry
                        .get("role")
                        .and_then(Value::as_str)
                        .is_some_and(|role| roles.contains(&role))
            })
        })
}

fn record_created_at(record: &Map<String, Value>) -> i64 {
    let Some(value) = record.get("created_at") else {
        return 0;
    };
    if value.is_null() || value == &Value::Bool(false) {
        return 0;
    }
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_f64().map(|value| value as i64))
        .or_else(|| {
            value
                .as_str()
                .and_then(|value| value.trim().parse::<i64>().ok())
        })
        .unwrap_or(0)
}

fn today_day(now: DateTime<Utc>) -> String {
    now.format("%Y%m%d").to_string()
}

fn day_minus(now: DateTime<Utc>, days: i64) -> String {
    (now.date_naive() - Duration::days(days))
        .format("%Y%m%d")
        .to_string()
}

fn day_ordinal(day: &str) -> i32 {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .expect("activity window and activity filenames use valid calendar days")
        .num_days_from_ce()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::{compute_cadence, day_minus, list_active_entity_ids, today_day};
    use crate::test_support::{journal, write_json, write_jsonl};

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0).unwrap()
    }

    fn facet(root: &std::path::Path, name: &str, muted: bool) {
        write_json(
            root,
            &format!("facets/{name}/facet.json"),
            json!({"name":name,"muted":muted}),
        );
    }

    fn records(root: &std::path::Path, facet: &str, day: &str, rows: &[serde_json::Value]) {
        write_jsonl(
            root,
            &format!("facets/{facet}/activities/{day}.jsonl"),
            rows,
        );
    }

    fn interaction(id: &str, created_at: i64, entity_id: &str, role: &str) -> serde_json::Value {
        json!({"id":id,"created_at":created_at,"participation":[{"entity_id":entity_id,"role":role}]})
    }

    #[test]
    fn utc_day_math() {
        let boundary = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(today_day(boundary), "20260101");
        assert_eq!(day_minus(boundary, 1), "20251231");
    }

    #[test]
    fn cadence_zero_single_and_multi_day_cases() {
        let temporary = journal();
        facet(temporary.path(), "work", false);
        let (zero, zero_sources) =
            compute_cadence(temporary.path(), "pat", false, now()).expect("zero cadence");
        assert_eq!(zero.recent_interactions_count_30d, 0);
        assert_eq!(zero.last_seen, None);
        assert!(zero_sources.is_empty());

        records(
            temporary.path(),
            "work",
            "20260408",
            &[interaction("single", 1, "pat", "attendee")],
        );
        let (single, _) =
            compute_cadence(temporary.path(), "pat", false, now()).expect("single cadence");
        assert_eq!(single.last_seen.as_deref(), Some("20260408"));
        assert_eq!(single.avg_interval_days, None);
        assert_eq!(single.gone_quiet_since, None);

        records(
            temporary.path(),
            "work",
            "20260331",
            &[interaction("first", 2, "pat", "attendee")],
        );
        records(
            temporary.path(),
            "work",
            "20260405",
            &[interaction("middle", 3, "pat", "attendee")],
        );
        records(
            temporary.path(),
            "work",
            "20260410",
            &[interaction("last", 4, "pat", "attendee")],
        );
        let (multiple, sources) =
            compute_cadence(temporary.path(), "pat", false, now()).expect("multi cadence");
        assert_eq!(multiple.recent_interactions_count_30d, 4);
        assert_eq!(multiple.last_seen.as_deref(), Some("20260410"));
        assert_eq!(multiple.avg_interval_days, Some(10.0 / 3.0));
        assert_eq!(multiple.gone_quiet_since, None);
        assert_eq!(
            sources
                .iter()
                .map(|source| source.activity_id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "middle", "single", "last"]
        );
    }

    #[test]
    fn cadence_uses_strict_quiet_threshold_and_distinct_days() {
        let temporary = journal();
        facet(temporary.path(), "work", false);
        records(
            temporary.path(),
            "work",
            "20260331",
            &[
                interaction("one", 1, "pat", "attendee"),
                interaction("two", 2, "pat", "attendee"),
                interaction("three", 3, "pat", "attendee"),
            ],
        );
        records(
            temporary.path(),
            "work",
            "20260405",
            &[interaction("four", 4, "pat", "attendee")],
        );
        let (boundary, _) =
            compute_cadence(temporary.path(), "pat", false, now()).expect("boundary cadence");
        assert_eq!(boundary.recent_interactions_count_30d, 4);
        assert_eq!(boundary.avg_interval_days, Some(5.0));
        assert_eq!(boundary.gone_quiet_since, None);

        records(
            temporary.path(),
            "work",
            "20260403",
            &[interaction("five", 5, "pat", "attendee")],
        );
        let (quiet, _) =
            compute_cadence(temporary.path(), "pat", false, now()).expect("quiet cadence");
        assert_eq!(quiet.last_seen.as_deref(), Some("20260405"));
        assert_eq!(quiet.avg_interval_days, Some(2.5));
        assert_eq!(quiet.gone_quiet_since, None);

        records(
            temporary.path(),
            "work",
            "20260402",
            &[interaction("quiet-last", 7, "quiet", "attendee")],
        );
        records(
            temporary.path(),
            "work",
            "20260331",
            &[
                interaction("one", 1, "pat", "attendee"),
                interaction("two", 2, "pat", "attendee"),
                interaction("three", 3, "pat", "attendee"),
                interaction("quiet-first", 8, "quiet", "attendee"),
            ],
        );
        let (gone_quiet, _) =
            compute_cadence(temporary.path(), "quiet", false, now()).expect("quiet cadence");
        assert_eq!(gone_quiet.avg_interval_days, Some(2.0));
        assert_eq!(gone_quiet.gone_quiet_since, Some(8));

        records(
            temporary.path(),
            "work",
            "20260409",
            &[interaction("recent", 6, "other", "attendee")],
        );
        let active = list_active_entity_ids(temporary.path(), 30, now()).expect("active IDs");
        assert_eq!(active, vec!["other", "pat", "quiet"]);
    }

    #[test]
    fn cadence_includes_mentions_only_when_requested_and_active_never_does() {
        let temporary = journal();
        facet(temporary.path(), "muted", true);
        records(
            temporary.path(),
            "muted",
            "20260410",
            &[interaction("mention", 1, "pat", "mentioned")],
        );
        let (without_mentions, _) =
            compute_cadence(temporary.path(), "pat", false, now()).expect("cadence");
        let (with_mentions, _) =
            compute_cadence(temporary.path(), "pat", true, now()).expect("cadence");
        assert_eq!(without_mentions.recent_interactions_count_30d, 0);
        assert_eq!(with_mentions.recent_interactions_count_30d, 1);
        assert!(
            list_active_entity_ids(temporary.path(), 30, now())
                .expect("active IDs")
                .is_empty()
        );
    }
}
