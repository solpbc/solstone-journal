// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Body archive, status, and paged-recent payloads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, NaiveDate, Utc};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

use crate::day::{
    DayError, build_day, family, grouped_unsigned, long_day, month_abbr, month_full_name, number,
    short_day, valid_day,
};
use crate::freshness::{build_source_freshness, quiet_day_expectations};
use crate::query::decoded_query_params;
use crate::router::{StoreError, ready_stats, unavailable_response};
use crate::{
    BodyImportInventoryEntry, HealthDedupeStats, MonthReader, coverage_month_keys,
    friendly_type_name, read_body_import_inventory,
};

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static TEST_ARCHIVE_ENTRY_TOTAL_DELTA: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(crate) const RECENT_DAY_LIMIT: usize = 14;
pub(crate) const RECENT_BATCH_LIMIT_CAP: usize = 31;
pub(crate) const STALE_SOURCE_DAYS: i64 = 30;
const FAMILY_ORDER: [&str; 10] = [
    "Sleep",
    "Glucose",
    "Recovery",
    "Activity",
    "Heart",
    "Mindfulness",
    "Hearing & audio",
    "Walking metrics",
    "Body measurements",
    "Other",
];

pub(crate) async fn status_route(State(root): State<Arc<PathBuf>>) -> Response {
    let stats = match ready_stats(&root) {
        Ok(stats) => stats,
        Err(error) => return unavailable_response(error),
    };
    match build_status(&root, stats.as_deref(), Utc::now().date_naive()) {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => unavailable_response(error),
    }
}

pub(crate) async fn recent_route(
    State(root): State<Arc<PathBuf>>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let params = decoded_query_params(raw_query.as_deref().unwrap_or_default());
    let before = params.get("before").map_or("", String::as_str);
    if !valid_day(before) {
        return invalid_day_response();
    }
    let limit = match params.get("limit") {
        None => RECENT_DAY_LIMIT,
        Some(raw) => match raw.parse::<i64>() {
            Ok(limit) if limit < 1 => return invalid_limit_response("limit must be at least 1"),
            Ok(limit) => (limit as usize).min(RECENT_BATCH_LIMIT_CAP),
            Err(_) => return invalid_limit_response("limit must be an integer"),
        },
    };
    let stats = match ready_stats(&root) {
        Ok(stats) => stats,
        Err(error) => return unavailable_response(error),
    };
    let by_day = stats
        .as_ref()
        .map_or_else(BTreeMap::new, |stats| stats.by_day.clone());
    match recent_day_rail(&root, &by_day, stats.as_deref(), Some(before), limit) {
        Ok((days, has_more)) => Json(json!({"days": days, "has_more": has_more})).into_response(),
        Err(error) => unavailable_response(error),
    }
}

pub(crate) fn build_status(
    root: &Path,
    stats: Option<&HealthDedupeStats>,
    today: NaiveDate,
) -> Result<Value, StoreError> {
    let empty = HealthDedupeStats {
        total: 0,
        by_type: BTreeMap::new(),
        by_source: BTreeMap::new(),
        by_month: BTreeMap::new(),
        by_day: BTreeMap::new(),
        type_ranges: BTreeMap::new(),
        coverage_window: crate::HealthDedupeTimeRange {
            first: None,
            last: None,
        },
    };
    let stats = stats.unwrap_or(&empty);
    let imports = imports(root)?;
    let recent = latest_sources_snapshot(root)?;
    let quiet_days = quiet_day_expectations(root);
    let freshness = build_source_freshness(root, &quiet_days, today).map_err(StoreError::Read)?;
    Ok(json!({
        "imports": imports,
        "normalized": {
            "total": stats.total,
            "by_type": stats.by_type,
            "by_source": recent.by_source,
            "by_month": stats.by_month,
        },
        "dedupe": {
            "total": stats.total,
            "by_type": stats.by_type,
            "by_source": stats.by_source,
            "by_month": stats.by_month,
        },
        "coverage_window": {"start": stats.coverage_window.first, "end": stats.coverage_window.last},
        "latest_by_source": recent.latest_by_source,
        "sources_month": recent.month,
        "sources_month_label": recent.month.as_deref().map(month_full_label),
        "day_counts": stats.by_day,
        "freshness": freshness,
        "archive": build_archive(root, stats, &imports, &recent)?,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourcesSnapshot {
    pub(crate) month: Option<String>,
    pub(crate) by_source: BTreeMap<String, u64>,
    pub(crate) latest_by_source: BTreeMap<String, String>,
}

pub(crate) fn latest_sources_snapshot(root: &Path) -> Result<SourcesSnapshot, StoreError> {
    let Some(month) = coverage_month_keys(root)
        .map_err(|error| StoreError::ShardUnreadable(error.to_string()))?
        .into_iter()
        .max()
    else {
        return Ok(SourcesSnapshot {
            month: None,
            by_source: BTreeMap::new(),
            latest_by_source: BTreeMap::new(),
        });
    };
    let mut reader = MonthReader::new(root);
    let rows = reader
        .read_month(&month)
        .map_err(|error| StoreError::ShardUnreadable(error.to_string()))?;
    let mut by_source = BTreeMap::new();
    let mut latest_by_source = BTreeMap::new();
    for row in rows.iter() {
        let source = crate::day::source_label(row);
        *by_source.entry(source.clone()).or_insert(0) += 1;
        if let Some(time) = row.row_time()
            // Source last-seen is chronological across offsets, unlike manifest imported_at.
            && latest_by_source
                .get(&source)
                .is_none_or(|current: &String| later_timestamp(time, current))
        {
            latest_by_source.insert(source, time.to_owned());
        }
    }
    Ok(SourcesSnapshot {
        month: Some(month),
        by_source,
        latest_by_source,
    })
}

fn imports(root: &Path) -> Result<Vec<Value>, StoreError> {
    let inventory =
        read_body_import_inventory(root).map_err(|error| StoreError::Read(error.to_string()))?;
    let mut entries = inventory.entries;
    // The reference deliberately orders manifest imported_at as raw strings, not datetimes.
    entries.sort_by_key(|entry| std::cmp::Reverse(import_sort_key(entry)));
    Ok(entries.into_iter().map(import_manifest).collect())
}

fn import_sort_key(entry: &BodyImportInventoryEntry) -> String {
    entry
        .manifest
        .get("imported_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn import_manifest(entry: BodyImportInventoryEntry) -> Value {
    Value::Object(entry.manifest)
}

fn build_archive(
    root: &Path,
    stats: &HealthDedupeStats,
    imports: &[Value],
    recent: &SourcesSnapshot,
) -> Result<Value, StoreError> {
    let entry_total = stats.total;
    #[cfg(all(test, feature = "full-tests"))]
    let entry_total = entry_total + TEST_ARCHIVE_ENTRY_TOTAL_DELTA.with(std::cell::Cell::get);
    let months = stats.by_month.keys().cloned().collect::<Vec<_>>();
    let days = stats.by_day.keys().cloned().collect::<Vec<_>>();
    let (recent_days, recent_days_has_more) =
        recent_day_rail(root, &stats.by_day, Some(stats), None, RECENT_DAY_LIMIT)?;
    let coverage = match (months.first(), months.last()) {
        (Some(first), Some(last)) => json!({
            "start_month": first, "end_month": last,
            "range_label": format!("{} – {}", month_label(first), month_label(last)),
        }),
        _ => Value::Null,
    };
    Ok(json!({
        "entry_total": entry_total,
        "entry_total_label": grouped(entry_total),
        "import_count": imports.len(),
        "months_observed": stats.by_month.len(),
        "coverage": coverage,
        "latest_day": days.last(),
        "day_grid": day_contribution_grid(&stats.by_day),
        "recent_days": recent_days,
        "recent_days_has_more": recent_days_has_more,
        "families": coverage_families(stats),
        "sources": source_chips(recent),
    }))
}

fn recent_day_rail(
    root: &Path,
    by_day: &BTreeMap<String, u64>,
    stats: Option<&HealthDedupeStats>,
    before: Option<&str>,
    limit: usize,
) -> Result<(Vec<Value>, bool), StoreError> {
    let eligible = by_day
        .keys()
        .filter(|day| before.is_none_or(|before| day.as_str() < before))
        .cloned()
        .collect::<Vec<_>>();
    let has_more = eligible.len() > limit;
    let days = eligible.into_iter().rev().take(limit).collect::<Vec<_>>();
    let mut reader = MonthReader::new(root);
    let mut items = Vec::new();
    for day in days {
        let target = NaiveDate::parse_from_str(&day, "%Y%m%d").expect("aggregate day key");
        let payload = build_day(root, target, stats, &mut reader).map_err(day_error)?;
        let glucose = &payload["glucose"];
        let glucose_label = if glucose["count"].as_u64().unwrap_or_default() > 0
            && glucose["unit"].as_str().is_some_and(|unit| unit != "mixed")
        {
            Some(format!(
                "{}–{} {} · avg {}",
                number(glucose["min"].as_f64().expect("glucose min")),
                number(glucose["max"].as_f64().expect("glucose max")),
                glucose["unit"].as_str().expect("glucose unit"),
                number(glucose["mean"].as_f64().expect("glucose mean")),
            ))
        } else {
            glucose["count"]
                .as_u64()
                .filter(|count| *count > 0)
                .map(|count| format!("{} readings", grouped(count)))
        };
        let sleep = &payload["sleep"];
        let asleep = sleep.get("asleep_duration").and_then(Value::as_str);
        let sleep_duration = asleep.or_else(|| sleep.get("duration").and_then(Value::as_str));
        let sleep_in_bed = asleep.and_then(|asleep| {
            sleep
                .get("in_bed_duration")
                .and_then(Value::as_str)
                .filter(|in_bed| *in_bed != asleep)
        });
        let activity = &payload["activity"];
        let sources = &payload["sources"];
        items.push(json!({
            "day": day,
            "label": short_day(&day),
            "sleep_duration": sleep_duration,
            "sleep_in_bed": sleep_in_bed,
            "glucose_label": glucose_label,
            "workout_count": activity.get("workouts").and_then(Value::as_array).map_or(0, Vec::len),
            "source_count": sources.get("names").and_then(Value::as_array).map_or(0, Vec::len),
        }));
    }
    Ok((items, has_more))
}

fn day_error(error: DayError) -> StoreError {
    match error {
        DayError::Shard(error) => StoreError::ShardUnreadable(error),
        DayError::Store(error) | DayError::Chronicle(error) => StoreError::Read(error),
    }
}

fn coverage_families(stats: &HealthDedupeStats) -> Vec<Value> {
    let mut folded =
        BTreeMap::<&str, (u64, Option<String>, Option<String>, BTreeSet<String>)>::new();
    for (record_type, count) in &stats.by_type {
        let entry = folded
            .entry(family(record_type))
            .or_insert((0, None, None, BTreeSet::new()));
        entry.0 += count;
        entry.3.insert(friendly_type_name(record_type));
        if let Some(range) = stats.type_ranges.get(record_type) {
            if range
                .first
                .as_ref()
                .is_some_and(|value| entry.1.as_ref().is_none_or(|current| value < current))
            {
                entry.1 = range.first.clone();
            }
            if range
                .last
                .as_ref()
                .is_some_and(|value| entry.2.as_ref().is_none_or(|current| value > current))
            {
                entry.2 = range.last.clone();
            }
        }
    }
    FAMILY_ORDER.into_iter().filter_map(|name| {
        let (count, first, last, types) = folded.remove(name)?;
        Some(json!({"name": name, "count": count, "count_label": grouped(count), "range_label": month_range_label(first.as_deref(), last.as_deref()), "types_label": types.into_iter().collect::<Vec<_>>().join(", ")}))
    }).collect()
}

fn source_chips(recent: &SourcesSnapshot) -> Vec<Value> {
    let newest = recent
        .latest_by_source
        .values()
        .filter_map(|time| parse_fixed_offset_time(time))
        .max();
    recent.by_source.iter().map(|(name, count)| {
        let latest = recent
            .latest_by_source
            .get(name)
            .and_then(|time| parse_fixed_offset_time(time));
        let stale = newest
            .zip(latest)
            .is_some_and(|(newest, latest)| newest - latest > chrono::Duration::days(STALE_SOURCE_DAYS));
        json!({"name": name, "count": count, "count_label": grouped(*count), "stale": stale, "last_seen_label": if stale { latest.map(|day| long_day(day.date_naive())) } else { None }})
    }).collect()
}

fn day_contribution_grid(by_day: &BTreeMap<String, u64>) -> Vec<Value> {
    let Some(first_key) = by_day.keys().next() else {
        return Vec::new();
    };
    let Some(last_key) = by_day.keys().next_back() else {
        return Vec::new();
    };
    let first = NaiveDate::parse_from_str(first_key, "%Y%m%d").expect("aggregate day");
    let last = NaiveDate::parse_from_str(last_key, "%Y%m%d").expect("aggregate day");
    let scale = ((by_day.values().max().copied().unwrap_or(0) as f64) + 1.0).ln();
    (first.year()..=last.year())
        .map(|year| {
            let start = first.max(NaiveDate::from_ymd_opt(year, 1, 1).unwrap());
            let end = last.min(NaiveDate::from_ymd_opt(year, 12, 31).unwrap());
            let mut weeks = Vec::<Vec<Value>>::new();
            let mut week = vec![Value::Null; start.weekday().num_days_from_monday() as usize];
            let mut current = start;
            while current <= end {
                let key = current.format("%Y%m%d").to_string();
                let count = by_day.get(&key).copied().unwrap_or(0);
                week.push(grid_cell(&key, count, scale));
                if week.len() == 7 {
                    weeks.push(std::mem::take(&mut week));
                }
                current = current.succ_opt().expect("valid date successor");
            }
            if !week.is_empty() {
                week.resize(7, Value::Null);
                weeks.push(week);
            }
            json!({"year": year, "weeks": weeks, "month_labels": month_label_positions(&weeks)})
        })
        .collect()
}

fn grid_cell(day: &str, count: u64, scale: f64) -> Value {
    let entries = if count == 0 {
        "no entries".to_owned()
    } else if count == 1 {
        "1 entry".to_owned()
    } else {
        format!("{} entries", grouped(count))
    };
    json!({"day": day, "count": count, "intensity": if count == 0 { 0.0 } else { (((count as f64) + 1.0).ln() / scale * 1000.0).round() / 1000.0 }, "title": format!("{}, {} · {entries}", short_day(day).expect("valid aggregate day"), &day[..4])})
}

fn month_label_positions(weeks: &[Vec<Value>]) -> Vec<Value> {
    let mut previous = None;
    weeks
        .iter()
        .enumerate()
        .filter_map(|(index, week)| {
            let day = week
                .iter()
                .find_map(|cell| cell.get("day").and_then(Value::as_str))?;
            let month = &day[4..6];
            (previous != Some(month)).then(|| {
                previous = Some(month);
                json!({"index": index, "label": month_abbr(month.parse().expect("month"))})
            })
        })
        .collect()
}

fn invalid_day_response() -> Response {
    error_envelope(
        "invalid_day",
        "that day couldn't be used.",
        "",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}
fn invalid_limit_response(detail: &str) -> Response {
    error_envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn grouped(value: u64) -> String {
    grouped_unsigned(value)
}
fn parse_fixed_offset_time(value: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(value).ok()
}

fn later_timestamp(candidate: &str, current: &str) -> bool {
    match (
        parse_fixed_offset_time(candidate),
        parse_fixed_offset_time(current),
    ) {
        (Some(candidate), Some(current)) => candidate > current,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => candidate > current,
    }
}

#[cfg(all(test, feature = "full-tests"))]
struct ArchiveEntryTotalPerturbation {
    previous: u64,
}

#[cfg(all(test, feature = "full-tests"))]
impl ArchiveEntryTotalPerturbation {
    fn add(delta: u64) -> Self {
        let previous = TEST_ARCHIVE_ENTRY_TOTAL_DELTA.with(|current| current.replace(delta));
        Self { previous }
    }
}

#[cfg(all(test, feature = "full-tests"))]
impl Drop for ArchiveEntryTotalPerturbation {
    fn drop(&mut self) {
        TEST_ARCHIVE_ENTRY_TOTAL_DELTA.with(|current| current.set(self.previous));
    }
}
fn month_label(month: &str) -> String {
    format!(
        "{} {}",
        month_abbr(month[5..].parse().expect("month")),
        &month[..4]
    )
}
fn month_full_label(month: &str) -> String {
    format!(
        "{} {}",
        month_full_name(month[5..].parse().expect("month")),
        &month[..4]
    )
}
fn month_range_label(first: Option<&str>, last: Option<&str>) -> Option<String> {
    match (first, last) {
        (Some(first), Some(last)) => {
            let first = month_label(&first[..7]);
            let last = month_label(&last[..7]);
            Some(if first == last {
                first
            } else {
                format!("{first} – {last}")
            })
        }
        (Some(first), None) => Some(month_label(&first[..7])),
        (None, Some(last)) => Some(month_label(&last[..7])),
        (None, None) => Some(String::new()),
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use serde_json::Map;
    use tower::ServiceExt;

    use super::*;
    use crate::corpus_test::assert_recorded_payload;
    use crate::{
        BodyAggregateSeed, BodyJournalSeed, BodySeedBundle, BodySeedManifest,
        read_health_dedupe_stats, seed_body_journal,
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-body-archive-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the independent normalized-row fields under test.
    fn test_row(
        key: &str,
        day: &str,
        source: &str,
        record_type: &str,
        value: Option<Value>,
        unit: Option<&str>,
        end: Option<&str>,
        stage: Option<&str>,
    ) -> Map<String, Value> {
        let mut row = Map::from_iter([
            ("dedupe_key".into(), json!(key)),
            ("record_type".into(), json!(record_type)),
            (
                "start_date".into(),
                json!(format!(
                    "{}-{}-{}T01:00:00Z",
                    &day[..4],
                    &day[4..6],
                    &day[6..]
                )),
            ),
            ("day".into(), json!(day)),
            ("source_name".into(), json!(source)),
        ]);
        if let Some(value) = value {
            row.insert("value".into(), value);
        }
        if let Some(unit) = unit {
            row.insert("unit".into(), json!(unit));
        }
        if let Some(end) = end {
            row.insert("end_date".into(), json!(end));
        }
        if let Some(stage) = stage {
            row.insert("metadata".into(), json!({"stage":stage}));
        }
        row
    }

    fn test_bundle(
        import_id: &str,
        month: &str,
        rows: Vec<Map<String, Value>>,
        extra: Map<String, Value>,
    ) -> BodySeedBundle {
        BodySeedBundle {
            import_id: import_id.to_owned(),
            source_family: "apple_health".to_owned(),
            manifest: BodySeedManifest::Present {
                source_type: Some("apple_health".to_owned()),
                entry_count: Some(rows.len() as u64),
                extra,
            },
            shards: if rows.is_empty() {
                BTreeMap::new()
            } else {
                BTreeMap::from([(month.to_owned(), rows)])
            },
        }
    }

    fn test_seed(bundles: Vec<BodySeedBundle>) -> BodyJournalSeed {
        BodyJournalSeed {
            dates: BTreeSet::new(),
            day_summaries: BTreeMap::new(),
            bundles,
            aggregate: BodyAggregateSeed::Direct,
            journal_config: None,
        }
    }

    #[test]
    fn corpus_status_and_recent_payloads_match_recorded_success_cases() {
        let root = TempDir::new();
        crate::day::tests::seed_populated_body_journal(root.path());
        let stats = read_health_dedupe_stats(root.path()).unwrap();
        let first = build_status(
            root.path(),
            stats.as_deref(),
            NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
        )
        .unwrap();
        assert_recorded_payload("first_run", "/app/body/api/status", root.path(), &first);
        let fixed = build_status(
            root.path(),
            stats.as_deref(),
            NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
        )
        .unwrap();
        assert_recorded_payload("fixed", "/app/body/api/status", root.path(), &fixed);
        let (days, has_more) = recent_day_rail(
            root.path(),
            &stats.as_ref().unwrap().by_day,
            stats.as_deref(),
            Some("20260802"),
            RECENT_BATCH_LIMIT_CAP,
        )
        .unwrap();
        assert_recorded_payload(
            "first_run",
            "/app/body/api/recent",
            root.path(),
            &json!({"days": days, "has_more": has_more}),
        );
        assert_recorded_payload(
            "fixed",
            "/app/body/api/recent",
            root.path(),
            &json!({"days": days, "has_more": has_more}),
        );
    }

    #[test]
    fn first_run_status_has_an_explicit_empty_archive() {
        let root = TempDir::new();
        let payload = build_status(
            root.path(),
            None,
            NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
        )
        .unwrap();
        assert_eq!(
            payload["archive"],
            json!({"entry_total":0,"entry_total_label":"0","import_count":0,"months_observed":0,"coverage":null,"latest_day":null,"day_grid":[],"recent_days":[],"recent_days_has_more":false,"families":[],"sources":[]})
        );
    }

    #[tokio::test]
    async fn missing_aggregate_refuses_status_and_recent() {
        let root = TempDir::new();
        let row = serde_json::from_value::<Map<String, Value>>(json!({"dedupe_key":"a","record_type":"Signal","start_date":"2026-08-01T00:00:00Z","day":"20260801"})).unwrap();
        seed_body_journal(
            root.path(),
            &BodyJournalSeed {
                dates: BTreeSet::new(),
                day_summaries: BTreeMap::new(),
                bundles: vec![BodySeedBundle {
                    import_id: "bundle".into(),
                    source_family: "apple_health".into(),
                    manifest: BodySeedManifest::Present {
                        source_type: Some("apple_health".into()),
                        entry_count: Some(1),
                        extra: Map::new(),
                    },
                    shards: BTreeMap::from([("2026-08".into(), vec![row])]),
                }],
                aggregate: BodyAggregateSeed::Absent,
                journal_config: None,
            },
        )
        .unwrap();
        for path in [
            "/app/body/api/status",
            "/app/body/api/recent?before=20260802",
        ] {
            let response = crate::api_router(root.path())
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(body["reason_code"], "body_store_aggregate_missing");
        }
    }

    #[tokio::test]
    async fn unreadable_aggregate_refuses_status_and_recent_with_the_shipped_reason() {
        let root = TempDir::new();
        crate::day::tests::seed_populated_body_journal(root.path());
        fs::write(
            root.path().join("imports/health-dedupe.sqlite"),
            "not sqlite",
        )
        .unwrap();
        for path in [
            "/app/body/api/status",
            "/app/body/api/recent?before=20260802",
        ] {
            let response = crate::api_router(root.path())
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(
                body["reason_code"], "body_store_aggregate_unreadable",
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn unreadable_shard_follows_healthy_verdict_then_refuses_with_shard_reason() {
        let root = TempDir::new();
        crate::day::tests::seed_populated_body_journal(root.path());
        fs::write(
            root.path()
                .join("imports/20260810_080000/normalized/2026-08.jsonl"),
            "not json\n",
        )
        .unwrap();
        assert!(matches!(
            crate::read_body_store_health(root.path()).unwrap(),
            crate::BodyStoreHealthVerdict::Healthy(_)
        ));
        for path in [
            "/app/body/api/status",
            "/app/body/api/recent?before=20260802",
        ] {
            let response = crate::api_router(root.path())
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(body["reason_code"], "body_store_shard_unreadable", "{path}");
        }
    }

    #[tokio::test]
    async fn recent_refusal_ladder_has_exactly_three_envelopes() {
        let root = TempDir::new();
        let mut envelopes = BTreeSet::new();
        for path in [
            "/app/body/api/recent",
            "/app/body/api/recent?before=nope",
            "/app/body/api/recent?before=20260230",
            "/app/body/api/recent?before=20260802&limit=nope",
            "/app/body/api/recent?before=20260802&limit=0",
            "/app/body/api/recent?before=20260802&limit=-1",
        ] {
            let response = crate::api_router(root.path())
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            envelopes.insert((
                body["reason_code"].as_str().unwrap().to_owned(),
                body["error"].as_str().unwrap().to_owned(),
                body["detail"].as_str().unwrap().to_owned(),
            ));
        }
        assert_eq!(envelopes.len(), 3);
        assert!(envelopes.contains(&(
            "invalid_day".into(),
            "that day couldn't be used.".into(),
            "".into()
        )));
        assert!(envelopes.contains(&(
            "invalid_request_value".into(),
            "one of those values couldn't be used.".into(),
            "limit must be an integer".into()
        )));
        assert!(envelopes.contains(&(
            "invalid_request_value".into(),
            "one of those values couldn't be used.".into(),
            "limit must be at least 1".into()
        )));
    }

    #[tokio::test]
    async fn recent_route_decodes_percent_encoded_before_and_limit() {
        let root = TempDir::new();
        seed_body_journal(
            root.path(),
            &test_seed(vec![test_bundle(
                "encoded",
                "2026-08",
                vec![test_row(
                    "encoded-row",
                    "20260801",
                    "Source",
                    "Signal",
                    Some(json!(1)),
                    None,
                    None,
                    None,
                )],
                Map::new(),
            )]),
        )
        .unwrap();
        let response = crate::api_router(root.path())
            .oneshot(
                Request::get("/app/body/api/recent?before=202608%30%32&limit=%31")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["days"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn corpus_comparator_rejects_a_native_builder_perturbation() {
        let root = TempDir::new();
        crate::day::tests::seed_populated_body_journal(root.path());
        let stats = read_health_dedupe_stats(root.path()).unwrap();
        let failure = catch_unwind(AssertUnwindSafe(|| {
            let _perturbation = ArchiveEntryTotalPerturbation::add(1);
            let payload = build_status(
                root.path(),
                stats.as_deref(),
                NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
            )
            .unwrap();
            assert_recorded_payload("first_run", "/app/body/api/status", root.path(), &payload);
        }))
        .expect_err("the builder perturbation must fail corpus replay");
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .expect("string panic message");
        assert_eq!(
            message,
            "recorded corpus structural mismatch: $.archive.entry_total: left=571; right=570"
        );
    }

    #[test]
    fn empty_aggregate_and_populated_shards_keep_the_two_folds_separate() {
        let root = TempDir::new();
        crate::day::tests::seed_populated_body_journal(root.path());
        fs::remove_file(root.path().join("imports/health-dedupe.sqlite")).unwrap();
        seed_body_journal(
            root.path(),
            &BodyJournalSeed {
                dates: BTreeSet::new(),
                day_summaries: BTreeMap::new(),
                bundles: Vec::new(),
                aggregate: BodyAggregateSeed::Empty,
                journal_config: None,
            },
        )
        .unwrap();
        assert_eq!(
            read_health_dedupe_stats(root.path())
                .unwrap()
                .unwrap()
                .total,
            0
        );
        let snapshot = latest_sources_snapshot(root.path()).unwrap();
        assert_eq!(snapshot.month.as_deref(), Some("2026-08"));
        assert!(!snapshot.by_source.is_empty());
    }

    #[test]
    fn gap_day_is_absent_from_the_rail_and_present_in_the_grid() {
        let root = TempDir::new();
        seed_body_journal(
            root.path(),
            &test_seed(vec![test_bundle(
                "gap",
                "2025-01",
                vec![
                    test_row(
                        "first",
                        "20250101",
                        "Source",
                        "Signal",
                        Some(json!(1)),
                        None,
                        None,
                        None,
                    ),
                    test_row(
                        "last",
                        "20250103",
                        "Source",
                        "Signal",
                        Some(json!(1)),
                        None,
                        None,
                        None,
                    ),
                ],
                Map::new(),
            )]),
        )
        .unwrap();
        let stats = read_health_dedupe_stats(root.path()).unwrap();
        let by_day = &stats.as_ref().unwrap().by_day;
        let (rail, _) =
            recent_day_rail(root.path(), by_day, stats.as_deref(), Some("20250104"), 14).unwrap();
        assert!(rail.iter().all(|item| item["day"] != "20250102"));
        let grid = day_contribution_grid(by_day);
        assert_eq!(grid[0]["weeks"][0][3]["day"], "20250102");
        assert_eq!(grid[0]["weeks"][0][3]["count"], 0);
        assert_eq!(grid[0]["weeks"][0][3]["title"], "Jan 2, 2025 · no entries");
        assert!(!by_day.contains_key("20250102"));
    }

    #[test]
    fn grid_has_two_year_blocks_and_both_leading_pads() {
        let by_day = BTreeMap::from([("20241231".to_owned(), 1), ("20250102".to_owned(), 1)]);
        let grid = day_contribution_grid(&by_day);
        assert_eq!(grid.len(), 2);
        assert_ne!(
            NaiveDate::from_ymd_opt(2025, 1, 1)
                .unwrap()
                .weekday()
                .num_days_from_monday(),
            0
        );
        assert_eq!(grid[0]["weeks"][0][0], Value::Null);
        assert_eq!(grid[0]["weeks"][0][1]["day"], "20241231");
        assert_eq!(grid[1]["weeks"][0][0], Value::Null);
        assert_eq!(grid[1]["weeks"][0][1], Value::Null);
        assert_eq!(grid[1]["weeks"][0][2]["day"], "20250101");
        assert_eq!(grid[1]["weeks"][0][2]["count"], 0);
        assert_eq!(grid[1]["weeks"][0][2]["title"], "Jan 1, 2025 · no entries");
    }

    #[test]
    fn source_chip_staleness_uses_newest_source_not_today() {
        let snapshot = SourcesSnapshot {
            month: Some("2026-08".into()),
            by_source: BTreeMap::from([("quiet".into(), 1), ("fresh".into(), 1)]),
            latest_by_source: BTreeMap::from([
                ("quiet".into(), "2026-08-01T01:00:00+00:00".into()),
                ("fresh".into(), "2026-08-31T23:00:00+00:00".into()),
            ]),
        };
        let chips = source_chips(&snapshot);
        let quiet = chips.iter().find(|chip| chip["name"] == "quiet").unwrap();
        let fresh = chips.iter().find(|chip| chip["name"] == "fresh").unwrap();
        assert!(quiet["stale"].as_bool().unwrap());
        assert!(!fresh["stale"].as_bool().unwrap());
    }

    #[test]
    fn snapshot_is_bounded_to_the_newest_month() {
        let root = TempDir::new();
        let old = test_row(
            "old",
            "20260701",
            "Old Source",
            "Signal",
            Some(json!(1)),
            None,
            None,
            None,
        );
        let newest = test_row(
            "new",
            "20260801",
            "New Source",
            "Signal",
            Some(json!(1)),
            None,
            None,
            None,
        );
        seed_body_journal(
            root.path(),
            &test_seed(vec![
                test_bundle("old", "2026-07", vec![old], Map::new()),
                test_bundle("new", "2026-08", vec![newest], Map::new()),
            ]),
        )
        .unwrap();
        let snapshot = latest_sources_snapshot(root.path()).unwrap();
        assert_eq!(snapshot.month.as_deref(), Some("2026-08"));
        assert_eq!(
            snapshot.by_source,
            BTreeMap::from([("New Source".to_owned(), 1)])
        );
    }

    #[test]
    fn snapshot_chooses_latest_source_timestamp_chronologically_across_offsets() {
        let root = TempDir::new();
        let mut offset_earlier = test_row(
            "offset-earlier",
            "20260801",
            "Watch",
            "Signal",
            Some(json!(1)),
            None,
            None,
            None,
        );
        offset_earlier.insert("start_date".into(), json!("2026-08-01T10:00:00+02:00"));
        let mut utc_later = test_row(
            "utc-later",
            "20260801",
            "Watch",
            "Signal",
            Some(json!(1)),
            None,
            None,
            None,
        );
        utc_later.insert("start_date".into(), json!("2026-08-01T09:00:00Z"));
        seed_body_journal(
            root.path(),
            &test_seed(vec![test_bundle(
                "mixed-offsets",
                "2026-08",
                vec![offset_earlier, utc_later],
                Map::new(),
            )]),
        )
        .unwrap();
        let snapshot = latest_sources_snapshot(root.path()).unwrap();
        assert_eq!(snapshot.latest_by_source["Watch"], "2026-08-01T09:00:00Z");
    }

    #[test]
    fn imports_sort_by_raw_manifest_timestamp_and_empty_bundle_uses_directory_id() {
        let root = TempDir::new();
        seed_body_journal(
            root.path(),
            &test_seed(vec![
                test_bundle(
                    "chronologically-later",
                    "2026-08",
                    vec![test_row(
                        "row",
                        "20260801",
                        "Source",
                        "Signal",
                        Some(json!(1)),
                        None,
                        None,
                        None,
                    )],
                    Map::from_iter([("imported_at".into(), json!("2026-08-10T08:30:00Z"))]),
                ),
                test_bundle(
                    "lexically-newer-directory",
                    "2026-08",
                    Vec::new(),
                    Map::from_iter([("imported_at".into(), json!("2026-08-10T09:00:00+02:00"))]),
                ),
            ]),
        )
        .unwrap();
        let stats = read_health_dedupe_stats(root.path()).unwrap();
        let payload = build_status(
            root.path(),
            stats.as_deref(),
            NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
        )
        .unwrap();
        assert_eq!(
            payload["imports"][0]["import_id"],
            "lexically-newer-directory"
        );
        assert_eq!(payload["imports"][0]["normalized_months_label"], "—");
        assert_eq!(payload["imports"][1]["import_id"], "chronologically-later");
    }

    #[test]
    fn rail_uses_glucose_ladder_and_stage_aware_sleep_pair() {
        let root = TempDir::new();
        let mut partial_asleep = test_row(
            "partial-asleep",
            "20260104",
            "Watch",
            "HKCategoryTypeIdentifierSleepAnalysis",
            None,
            None,
            Some("2026-01-04T05:00:00Z"),
            Some("asleep"),
        );
        partial_asleep.insert("start_date".into(), json!("2026-01-03T23:00:00Z"));
        let mut partial_bed = test_row(
            "partial-bed",
            "20260104",
            "Watch",
            "HKCategoryTypeIdentifierSleepAnalysis",
            None,
            None,
            Some("2026-01-04T07:00:00Z"),
            Some("in bed"),
        );
        partial_bed.insert("start_date".into(), json!("2026-01-03T23:00:00Z"));
        let mut full_asleep = test_row(
            "full-asleep",
            "20260105",
            "Watch",
            "HKCategoryTypeIdentifierSleepAnalysis",
            None,
            None,
            Some("2026-01-05T07:00:00Z"),
            Some("asleep"),
        );
        full_asleep.insert("start_date".into(), json!("2026-01-04T23:00:00Z"));
        seed_body_journal(
            root.path(),
            &test_seed(vec![test_bundle(
                "rail",
                "2026-01",
                vec![
                    test_row(
                        "known-a",
                        "20260101",
                        "CGM",
                        "HKQuantityTypeIdentifierBloodGlucose",
                        Some(json!(90)),
                        Some("mg/dL"),
                        None,
                        None,
                    ),
                    test_row(
                        "known-b",
                        "20260101",
                        "CGM",
                        "HKQuantityTypeIdentifierBloodGlucose",
                        Some(json!(110)),
                        Some("mg/dL"),
                        None,
                        None,
                    ),
                    test_row(
                        "count-a",
                        "20260102",
                        "CGM",
                        "HKQuantityTypeIdentifierBloodGlucose",
                        Some(json!(90)),
                        None,
                        None,
                        None,
                    ),
                    test_row(
                        "count-b",
                        "20260102",
                        "CGM",
                        "HKQuantityTypeIdentifierBloodGlucose",
                        Some(json!(110)),
                        None,
                        None,
                        None,
                    ),
                    test_row(
                        "no-glucose",
                        "20260103",
                        "Watch",
                        "Signal",
                        Some(json!(1)),
                        None,
                        None,
                        None,
                    ),
                    partial_asleep,
                    partial_bed,
                    full_asleep,
                    test_row(
                        "sleep-differ-day",
                        "20260104",
                        "Watch",
                        "Signal",
                        Some(json!(1)),
                        None,
                        None,
                        None,
                    ),
                    test_row(
                        "sleep-match-day",
                        "20260105",
                        "Watch",
                        "Signal",
                        Some(json!(1)),
                        None,
                        None,
                        None,
                    ),
                ],
                Map::new(),
            )]),
        )
        .unwrap();
        let stats = read_health_dedupe_stats(root.path()).unwrap();
        let (rail, _) = recent_day_rail(
            root.path(),
            &stats.as_ref().unwrap().by_day,
            stats.as_deref(),
            Some("20260107"),
            14,
        )
        .unwrap();
        let find = |day: &str| rail.iter().find(|item| item["day"] == day).unwrap();
        assert_eq!(find("20260101")["glucose_label"], "90–110 mg/dL · avg 100");
        assert_eq!(find("20260102")["glucose_label"], "2 readings");
        assert_eq!(find("20260103")["glucose_label"], Value::Null);
        assert_eq!(find("20260104")["sleep_duration"], "6h 00m");
        assert_eq!(find("20260104")["sleep_in_bed"], "8h 00m");
        assert_eq!(find("20260105")["sleep_duration"], "8h 00m");
        assert_eq!(find("20260105")["sleep_in_bed"], Value::Null);
    }

    #[test]
    fn rendered_status_count_labels_group_large_seeded_totals() {
        let root = TempDir::new();
        let rows = (0..1_001)
            .map(|index| {
                test_row(
                    &format!("row-{index}"),
                    "20260801",
                    "Large Source",
                    "Signal",
                    Some(json!(index)),
                    None,
                    None,
                    None,
                )
            })
            .collect();
        seed_body_journal(
            root.path(),
            &test_seed(vec![test_bundle("large", "2026-08", rows, Map::new())]),
        )
        .unwrap();
        let stats = read_health_dedupe_stats(root.path()).unwrap();
        let payload = build_status(
            root.path(),
            stats.as_deref(),
            NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
        )
        .unwrap();
        assert_eq!(payload["archive"]["entry_total_label"], "1,001");
        assert_eq!(payload["archive"]["families"][0]["count_label"], "1,001");
        assert_eq!(payload["archive"]["sources"][0]["count_label"], "1,001");
    }

    #[test]
    fn cursor_walk_exhausts_the_archive_once_and_default_is_fourteen() {
        // The route requires a cursor; starting strictly newer than the newest
        // entry day is what makes a paged walk visit every counted day.
        let root = TempDir::new();
        crate::day::tests::seed_populated_body_journal(root.path());
        let stats = read_health_dedupe_stats(root.path()).unwrap();
        let by_day = &stats.as_ref().unwrap().by_day;
        let (initial, has_more) = recent_day_rail(
            root.path(),
            by_day,
            stats.as_deref(),
            Some("20260802"),
            RECENT_DAY_LIMIT,
        )
        .unwrap();
        assert_eq!(initial.len(), RECENT_DAY_LIMIT);
        assert!(has_more);
        let mut seen = BTreeSet::new();
        let mut before = "20260802".to_owned();
        loop {
            let (days, more) = recent_day_rail(
                root.path(),
                by_day,
                stats.as_deref(),
                Some(&before),
                RECENT_DAY_LIMIT,
            )
            .unwrap();
            if days.is_empty() {
                assert!(!more);
                break;
            }
            let oldest = days.last().unwrap()["day"].as_str().unwrap().to_owned();
            for day in days {
                assert!(seen.insert(day["day"].as_str().unwrap().to_owned()));
            }
            before = oldest;
            if !more {
                break;
            }
        }
        assert_eq!(seen, by_day.keys().cloned().collect());
    }
}
