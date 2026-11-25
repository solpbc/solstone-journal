// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Garmin Connect Personal API sync engine.
//!
//! Fetches all Garmin Personal API JSON experience and wellness categories into
//! `chronicle/<YYYYMMDD>/import.garmin/<window>/body.jsonl` using the shared
//! body placement, filing, publication, activity-record, and per-run Trash
//! contracts.
//!
//! Key invariants:
//! - Strictly owner-local: reads the owner's `GCDC_` key from `config/journal.json`,
//!   exchanges directly with `https://personal.connectapi.garmin.com` via a `0o600`
//!   24h bearer JWT cache, and never downloads `.fit` archives.
//! - Conditional run allocation: creates `imports/<YYYYMMDD_HHMMSS>/` only when a
//!   run actually writes rows or updates window filings; quiet hourly polls leave
//!   zero empty run directories.
//! - Rate-limit and server-quirk safe: respects `650ms` transport pacing, retries
//!   `429` with `Retry-After` / exponential backoff, bisects HTTP `500` on
//!   `search/activities` (`40 -> 20 -> 10`) and `v2/dailysummaries` (`31d -> 7d -> 1d`),
//!   smart-gates single-day HR/SpO2/HRV endpoints from ranged summaries, and
//!   adaptively probes Women's Health and Nutrition so accounts that don't log
//!   them never burn 365 calls/year.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use solstone_core_format::content::body_records::{BODY_RECORDS_FILE, RowKind};
use solstone_core_import::garmin::{
    self, BackfillState, COVERAGE_REL, Error, GarminHttp, LiveGarminHttp, RunRecord,
};
use solstone_core_import::oura_fetched::Fetched;
use solstone_core_import::publish::{
    CreatedSegment, NativePublicationOperations, PublicationInput, PublicationStatus,
    publish_with_operations,
};
use solstone_core_import::text::TextCreated;
use solstone_core_import_sources::apple_health::{
    DayRows, WindowId, decide_filing, read_or_create_zone, settle_readings_noting, stored_rows,
    stored_windows_of_day,
};
use solstone_core_import_sources::body_placement::DayPlacement;
use solstone_core_import_sources::body_stream::GARMIN;
use solstone_core_import_sources::garmin::{ReadRow, place_rows_by_day, read_page};
use solstone_core_import_sources::save;
use solstone_core_journal_io::{
    AtomicWriteOptions, JsonWriteOptions, atomic_replace, create_directory_with_mode, write_json,
};

use crate::body_segments::{ListedMoves, holds_a_run};
use crate::import_publication::{ImportTerminalInput, finish_import_attempt_indexing};
use crate::registry::now_ms;

/// Earliest calendar date progressive newest-first backfill walks back to.
pub const HISTORY_FLOOR: (i16, i8, i8) = (2015, 1, 1);
/// Ranged endpoints are queried in 31-day inclusive chunks (avoids Garmin's >90d 500 bug).
const CHUNK_DAYS: i64 = 31;
/// Every ordinary sync re-reads the trailing 7 calendar days for watch-sync convergence.
const TRAILING_DAYS: i64 = 7;
/// Trailing single-day HR/SpO2 convergence depth on hourly checks.
const TRAILING_DETAIL_DAYS: i64 = 3;
/// Maximum 31-day backfill chunks executed in a single run (~1 year per run; resumes seamlessly).
const MAX_BACKFILL_CHUNKS_PER_RUN: usize = 12;
/// Stop walking backward once this many consecutive 31-day chunks produce zero rows.
const EMPTY_CHUNKS_STOP: u32 = 6;
/// Initial page size for `search/activities` (Garmin `500`s above ~45 including default 99).
const INITIAL_ACTIVITY_LIMIT: usize = 40;
/// Maximum pages fetched per chunk on `search/activities`.
const MAX_ACTIVITY_PAGES: usize = 50;
/// Maximum retries on HTTP `429` before surfacing `Error::Limited`.
const MAX_429_RETRIES: u32 = 3;
/// Re-probe inactive Women's Health every 7 days on hourly checks.
const WOMENS_HEALTH_REPROBE_DAYS: i64 = 7;
/// Re-probe inactive Nutrition once per day on hourly checks.
const NUTRITION_REPROBE_DAYS: i64 = 1;
const LISTING_FILE: &str = "body-touched.jsonl";
const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// Request parameters for one Garmin sync invocation.
pub struct GarminSyncRequest<'a> {
    /// Owner-local timestamp `YYYYMMDD_HHMMSS` used if this run allocates an import record.
    pub timestamp: &'a str,
    pub facet: Option<&'a str>,
    pub listed: ListedMoves,
    /// Explicit owner date range (inclusive). `None` runs trailing 7d convergence + progressive backfill.
    pub range: Option<(Date, Date)>,
    /// True when invoked by the hourly scheduler (`--scheduled`).
    pub scheduled: bool,
    pub http: &'a mut dyn GarminHttp,
    /// Optional clock seam for deterministic tests.
    pub clock: Option<&'a dyn Fn() -> Timestamp>,
    /// Optional sleep seam for deterministic backoff tests.
    pub sleep_fn: Option<&'a dyn Fn(Duration)>,
}

impl<'a> GarminSyncRequest<'a> {
    #[must_use]
    pub fn new(timestamp: &'a str, facet: Option<&'a str>, http: &'a mut dyn GarminHttp) -> Self {
        Self {
            timestamp,
            facet,
            listed: ListedMoves::Keep,
            range: None,
            scheduled: false,
            http,
            clock: None,
            sleep_fn: None,
        }
    }
}

/// Observable summary of one Garmin sync invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GarminSyncOutcome {
    /// Allocated `imports/<id>/` run id, or `None` when nothing changed.
    pub import_id: Option<String>,
    pub rows_written: u64,
    pub windows_published: u64,
    pub workouts_written: u64,
    pub days_touched: Vec<String>,
    pub backfill: BackfillState,
    pub paused_for_key: bool,
    pub summary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ProbeState {
    #[default]
    Unknown,
    Active,
    Inactive {
        last_checked: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Coverage {
    #[serde(default)]
    backfill_complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backfill_reached: Option<String>,
    #[serde(default)]
    consecutive_empty_chunks: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_trailing_to: Option<String>,
    #[serde(default)]
    womens_health: ProbeState,
    #[serde(default)]
    nutrition: ProbeState,
}

impl Coverage {
    fn load(journal: &Path) -> Result<Self, Error> {
        match fs::read(journal.join(COVERAGE_REL)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Storage),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err(Error::Storage),
        }
    }

    fn save(&self, journal: &Path) -> Result<(), Error> {
        create_directory_with_mode(&journal.join("imports"), 0o700).map_err(|_| Error::Storage)?;
        write_json(
            journal.join(COVERAGE_REL),
            self,
            JsonWriteOptions {
                mode: Some(0o600),
                ..Default::default()
            },
        )
        .map_err(|_| Error::Storage)
    }

    fn backfill_state(&self) -> BackfillState {
        BackfillState {
            complete: self.backfill_complete,
            reached_day: self.backfill_reached.clone(),
        }
    }
}

/// Human-readable dry-run plan for `--sync garmin` (without `--save`).
pub fn plan_lines(
    journal: &Path,
    range: Option<(Date, Date)>,
    today: Date,
) -> Result<String, Error> {
    let connected = garmin::key(journal)?.is_some();
    let coverage = Coverage::load(journal)?;
    let mut out = String::new();
    if !connected {
        out.push_str("Garmin Connect is not connected.\n");
        return Ok(out);
    }
    if let Some((from, to)) = range {
        out.push_str(&format!(
            "Garmin Connect dry run: would sync {from} through {to} (explicit range, reclaims deleted entries).\n"
        ));
    } else {
        let trailing_from = clamp_floor(sub_days(today, TRAILING_DAYS - 1));
        out.push_str(&format!(
            "Garmin Connect dry run: would check trailing days {trailing_from} through {today}.\n"
        ));
        if !coverage.backfill_complete {
            let next_end = coverage
                .backfill_reached
                .as_deref()
                .and_then(|s| Date::strptime("%Y-%m-%d", s).ok())
                .map(|d| sub_days(d, 1))
                .unwrap_or_else(|| sub_days(trailing_from, 1));
            let floor = history_floor();
            if next_end >= floor {
                out.push_str(&format!(
                    "Progressive backfill would continue backward from {next_end} toward {floor}.\n"
                ));
            }
        } else {
            out.push_str("Historical backfill is complete.\n");
        }
    }
    Ok(out)
}

/// Execute a live Garmin Connect sync using the production fixed-origin HTTP transport.
pub fn sync_garmin_live(
    journal: &Path,
    timestamp: &str,
    facet: Option<&str>,
    listed: ListedMoves,
    range: Option<(Date, Date)>,
    scheduled: bool,
) -> Result<GarminSyncOutcome, Error> {
    let api_key = garmin::key(journal)?.ok_or(Error::NotConnected)?;
    let mut http = LiveGarminHttp::new(journal.to_path_buf(), api_key);
    let mut request = GarminSyncRequest::new(timestamp, facet, &mut http);
    request.listed = listed;
    request.range = range;
    request.scheduled = scheduled;
    sync_garmin(journal, request)
}

/// Execute one Garmin Connect sync run against `request.http`.
pub fn sync_garmin(
    journal: &Path,
    mut request: GarminSyncRequest<'_>,
) -> Result<GarminSyncOutcome, Error> {
    if let Some(facet) = request.facet {
        crate::cli_argv::require_enabled_facet(facet, journal).map_err(|_| Error::Storage)?;
    }

    let now = request.clock.map_or_else(Timestamp::now, |clock| clock());
    let now_iso = now.to_string();

    if garmin::key(journal)?.is_none() {
        return Err(Error::NotConnected);
    }
    if request.scheduled && garmin::paused_for_key_error(journal) {
        let state = garmin::read_state(journal).unwrap_or_default();
        return Ok(GarminSyncOutcome {
            import_id: None,
            rows_written: 0,
            windows_published: 0,
            workouts_written: 0,
            days_touched: Vec::new(),
            backfill: state.backfill,
            paused_for_key: true,
            summary: "Garmin Connect check skipped until a new key is saved.".to_owned(),
        });
    }

    let _lock = garmin::hold(journal)?;
    if solstone_core_import::body_trash::source_trash_held(journal, GARMIN.source, "").is_some() {
        return Err(Error::Busy);
    }

    let Some(api_key) = garmin::key(journal)? else {
        return Err(Error::NotConnected);
    };

    let zone = read_or_create_zone(journal, &GARMIN).map_err(|_| Error::Storage)?;
    let today = now.to_zoned_datetime(zone.clone()).date();
    let scheduled = request.scheduled;

    let result = run_locked(journal, &api_key, &zone, today, now, &mut request);
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            if !matches!(error, Error::NotConnected | Error::Busy) {
                let _ = garmin::record_sync_failure(journal, &now_iso, scheduled, error);
            }
            Err(error)
        }
    }
}

struct Session<'a> {
    journal: &'a Path,
    api_key: &'a str,
    bearer: String,
    http: &'a mut dyn GarminHttp,
    clock: Option<&'a dyn Fn() -> Timestamp>,
    sleep_fn: Option<&'a dyn Fn(Duration)>,
    retried_401: bool,
}

impl<'a> Session<'a> {
    fn open(
        journal: &'a Path,
        api_key: &'a str,
        http: &'a mut dyn GarminHttp,
        clock: Option<&'a dyn Fn() -> Timestamp>,
        sleep_fn: Option<&'a dyn Fn(Duration)>,
    ) -> Result<Self, Error> {
        let now_s = clock.map_or_else(Timestamp::now, |c| c()).as_second();
        let token = garmin::ensure_bearer_token(journal, http, api_key, false, now_s)?;
        Ok(Self {
            journal,
            api_key,
            bearer: token.access_token,
            http,
            clock,
            sleep_fn,
            retried_401: false,
        })
    }

    fn check_continues(&self) -> Result<(), Error> {
        if !solstone_core_import::work_continues() {
            return Err(Error::Stopped);
        }
        let current = garmin::key(self.journal)?;
        if current.as_deref() != Some(self.api_key) {
            return Err(Error::Stopped);
        }
        Ok(())
    }

    fn sleep(&self, duration: Duration) {
        if let Some(sleep_fn) = self.sleep_fn {
            sleep_fn(duration);
        } else {
            std::thread::sleep(duration);
        }
    }

    fn get_json(
        &mut self,
        path: &str,
        query: &[(&str, String)],
        allow_not_configured: bool,
    ) -> Result<Option<Value>, Error> {
        let mut rate_attempt = 0_u32;
        loop {
            self.check_continues()?;
            let response = self.http.get_json(&self.bearer, path, query)?;
            match response.status {
                200 => {
                    let trimmed = trim_ascii(&response.body);
                    if trimmed.is_empty() {
                        return Ok(None);
                    }
                    let value: Value =
                        serde_json::from_slice(trimmed).map_err(|_| Error::Response)?;
                    return Ok(Some(value));
                }
                204 => return Ok(None),
                401 => {
                    if self.retried_401 {
                        garmin::clear_token_cache(self.journal);
                        return Err(Error::Key);
                    }
                    self.retried_401 = true;
                    let now_s = self.clock.map_or_else(Timestamp::now, |c| c()).as_second();
                    let fresh = garmin::ensure_bearer_token(
                        self.journal,
                        self.http,
                        self.api_key,
                        true,
                        now_s,
                    )?;
                    self.bearer = fresh.access_token;
                }
                403 => {
                    if allow_not_configured {
                        return Ok(None);
                    }
                    return Err(Error::Access);
                }
                404 => return Ok(None),
                429 => {
                    if rate_attempt >= MAX_429_RETRIES {
                        return Err(Error::Limited);
                    }
                    let wait_secs = response
                        .retry_after_secs
                        .unwrap_or_else(|| 2_u64 << rate_attempt)
                        .clamp(1, 60);
                    rate_attempt += 1;
                    self.sleep(Duration::from_secs(wait_secs));
                }
                500..=599 => return Err(Error::Unavailable),
                _ => return Err(Error::Response),
            }
        }
    }
}

fn run_locked(
    journal: &Path,
    api_key: &str,
    zone: &TimeZone,
    today: Date,
    now: Timestamp,
    request: &mut GarminSyncRequest<'_>,
) -> Result<GarminSyncOutcome, Error> {
    let mut coverage = Coverage::load(journal)?;
    let mut fetched = Fetched::load_for(journal, GARMIN.source).map_err(|_| Error::Storage)?;
    let comes_back = request.range.is_some() && !request.scheduled;
    let mut session = Session::open(
        journal,
        api_key,
        request.http,
        request.clock,
        request.sleep_fn,
    )?;

    let mut collected: Vec<ReadRow> = Vec::new();
    let progress_backfill;

    if let Some((from, to)) = request.range {
        if from > to || to > today {
            return Err(Error::InvalidRange);
        }
        for (chunk_start, chunk_end) in split_chunks_newest_first(from, to, CHUNK_DAYS) {
            let chunk_rows = fetch_chunk(
                &mut session,
                chunk_start,
                chunk_end,
                today,
                &mut coverage,
                &fetched,
                comes_back,
                true,
            )?;
            collected.extend(chunk_rows);
        }
        coverage.save(journal)?;
        progress_backfill = coverage.backfill_state();
    } else {
        // Phase 1: trailing 7-day convergence (always runs first so today's sleep,
        // workouts, naps, and daily totals stay current).
        let trailing_from = clamp_floor(sub_days(today, TRAILING_DAYS - 1));
        let trailing_rows = fetch_chunk(
            &mut session,
            trailing_from,
            today,
            today,
            &mut coverage,
            &fetched,
            false,
            false,
        )?;
        collected.extend(trailing_rows);
        coverage.last_trailing_to = Some(iso_date(today));
        if coverage.backfill_reached.is_none() {
            coverage.backfill_reached = Some(iso_date(trailing_from));
        }
        coverage.save(journal)?;

        // Phase 2: progressive newest-first 31-day backfill down to HISTORY_FLOOR.
        let floor = history_floor();
        if !coverage.backfill_complete {
            let mut cursor_end = coverage
                .backfill_reached
                .as_deref()
                .and_then(|s| Date::strptime("%Y-%m-%d", s).ok())
                .map(|d| sub_days(d, 1))
                .unwrap_or_else(|| sub_days(trailing_from, 1));
            for _ in 0..MAX_BACKFILL_CHUNKS_PER_RUN {
                if cursor_end < floor {
                    coverage.backfill_complete = true;
                    break;
                }
                let chunk_start = clamp_floor(sub_days(cursor_end, CHUNK_DAYS - 1));
                let chunk_rows = fetch_chunk(
                    &mut session,
                    chunk_start,
                    cursor_end,
                    today,
                    &mut coverage,
                    &fetched,
                    false,
                    false,
                )?;
                if chunk_rows.is_empty() {
                    coverage.consecutive_empty_chunks =
                        coverage.consecutive_empty_chunks.saturating_add(1);
                } else {
                    coverage.consecutive_empty_chunks = 0;
                    collected.extend(chunk_rows);
                }
                coverage.backfill_reached = Some(iso_date(chunk_start));
                if chunk_start <= floor || coverage.consecutive_empty_chunks >= EMPTY_CHUNKS_STOP {
                    coverage.backfill_complete = true;
                }
                coverage.save(journal)?;
                update_backfill_progress_only(journal, coverage.backfill_state());
                if coverage.backfill_complete {
                    break;
                }
                cursor_end = sub_days(chunk_start, 1);
            }
        }
        progress_backfill = coverage.backfill_state();
    }

    persist_run(
        journal,
        zone,
        now,
        request.timestamp,
        request.facet,
        request.listed,
        comes_back,
        &collected,
        &mut fetched,
        progress_backfill,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "cohesive per-chunk fetch context across Garmin's ranged and gated endpoints"
)]
fn fetch_chunk(
    session: &mut Session<'_>,
    start: Date,
    end: Date,
    today: Date,
    coverage: &mut Coverage,
    fetched: &Fetched,
    comes_back: bool,
    explicit_range: bool,
) -> Result<Vec<ReadRow>, Error> {
    let start_str = iso_date(start);
    let end_str = iso_date(end);
    let mut rows = Vec::new();

    // 1. Activities (#4 search/activities + #2 activity/{id} enrichment for new items).
    let activities = fetch_activities_chunk(session, start, end, fetched, comes_back)?;
    rows.extend(activities);

    // 2. Health Snapshots (#28 — carries Garmin deviceName when present).
    if let Some(payload) = session.get_json(
        &format!("/wellnessactivity-service/activity/summary/range/{start_str}/{end_str}"),
        &[],
        false,
    )? {
        rows.extend(read_page("health_snapshot", None, &payload));
    }

    // 3. Sleep & Naps (#16 dailySleepSummariesByDate).
    if let Some(payload) = session.get_json(
        "/sleep-service/sleep/dailySleepSummariesByDate",
        &[
            ("startDate", start_str.clone()),
            ("endDate", end_str.clone()),
        ],
        false,
    )? {
        rows.extend(read_page("sleep", None, &payload));
    }

    // 4. Daily Summaries (#22 v2/dailysummaries — with 500 bisection) + smart-gate date sets.
    let (summary_rows, hr_dates, spo2_dates) = fetch_daily_summaries_range(session, start, end)?;
    rows.extend(summary_rows);

    // 5. HRV Daily (#10) -> smart-gate overnight HRV detail (#9).
    let mut hrv_dates = BTreeSet::new();
    if let Some(payload) = session.get_json(
        &format!("/hrv-service/hrv/daily/{start_str}/{end_str}"),
        &[],
        false,
    )? {
        collect_hrv_dates(&payload, &mut hrv_dates);
        rows.extend(read_page("hrv", None, &payload));
    }

    // 6. Body Battery (#25).
    if let Some(payload) = session.get_json(
        "/wellness-service/wellness/bodyBattery/reports/daily",
        &[
            ("startDate", start_str.clone()),
            ("endDate", end_str.clone()),
        ],
        false,
    )? {
        rows.extend(read_page("body_battery", None, &payload));
    }

    // 7. Weight & Body Composition (#24 weight/range?includeAll=true; NEVER #23 weight/all).
    if let Some(payload) = session.get_json(
        &format!("/weight-service/weight/range/{start_str}/{end_str}"),
        &[("includeAll", "true".to_owned())],
        false,
    )? {
        rows.extend(read_page("weight", None, &payload));
    }

    // 8. Hydration (#18).
    if let Some(payload) = session.get_json(
        &format!("/usersummary-service/stats/hydration/daily/{start_str}/{end_str}"),
        &[],
        false,
    )? {
        rows.extend(read_page("hydration", None, &payload));
    }

    // 9. VO2 Max (#11 maxmet/daily).
    if let Some(payload) = session.get_json(
        &format!("/metrics-service/metrics/maxmet/daily/{start_str}/{end_str}"),
        &[],
        false,
    )? {
        rows.extend(read_page("vo2_max", None, &payload));
    }

    // 10. Fitness Age (#7 fitnessage/v2/{end}) — sampled on chunk end date.
    if let Some(payload) = session.get_json(
        &format!("/fitnessage-service/fitnessage/v2/{end_str}"),
        &[],
        false,
    )? {
        rows.extend(read_page("fitness_age", Some(&end_str), &payload));
    }

    // 11. Smart-gated single-day endpoints (#27 dailyHeartRate, #26 spo2, #9 hrv/{date}).
    // On trailing convergence polls, limit re-reads of already-covered days to the last 3 days
    // unless explicit range or backfill chunk.
    let detail_floor = if !explicit_range && end == today && coverage.last_trailing_to.is_some() {
        sub_days(today, TRAILING_DETAIL_DAYS - 1)
    } else {
        start
    };
    for date in hr_dates.into_iter().filter(|d| *d >= detail_floor) {
        let day_str = iso_date(date);
        if let Some(payload) = session.get_json(
            "/wellness-service/wellness/dailyHeartRate",
            &[("date", day_str.clone())],
            false,
        )? {
            rows.extend(read_page("daily_heart_rate", Some(&day_str), &payload));
        }
    }
    for date in spo2_dates.into_iter().filter(|d| *d >= detail_floor) {
        let day_str = iso_date(date);
        if let Some(payload) = session.get_json(
            &format!("/wellness-service/wellness/daily/spo2acclimation/{day_str}"),
            &[],
            false,
        )? {
            rows.extend(read_page("spo2", Some(&day_str), &payload));
        }
    }
    for date in hrv_dates.into_iter().filter(|d| *d >= detail_floor) {
        let day_str = iso_date(date);
        if let Some(payload) =
            session.get_json(&format!("/hrv-service/hrv/{day_str}"), &[], false)?
        {
            rows.extend(read_page("hrv", Some(&day_str), &payload));
        }
    }

    // 12. Women's Health (#13 summary, #14 dayview) — adaptive non-fatal probing.
    let wh_rows = fetch_womens_health_chunk(session, start, end, today, coverage, explicit_range)?;
    rows.extend(wh_rows);

    // 13. Nutrition (#12 food/logs/{date}) — adaptive non-fatal probing.
    let nutrition_rows =
        fetch_nutrition_chunk(session, start, end, today, coverage, explicit_range)?;
    rows.extend(nutrition_rows);

    Ok(rows)
}

fn fetch_activities_chunk(
    session: &mut Session<'_>,
    start: Date,
    end: Date,
    fetched: &Fetched,
    comes_back: bool,
) -> Result<Vec<ReadRow>, Error> {
    let items = fetch_activities_range_bisect(session, start, end, INITIAL_ACTIVITY_LIMIT)?;
    let mut rows = Vec::new();
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(activity_id) = obj.get("activityId").and_then(Value::as_i64) else {
            continue;
        };
        let identity =
            solstone_core_import_sources::garmin::identity("activity", &activity_id.to_string());
        let row_id = solstone_core_import_sources::garmin::row_id(&identity);
        // On ordinary syncs, activities already fetched keep their enriched detail
        // (or stay suppressed if deleted by the owner) without re-fetching detail.
        if !comes_back && fetched.contains(&row_id) {
            continue;
        }
        let merged = match session.get_json(
            &format!("/activity-service/activity/{activity_id}"),
            &[],
            true,
        ) {
            Ok(Some(Value::Object(detail))) => merge_activity_objects(obj, &detail),
            Ok(_) | Err(Error::Unavailable | Error::Response) => Value::Object(obj.clone()),
            Err(error) => return Err(error),
        };
        rows.extend(read_page("activities", None, &merged));
    }
    Ok(rows)
}

fn merge_activity_objects(summary_item: &Map<String, Value>, detail: &Map<String, Value>) -> Value {
    let mut merged = summary_item.clone();
    for (key, value) in detail {
        if !value.is_null() {
            merged.insert(key.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

fn fetch_activities_range_bisect(
    session: &mut Session<'_>,
    start: Date,
    end: Date,
    mut limit: usize,
) -> Result<Vec<Value>, Error> {
    let start_str = iso_date(start);
    let end_str = iso_date(end);
    let mut offset = 0_usize;
    let mut collected = Vec::new();
    for _ in 0..MAX_ACTIVITY_PAGES {
        let query = [
            ("startDate", start_str.clone()),
            ("endDate", end_str.clone()),
            ("start", offset.to_string()),
            ("limit", limit.to_string()),
        ];
        match session.get_json(
            "/activitylist-service/activities/search/activities",
            &query,
            false,
        ) {
            Ok(Some(payload)) => {
                let page_items = extract_activity_array(payload);
                let count = page_items.len();
                collected.extend(page_items);
                if count < limit {
                    return Ok(collected);
                }
                offset += count;
            }
            Ok(None) => return Ok(collected),
            Err(Error::Unavailable) => {
                if limit > 10 {
                    limit = (limit / 2).max(10);
                    continue;
                }
                if start < end {
                    let mid = midpoint_date(start, end);
                    let mut newer = fetch_activities_range_bisect(
                        session,
                        add_days(mid, 1),
                        end,
                        INITIAL_ACTIVITY_LIMIT,
                    )?;
                    let older =
                        fetch_activities_range_bisect(session, start, mid, INITIAL_ACTIVITY_LIMIT)?;
                    newer.extend(older);
                    return Ok(newer);
                }
                log::warn!(
                    "Garmin search/activities returned 500 on single day {start_str} at limit={limit}; skipping page"
                );
                return Ok(collected);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(collected)
}

fn extract_activity_array(payload: Value) -> Vec<Value> {
    match payload {
        Value::Array(items) => items,
        Value::Object(mut map) => map
            .remove("activities")
            .and_then(|v| match v {
                Value::Array(items) => Some(items),
                _ => None,
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

type DailySummariesBatch = (Vec<ReadRow>, BTreeSet<Date>, BTreeSet<Date>);

fn fetch_daily_summaries_range(
    session: &mut Session<'_>,
    start: Date,
    end: Date,
) -> Result<DailySummariesBatch, Error> {
    let start_str = iso_date(start);
    let end_str = iso_date(end);
    match session.get_json(
        "/usersummary-service/v2/dailysummaries",
        &[
            ("startDate", start_str.clone()),
            ("endDate", end_str.clone()),
        ],
        false,
    ) {
        Ok(Some(payload)) => {
            let mut hr_dates = BTreeSet::new();
            let mut spo2_dates = BTreeSet::new();
            collect_summary_signal_dates(&payload, &mut hr_dates, &mut spo2_dates);
            let rows = read_page("daily_summary", None, &payload);
            Ok((rows, hr_dates, spo2_dates))
        }
        Ok(None) => Ok((Vec::new(), BTreeSet::new(), BTreeSet::new())),
        Err(Error::Unavailable) => {
            let span = days_inclusive(start, end);
            if span > 7 {
                let mut all_rows = Vec::new();
                let mut all_hr = BTreeSet::new();
                let mut all_spo2 = BTreeSet::new();
                for (sub_start, sub_end) in split_chunks_newest_first(start, end, 7) {
                    let (rows, hr, spo2) =
                        fetch_daily_summaries_range(session, sub_start, sub_end)?;
                    all_rows.extend(rows);
                    all_hr.extend(hr);
                    all_spo2.extend(spo2);
                }
                Ok((all_rows, all_hr, all_spo2))
            } else if span > 1 {
                let mut all_rows = Vec::new();
                let mut all_hr = BTreeSet::new();
                let mut all_spo2 = BTreeSet::new();
                for day in each_day_descending(start, end) {
                    let (rows, hr, spo2) = fetch_daily_summaries_range(session, day, day)?;
                    all_rows.extend(rows);
                    all_hr.extend(hr);
                    all_spo2.extend(spo2);
                }
                Ok((all_rows, all_hr, all_spo2))
            } else {
                log::warn!(
                    "Garmin v2/dailysummaries returned 500 on single day {start_str}; skipping day"
                );
                Ok((Vec::new(), BTreeSet::new(), BTreeSet::new()))
            }
        }
        Err(error) => Err(error),
    }
}

fn fetch_womens_health_chunk(
    session: &mut Session<'_>,
    start: Date,
    end: Date,
    today: Date,
    coverage: &mut Coverage,
    explicit_range: bool,
) -> Result<Vec<ReadRow>, Error> {
    let should_probe = match &coverage.womens_health {
        ProbeState::Active => true,
        ProbeState::Unknown => true,
        ProbeState::Inactive { last_checked } => {
            explicit_range
                || Date::strptime("%Y-%m-%d", last_checked)
                    .map(|last| days_inclusive(last, today) > WOMENS_HEALTH_REPROBE_DAYS)
                    .unwrap_or(true)
        }
    };
    if !should_probe {
        return Ok(Vec::new());
    }

    let end_str = iso_date(end);
    let summary_rows = match session.get_json(
        &format!("/periodichealth-service/menstrualcycle/summary/{end_str}"),
        &[],
        true,
    )? {
        Some(payload) => read_page("menstrual_cycle_summary", Some(&end_str), &payload),
        None => Vec::new(),
    };

    let mut rows = Vec::new();
    let was_active = coverage.womens_health == ProbeState::Active;
    let mut probed_end_dayview = false;
    if !summary_rows.is_empty() {
        coverage.womens_health = ProbeState::Active;
        rows.extend(summary_rows);
    } else if !was_active {
        // Also check one dayview on `end` in case only daily logs exist without a cycle summary.
        probed_end_dayview = true;
        let probe_day = match session.get_json(
            &format!("/periodichealth-service/menstrualcycle/dayview/{end_str}"),
            &[],
            true,
        )? {
            Some(payload) => read_page("menstrual_cycle_dayview", Some(&end_str), &payload),
            None => Vec::new(),
        };
        if !probe_day.is_empty() {
            coverage.womens_health = ProbeState::Active;
            rows.extend(probe_day);
        } else {
            coverage.womens_health = ProbeState::Inactive {
                last_checked: iso_date(today),
            };
            return Ok(rows);
        }
    }

    // Active Women's Health account: fetch daily cycle/symptom views across the chunk.
    for day in each_day_descending(start, end) {
        let day_str = iso_date(day);
        if day_str == end_str && probed_end_dayview {
            continue;
        }
        if let Some(payload) = session.get_json(
            &format!("/periodichealth-service/menstrualcycle/dayview/{day_str}"),
            &[],
            true,
        )? {
            rows.extend(read_page(
                "menstrual_cycle_dayview",
                Some(&day_str),
                &payload,
            ));
        }
    }
    Ok(rows)
}

fn fetch_nutrition_chunk(
    session: &mut Session<'_>,
    start: Date,
    end: Date,
    today: Date,
    coverage: &mut Coverage,
    explicit_range: bool,
) -> Result<Vec<ReadRow>, Error> {
    let mut rows = Vec::new();
    match &coverage.nutrition {
        ProbeState::Active => {
            for day in each_day_descending(start, end) {
                let day_str = iso_date(day);
                if let Some(payload) = session.get_json(
                    &format!("/nutrition-service/food/logs/{day_str}"),
                    &[],
                    true,
                )? {
                    rows.extend(read_page("nutrition", Some(&day_str), &payload));
                }
            }
        }
        ProbeState::Unknown => {
            // Probe trailing up to 7 days inside this chunk; promote to Active on first hit.
            let probe_start = start.max(sub_days(end, 6));
            let mut found = false;
            for day in each_day_descending(probe_start, end) {
                let day_str = iso_date(day);
                if let Some(payload) = session.get_json(
                    &format!("/nutrition-service/food/logs/{day_str}"),
                    &[],
                    true,
                )? {
                    let day_rows = read_page("nutrition", Some(&day_str), &payload);
                    if !day_rows.is_empty() {
                        found = true;
                        rows.extend(day_rows);
                    }
                }
            }
            if found {
                coverage.nutrition = ProbeState::Active;
                if probe_start > start {
                    for day in each_day_descending(start, sub_days(probe_start, 1)) {
                        let day_str = iso_date(day);
                        if let Some(payload) = session.get_json(
                            &format!("/nutrition-service/food/logs/{day_str}"),
                            &[],
                            true,
                        )? {
                            rows.extend(read_page("nutrition", Some(&day_str), &payload));
                        }
                    }
                }
            } else {
                coverage.nutrition = ProbeState::Inactive {
                    last_checked: iso_date(today),
                };
            }
        }
        ProbeState::Inactive { last_checked } => {
            if explicit_range {
                for day in each_day_descending(start, end) {
                    let day_str = iso_date(day);
                    if let Some(payload) = session.get_json(
                        &format!("/nutrition-service/food/logs/{day_str}"),
                        &[],
                        true,
                    )? {
                        let day_rows = read_page("nutrition", Some(&day_str), &payload);
                        if !day_rows.is_empty() {
                            coverage.nutrition = ProbeState::Active;
                            rows.extend(day_rows);
                        }
                    }
                }
            } else {
                let due = Date::strptime("%Y-%m-%d", last_checked)
                    .map(|last| days_inclusive(last, today) > NUTRITION_REPROBE_DAYS)
                    .unwrap_or(true);
                if due && end == today {
                    let day_str = iso_date(today);
                    if let Some(payload) = session.get_json(
                        &format!("/nutrition-service/food/logs/{day_str}"),
                        &[],
                        true,
                    )? {
                        let day_rows = read_page("nutrition", Some(&day_str), &payload);
                        if !day_rows.is_empty() {
                            coverage.nutrition = ProbeState::Active;
                            rows.extend(day_rows);
                        } else {
                            coverage.nutrition = ProbeState::Inactive {
                                last_checked: day_str,
                            };
                        }
                    } else {
                        coverage.nutrition = ProbeState::Inactive {
                            last_checked: day_str,
                        };
                    }
                }
            }
        }
    }
    Ok(rows)
}

fn collect_summary_signal_dates(
    payload: &Value,
    hr_dates: &mut BTreeSet<Date>,
    spo2_dates: &mut BTreeSet<Date>,
) {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => {
            if let Some(list) = obj
                .get("data")
                .or_else(|| obj.get("dailySummaries"))
                .or_else(|| obj.get("summaries"))
                .and_then(Value::as_array)
            {
                list.iter().filter_map(Value::as_object).collect()
            } else {
                vec![obj]
            }
        }
        _ => Vec::new(),
    };
    for item in items {
        let Some(date) = item
            .get("calendarDate")
            .or_else(|| item.get("summaryDate"))
            .and_then(Value::as_str)
            .and_then(|s| Date::strptime("%Y-%m-%d", s.get(..10)?).ok())
        else {
            continue;
        };
        let flat_hr = [
            "minHeartRate",
            "maxHeartRate",
            "restingHeartRate",
            "lastSevenDaysAvgRestingHeartRate",
            "averageHeartRate",
        ]
        .iter()
        .any(|k| {
            item.get(*k)
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
        });
        let nested_hr = item
            .get("heartRate")
            .and_then(Value::as_object)
            .is_some_and(|hr| hr.values().any(|v| v.as_f64().is_some_and(|n| n > 0.0)));
        if flat_hr || nested_hr {
            hr_dates.insert(date);
        }
        let flat_spo2 = ["averageSpo2", "lowestSpo2", "latestSpo2"].iter().any(|k| {
            item.get(*k)
                .and_then(Value::as_f64)
                .is_some_and(|v| v > 0.0)
        });
        let nested_spo2 = item
            .get("pulseOx")
            .and_then(Value::as_object)
            .is_some_and(|po| {
                po.iter().any(|(k, v)| {
                    k.to_ascii_lowercase().contains("spo2") && v.as_f64().is_some_and(|n| n > 0.0)
                })
            });
        if flat_spo2 || nested_spo2 {
            spo2_dates.insert(date);
        }
    }
}

fn collect_hrv_dates(payload: &Value, hrv_dates: &mut BTreeSet<Date>) {
    let items: Vec<&Map<String, Value>> = match payload {
        Value::Array(list) => list.iter().filter_map(Value::as_object).collect(),
        Value::Object(obj) => {
            if let Some(list) = obj.get("hrvSummaries").and_then(Value::as_array) {
                list.iter().filter_map(Value::as_object).collect()
            } else if let Some(summary) = obj.get("hrvSummary").and_then(Value::as_object) {
                vec![summary]
            } else {
                vec![obj]
            }
        }
        _ => Vec::new(),
    };
    for item in items {
        if let Some(date) = item
            .get("calendarDate")
            .and_then(Value::as_str)
            .and_then(|s| Date::strptime("%Y-%m-%d", s.get(..10)?).ok())
        {
            hrv_dates.insert(date);
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "final publication step wiring placement, facet, fetched dedup, and ledger update"
)]
fn persist_run(
    journal: &Path,
    zone: &TimeZone,
    now: Timestamp,
    timestamp: &str,
    facet: Option<&str>,
    listed: ListedMoves,
    comes_back: bool,
    read_rows: &[ReadRow],
    fetched: &mut Fetched,
    backfill: BackfillState,
) -> Result<GarminSyncOutcome, Error> {
    let now_iso = now.to_string();
    let placed = place_rows_by_day(journal, zone, read_rows).map_err(|_| Error::Storage)?;
    let move_listed = match (facet, listed) {
        (Some(_), ListedMoves::Move) => true,
        (Some(facet_name), ListedMoves::MoveAtMost(shown)) => {
            let (count, _) = crate::body_activity::listed_elsewhere(
                journal,
                GARMIN.records().stream,
                facet_name,
            );
            count <= shown
        }
        _ => false,
    };

    // Filter out owner-deleted rows on ordinary syncs before checking whether anything changes.
    let mut prepared_days: Vec<(String, DayRows)> = Vec::new();
    let mut any_change = false;
    for (day, windows) in placed {
        let (windows, _, _) = settle_readings_noting(journal, &GARMIN, &day, zone, windows)
            .map_err(|_| Error::Storage)?;
        let filtered = if comes_back {
            windows
        } else {
            filter_deleted_rows(journal, &day, windows, fetched)?
        };
        if filtered.is_empty() {
            continue;
        }
        let filed = DayPlacement::load(journal, &GARMIN, &day)
            .map_err(|_| Error::Storage)?
            .coming_back(comes_back)
            .file_for_import(filtered.clone(), facet, move_listed, None)
            .map_err(|_| Error::Storage)?;
        for (window, rows) in filed.windows {
            let filing = filed.filings.get(&window.1).map(String::as_str);
            let decision = decide_filing(
                journal,
                &GARMIN,
                &window,
                rows,
                filing,
                move_listed,
                None,
                comes_back,
            )
            .map_err(|_| Error::Storage)?;
            if !decision.absent
                && !decision.stayed_deleted
                && !decision.unreadable
                && (decision.contents.is_some() || decision.facet_changed || decision.publish)
            {
                any_change = true;
            }
        }
        prepared_days.push((day, filtered));
    }

    // Quiet poll: zero rows/windows changed -> allocate NO `imports/<id>/` directory.
    if !any_change {
        garmin::record_sync_success(journal, &now_iso, None, backfill.clone())?;
        return Ok(GarminSyncOutcome {
            import_id: None,
            rows_written: 0,
            windows_published: 0,
            workouts_written: 0,
            days_touched: Vec::new(),
            backfill,
            paused_for_key: false,
            summary: "Garmin Connect is up to date; no new entries.".to_owned(),
        });
    }

    // Allocate one import run directory now that we know data is actually landing.
    let requested =
        solstone_core_import::validate_timestamp(timestamp).map_err(|_| Error::Storage)?;
    let import_id =
        solstone_core_import::bind_import_record_where(journal, &requested, None, &|existing| {
            !holds_a_run(journal, existing)
        })
        .map_err(|_| Error::Storage)?
        .import_id
        .as_str()
        .to_owned();
    let generation = solstone_core_import::admit_running_attempt(
        journal,
        &import_id,
        now_ms(),
        Some(GARMIN.source),
    )
    .map_err(|_| Error::Storage)?
    .generation;

    let mut total_rows_written = 0_u64;
    let mut total_windows_published = 0_u64;
    let mut total_workouts_written = 0_u64;
    let mut touched_days: BTreeSet<String> = BTreeSet::new();
    let mut listing_len = 0_u64;
    let listing_path = journal.join("imports").join(&import_id).join(LISTING_FILE);

    for (day, windows) in prepared_days {
        let filed = DayPlacement::load(journal, &GARMIN, &day)
            .map_err(|_| Error::Storage)?
            .coming_back(comes_back)
            .file_for_import(windows, facet, move_listed, Some(&import_id))
            .map_err(|_| Error::Storage)?;

        let mut publish_windows: Vec<WindowId> = Vec::new();
        let mut day_listing: Vec<(String, u64, u64, u64)> = Vec::new();
        let mut held_now: Vec<String> = Vec::new();
        let mut day_had_write = false;

        for (window, rows) in filed.windows {
            let filing = filed.filings.get(&window.1).map(String::as_str);
            let arriving_ids: Vec<String> = rows
                .values()
                .filter(|r| r.kind != RowKind::Moved)
                .map(|r| r.id.clone())
                .collect();
            let incoming_workouts: BTreeSet<String> = rows
                .values()
                .filter(|r| {
                    r.kind == RowKind::Experience && r.experience.as_deref() == Some("workout")
                })
                .map(|r| r.id.clone())
                .collect();
            let existing_ids: BTreeSet<String> = stored_rows(journal, &GARMIN, &window)
                .ok()
                .flatten()
                .map(|(_, stored)| stored.into_iter().map(|r| r.id).collect())
                .unwrap_or_default();

            let decision = decide_filing(
                journal,
                &GARMIN,
                &window,
                rows,
                filing,
                move_listed,
                Some(&import_id),
                comes_back,
            )
            .map_err(|_| Error::Storage)?;

            if decision.absent || decision.stayed_deleted || decision.unreadable {
                continue;
            }
            held_now.extend(arriving_ids);

            if decision.contents.is_some() || decision.facet_changed || decision.publish {
                let win_dir = journal
                    .join("chronicle")
                    .join(&window.0)
                    .join(GARMIN.stream)
                    .join(&window.1);
                if let Some(contents) = decision.contents {
                    save::write_body_window_file(
                        journal,
                        GARMIN.stream,
                        &window.0,
                        &window.1,
                        BODY_RECORDS_FILE,
                        &contents,
                        comes_back,
                    )
                    .map_err(|_| Error::Storage)?;
                    day_had_write = true;
                }
                if decision.facet_changed
                    && let Some(filing_text) = filing
                    && win_dir.join(BODY_RECORDS_FILE).is_file()
                {
                    let facet_path = win_dir.join("talents/facets.json");
                    if let Some(parent) = facet_path.parent() {
                        fs::create_dir_all(parent).map_err(|_| Error::Storage)?;
                    }
                    atomic_replace(&facet_path, filing_text.as_bytes(), PRIVATE)
                        .map_err(|_| Error::Storage)?;
                }
                if decision.publish {
                    publish_windows.push(window.clone());
                }
                total_rows_written += decision.new_rows + decision.updated_rows;
                total_workouts_written += incoming_workouts
                    .iter()
                    .filter(|id| !existing_ids.contains(*id))
                    .count() as u64;
                day_listing.push((
                    window.1.clone(),
                    decision.new_rows,
                    decision.updated_rows,
                    decision.unchanged_rows,
                ));
            }
        }

        if !publish_windows.is_empty() {
            let created: Vec<TextCreated> = publish_windows
                .iter()
                .map(|window| TextCreated {
                    day: window.0.clone(),
                    segment: window.1.clone(),
                    stream: GARMIN.stream.to_owned(),
                    hints: save::named_stream_hints(GARMIN.source),
                    path: journal
                        .join("chronicle")
                        .join(&window.0)
                        .join(GARMIN.stream)
                        .join(&window.1)
                        .join(BODY_RECORDS_FILE),
                })
                .collect();
            let segments: Vec<CreatedSegment> =
                created.iter().map(TextCreated::created_segment).collect();
            let files: Vec<PathBuf> = created.iter().map(|c| c.path.clone()).collect();
            let record = publish_with_operations(
                PublicationInput {
                    journal,
                    import_dir: None,
                    import_id: &import_id,
                    importer: GARMIN.source,
                    revision: None,
                    segments: &segments,
                    files_created: &files,
                    index_only: &[],
                    may_write_record: None,
                },
                &NativePublicationOperations,
            )
            .map_err(|_| Error::Storage)?;
            if record.status == PublicationStatus::Success {
                total_windows_published += publish_windows.len() as u64;
            }
            touched_days.insert(day.clone());
        }

        if day_had_write || !day_listing.is_empty() {
            let entry = json!({
                "day": day,
                "windows": day_listing,
            });
            listing_len = solstone_core_journal_io::append_jsonl_at_checkpoint(
                &listing_path,
                listing_len,
                &entry,
            )
            .map_err(|_| Error::Storage)?;
            let _ =
                crate::body_activity::write_records_at(journal, &GARMIN, &day, now, move_listed);
        }
        if !held_now.is_empty() {
            fetched
                .record(journal, &day, held_now.iter().map(String::as_str))
                .map_err(|_| Error::Storage)?;
        }
    }

    let days_vec: Vec<String> = touched_days.iter().cloned().collect();
    let days_span = days_vec
        .first()
        .zip(days_vec.last())
        .map(|(first, last)| [first.clone(), last.clone()]);

    let summary_text = format!(
        "# Garmin Connect sync\n\nSaved {total_rows_written} entries across {} day(s) ({total_windows_published} window(s)).\n",
        days_vec.len()
    );
    let summary_path = journal.join("imports").join(&import_id).join("summary.md");
    atomic_replace(&summary_path, summary_text.as_bytes(), PRIVATE).map_err(|_| Error::Storage)?;

    solstone_core_import::write_manifest(&solstone_core_import::ManifestWriteRequest {
        journal_root: journal,
        import_id: &import_id,
        source_type: GARMIN.source,
        source_hash: &solstone_core_import::SourceHash::new(String::new()),
        entry_count: total_rows_written,
        days_affected: &days_vec,
        files_created: &[],
        imported_via: "native",
        link_id: None,
        observer_handle: None,
        raw_retention: Some("discard"),
        span: days_span.as_ref().map(std::slice::from_ref),
    })
    .map_err(|_| Error::Storage)?;

    let _ = finish_import_attempt_indexing(
        journal,
        &import_id,
        generation,
        GARMIN.source,
        ImportTerminalInput::Success(&[]),
        &[summary_path],
    );

    let run_record = RunRecord {
        id: import_id.clone(),
        imported_at: Some(now_iso.clone()),
        days: days_span,
        days_count: days_vec.len(),
        rows: total_rows_written.max(total_windows_published),
        workouts: total_workouts_written,
    };
    garmin::record_sync_success(journal, &now_iso, Some(run_record), backfill.clone())?;

    Ok(GarminSyncOutcome {
        import_id: Some(import_id),
        rows_written: total_rows_written,
        windows_published: total_windows_published,
        workouts_written: total_workouts_written,
        days_touched: days_vec,
        backfill,
        paused_for_key: false,
        summary: summary_text,
    })
}

/// Remove rows that an earlier Garmin sync brought in and the owner has since
/// deleted (via per-run Trash or window delete) when running an ordinary sync.
fn filter_deleted_rows(
    journal: &Path,
    day: &str,
    windows: DayRows,
    fetched: &Fetched,
) -> Result<DayRows, Error> {
    let mut day_held = BTreeSet::new();
    for stored in stored_windows_of_day(journal, &GARMIN, day).map_err(|_| Error::Storage)? {
        if let Ok(Some((_, rows))) = stored_rows(journal, &GARMIN, &stored) {
            day_held.extend(rows.into_iter().map(|r| r.id));
        }
    }
    let mut kept = Vec::with_capacity(windows.len());
    for (window, mut rows) in windows {
        rows.retain(|_, row| day_held.contains(&row.id) || !fetched.contains(&row.id));
        if !rows.is_empty() {
            kept.push((window, rows));
        }
    }
    Ok(kept)
}

fn update_backfill_progress_only(journal: &Path, backfill: BackfillState) {
    if let Ok(mut state) = garmin::read_state(journal) {
        state.backfill = backfill;
        let _ = garmin::save_state(journal, &state);
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |idx| idx + 1);
    &bytes[start..end]
}

fn history_floor() -> Date {
    Date::new(HISTORY_FLOOR.0, HISTORY_FLOOR.1, HISTORY_FLOOR.2).expect("valid floor date")
}

fn clamp_floor(date: Date) -> Date {
    date.max(history_floor())
}

fn iso_date(date: Date) -> String {
    date.strftime("%Y-%m-%d").to_string()
}

fn add_days(date: Date, days: i64) -> Date {
    date.checked_add(Span::new().days(days)).unwrap_or(date)
}

fn sub_days(date: Date, days: i64) -> Date {
    date.checked_sub(Span::new().days(days)).unwrap_or(date)
}

fn days_inclusive(start: Date, end: Date) -> i64 {
    if end < start {
        return 0;
    }
    start
        .until(end)
        .map(|span| i64::from(span.get_days()) + 1)
        .unwrap_or(1)
}

fn midpoint_date(start: Date, end: Date) -> Date {
    let span = days_inclusive(start, end);
    add_days(start, (span / 2).saturating_sub(1))
}

fn split_chunks_newest_first(start: Date, end: Date, chunk_days: i64) -> Vec<(Date, Date)> {
    let mut chunks = Vec::new();
    let mut cursor = end;
    while cursor >= start {
        let chunk_start = sub_days(cursor, chunk_days - 1).max(start);
        chunks.push((chunk_start, cursor));
        if chunk_start <= start {
            break;
        }
        cursor = sub_days(chunk_start, 1);
    }
    chunks
}

fn each_day_descending(start: Date, end: Date) -> Vec<Date> {
    let mut days = Vec::new();
    let mut cursor = end;
    while cursor >= start {
        days.push(cursor);
        if cursor <= start {
            break;
        }
        cursor = sub_days(cursor, 1);
    }
    days
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use solstone_core_import::garmin::RawResponse;
    use solstone_core_import::import_trash::Imports;
    use solstone_core_indexer_store::RetentionIndex;
    use solstone_core_removals::{Action, Page, Surface};

    fn make_jwt(iat: i64, exp: i64) -> String {
        fn b64(bytes: &[u8]) -> String {
            const TABLE: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let b0 = u32::from(chunk[0]);
                let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
                let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
                let triple = (b0 << 16) | (b1 << 8) | b2;
                out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
                out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
                if chunk.len() > 1 {
                    out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
                }
                if chunk.len() > 2 {
                    out.push(TABLE[(triple & 0x3f) as usize] as char);
                }
            }
            out
        }
        let h = b64(br#"{"alg":"RS256","typ":"JWT"}"#);
        let p = b64(format!(r#"{{"iat":{iat},"exp":{exp}}}"#).as_bytes());
        format!("{h}.{p}.sig")
    }

    fn seed_journal(root: &Path, key: &str) {
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("chronicle")).unwrap();
        fs::create_dir_all(root.join("imports")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"America/Denver"}}"#,
        )
        .unwrap();
        garmin::set_key(root, Some(key), None).unwrap();
    }

    type Responder = Box<dyn FnMut(&str, &[(&str, String)]) -> Result<RawResponse, Error>>;

    struct MockHttp {
        token_responses: VecDeque<Result<RawResponse, Error>>,
        token_calls: usize,
        get_calls: Vec<(String, Vec<(String, String)>)>,
        responder: Responder,
    }

    impl GarminHttp for MockHttp {
        fn exchange_token(&mut self, _api_key: &str) -> Result<RawResponse, Error> {
            self.token_calls += 1;
            self.token_responses.pop_front().unwrap_or_else(|| {
                Ok(RawResponse {
                    status: 200,
                    body: make_jwt(1_800_000_000, 1_800_086_400).into_bytes(),
                    retry_after_secs: None,
                })
            })
        }

        fn get_json(
            &mut self,
            _bearer: &str,
            path: &str,
            query: &[(&str, String)],
        ) -> Result<RawResponse, Error> {
            self.get_calls.push((
                path.to_owned(),
                query
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), v.clone()))
                    .collect(),
            ));
            (self.responder)(path, query)
        }
    }

    fn ok_json(val: Value) -> Result<RawResponse, Error> {
        Ok(RawResponse {
            status: 200,
            body: serde_json::to_vec(&val).unwrap(),
            retry_after_secs: None,
        })
    }

    fn status_resp(status: u16) -> Result<RawResponse, Error> {
        Ok(RawResponse {
            status,
            body: Vec::new(),
            retry_after_secs: None,
        })
    }

    #[test]
    fn full_range_sync_handles_quirks_quiet_poll_and_per_run_trash_suppression_and_reclaim() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_journal(root, "GCDC_test_owner_key");

        let fixed_now = Timestamp::from_second(1_795_540_800).unwrap(); // 2026-11-24T17:20:00Z
        let clock = || fixed_now;
        let sleep_fn = |_: Duration| {};

        let mut summary_500_once = true;
        let mut activity_500_once = true;
        let mut hrv_429_once = true;

        let mut http = MockHttp {
            token_responses: [Ok(RawResponse {
                status: 200,
                body: make_jwt(fixed_now.as_second(), fixed_now.as_second() + 86_400).into_bytes(),
                retry_after_secs: None,
            })]
            .into(),
            token_calls: 0,
            get_calls: Vec::new(),
            responder: Box::new(move |path, query| {
                if path == "/activitylist-service/activities/search/activities" {
                    let limit = query
                        .iter()
                        .find(|(k, _)| *k == "limit")
                        .map(|(_, v)| v.as_str())
                        .unwrap_or("");
                    if activity_500_once && limit == "40" {
                        activity_500_once = false;
                        return status_resp(500);
                    }
                    return ok_json(json!([{
                        "activityId": 40_616_954_664_i64,
                        "activityName": "Morning Ridge Run",
                        "activityType": {"typeKey": "trail_running"},
                        "startTimeLocal": "2026-11-24 07:00:00",
                        "startTimeGMT": "2026-11-24 14:00:00",
                        "duration": 1800.0,
                        "distance": 5200.0,
                        "calories": 420.0,
                        "averageHR": 154.0,
                        "startLatitude": 40.015,
                        "startLongitude": -105.2705,
                        "locationName": "Boulder"
                    }]));
                }
                if path == "/activity-service/activity/40616954664" {
                    return ok_json(json!({
                        "activityId": 40_616_954_664_i64,
                        "description": "Crisp air on the mesa."
                    }));
                }
                if path.starts_with("/sleep-service/sleep/dailySleepSummariesByDate") {
                    return ok_json(json!([
                        {"dailySleepDTO": {"id": null, "calendarDate": "2026-11-23"}},
                        {
                            "dailySleepDTO": {
                                "id": 99887766_i64,
                                "calendarDate": "2026-11-24",
                                "sleepStartTimestampGMT": 1_795_496_400_000_i64,
                                "sleepStartTimestampLocal": 1_795_471_200_000_i64,
                                "sleepEndTimestampGMT": 1_795_521_600_000_i64,
                                "sleepEndTimestampLocal": 1_795_496_400_000_i64,
                                "sleepTimeSeconds": 25200,
                                "deepSleepSeconds": 5400,
                                "sleepScores": {"overall": {"value": 84}}
                            }
                        }
                    ]));
                }
                if path == "/usersummary-service/v2/dailysummaries" {
                    if summary_500_once {
                        summary_500_once = false;
                        return status_resp(500);
                    }
                    return ok_json(json!([{
                        "calendarDate": "2026-11-24",
                        "totalSteps": 9420,
                        "totalDistanceMeters": 7250.0,
                        "restingHeartRate": 51,
                        "minHeartRate": 47,
                        "maxHeartRate": 168,
                        "averageSpo2": 97.0,
                        "averageStressLevel": -1
                    }]));
                }
                if path.starts_with("/hrv-service/hrv/daily/") {
                    if hrv_429_once {
                        hrv_429_once = false;
                        return Ok(RawResponse {
                            status: 429,
                            body: Vec::new(),
                            retry_after_secs: Some(1),
                        });
                    }
                    return ok_json(json!({
                        "hrvSummaries": [{
                            "calendarDate": "2026-11-24",
                            "lastNightAvg": 64,
                            "status": "BALANCED"
                        }]
                    }));
                }
                if path == "/weight-service/weight/range/2026-11-23/2026-11-24" {
                    return ok_json(json!({
                        "dailyWeightSummaries": [{
                            "summaryDate": "2026-11-24",
                            "allWeightMetrics": [{
                                "samplePk": 12345_i64,
                                "timestampGMT": 1_795_530_000_000_i64,
                                "weight": 74276.7,
                                "bmi": 22.8
                            }]
                        }]
                    }));
                }
                if path.starts_with("/periodichealth-service/menstrualcycle/summary/") {
                    // Unconfigured account returns 403 -> non-fatal!
                    return status_resp(403);
                }
                status_resp(204)
            }),
        };

        let d1 = Date::new(2026, 11, 23).unwrap();
        let d2 = Date::new(2026, 11, 24).unwrap();
        let mut req = GarminSyncRequest::new("20261124_102000", None, &mut http);
        req.range = Some((d1, d2));
        req.clock = Some(&clock);
        req.sleep_fn = Some(&sleep_fn);

        let first = sync_garmin(root, req).unwrap();
        let run_id = first.import_id.clone().expect("run allocated");
        assert!(first.rows_written > 0);
        assert_eq!(first.workouts_written, 1);
        assert!(
            root.join("imports")
                .join(&run_id)
                .join("body-touched.jsonl")
                .is_file()
        );
        assert!(
            root.join("imports")
                .join(&run_id)
                .join("manifest.json")
                .is_file()
        );
        // Never called broken #23 weight/all.
        assert!(
            !http
                .get_calls
                .iter()
                .any(|(p, _)| p.contains("/weight/all/"))
        );

        // Second sync with identical data: quiet poll allocates NO new imports/<id>/ directory!
        let mut req2 = GarminSyncRequest::new("20261124_112000", None, &mut http);
        req2.range = Some((d1, d2));
        req2.scheduled = true;
        req2.clock = Some(&clock);
        req2.sleep_fn = Some(&sleep_fn);
        let second = sync_garmin(root, req2).unwrap();
        assert_eq!(second.import_id, None);
        assert_eq!(second.rows_written, 0);
        assert!(!root.join("imports/20261124_112000").exists());

        // Trash the first run via shared Import Trash machinery.
        let index = RetentionIndex::new(root);
        let made = solstone_core_import::made_from::MadeFromIt::new();
        let trashed = Imports::new(&index, &made)
            .trash(
                root,
                &run_id,
                Action::ImportDelete,
                Surface::of_request(Page::Settings, None),
            )
            .unwrap();
        assert!(!root.join("imports").join(&run_id).exists());
        let st_after_trash = garmin::status(root, false, true).unwrap();
        assert_eq!(st_after_trash.runs.len(), 0);
        assert_eq!(st_after_trash.workouts, 0);

        // Ordinary scheduled sync MUST NOT bring back trashed rows (recorded in .garmin-fetched.jsonl).
        let mut req3 = GarminSyncRequest::new("20261124_122000", None, &mut http);
        req3.range = Some((d1, d2));
        req3.scheduled = true;
        req3.clock = Some(&clock);
        req3.sleep_fn = Some(&sleep_fn);
        let third = sync_garmin(root, req3).unwrap();
        assert_eq!(third.import_id, None);
        assert_eq!(third.rows_written, 0);

        // Explicit range backfill (`scheduled = false`) DOES reclaim them!
        let mut req4 = GarminSyncRequest::new("20261124_132000", None, &mut http);
        req4.range = Some((d1, d2));
        req4.scheduled = false;
        req4.clock = Some(&clock);
        req4.sleep_fn = Some(&sleep_fn);
        let fourth = sync_garmin(root, req4).unwrap();
        assert!(fourth.import_id.is_some());
        assert!(fourth.rows_written > 0);
        assert_eq!(fourth.workouts_written, 1);

        let _ = trashed;
    }

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn loopback_live_http_accepts_text_plain_jwt_retries_401_once_and_pauses_on_dead_key() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_journal(root, "GCDC_loopback_key");

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let calls_bg = Arc::clone(&calls);

        let jwt1 = make_jwt(1_800_000_000, 1_800_086_400);
        let jwt1_bg = jwt1.clone();
        let server = std::thread::spawn(move || {
            let mut token_count = 0_u32;
            for stream in listener.incoming().take(3) {
                let mut stream = stream.unwrap();
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("").to_owned();
                calls_bg.lock().unwrap().push(first_line.clone());

                if first_line.starts_with("POST /personalapitoken-service/") {
                    // Enforce Garmin's Accept: */* trap (406 if application/json sent).
                    assert!(
                        req.to_ascii_lowercase().contains("accept: */*"),
                        "token endpoint must send Accept: */*, got:\n{req}"
                    );
                    token_count += 1;
                    if token_count == 1 {
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            jwt1_bg.len(),
                            jwt1_bg
                        );
                        stream.write_all(resp.as_bytes()).unwrap();
                    } else {
                        // Second token exchange returns 401 -> key expired/revoked!
                        stream
                            .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .unwrap();
                    }
                } else {
                    // First data GET returns 401 -> triggers single forced token refresh!
                    stream
                        .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .unwrap();
                }
            }
        });

        let mut http = LiveGarminHttp::with_loopback_origin_for_tests(
            format!("http://{addr}"),
            None,
            Duration::ZERO,
        );
        let fixed_now = Timestamp::from_second(1_800_000_100).unwrap();
        let clock = || fixed_now;
        let sleep_fn = |_: Duration| {};
        let d = Date::new(2026, 11, 24).unwrap();
        let mut req = GarminSyncRequest::new("20261124_100000", None, &mut http);
        req.range = Some((d, d));
        req.scheduled = true;
        req.clock = Some(&clock);
        req.sleep_fn = Some(&sleep_fn);

        let err = sync_garmin(root, req).unwrap_err();
        assert_eq!(err, Error::Key);
        server.join().unwrap();

        assert!(garmin::paused_for_key_error(root));
        assert!(garmin::read_token_cache(root).is_none());
        let recorded = calls.lock().unwrap();
        assert_eq!(
            recorded.len(),
            3,
            "expected token -> 401 GET -> 401 refresh: {recorded:?}"
        );

        // Next scheduled check pauses immediately without making any HTTP call.
        let mut dummy = LiveGarminHttp::with_loopback_origin_for_tests(
            "http://127.0.0.1:1".to_owned(),
            None,
            Duration::ZERO,
        );
        let mut req_paused = GarminSyncRequest::new("20261124_110000", None, &mut dummy);
        req_paused.scheduled = true;
        req_paused.clock = Some(&clock);
        let paused = sync_garmin(root, req_paused).unwrap();
        assert!(paused.paused_for_key);
        assert_eq!(paused.import_id, None);
    }

    #[test]
    fn active_womens_health_and_nutrition_ingest_whole_and_backfill_stops_after_empty_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_journal(root, "GCDC_active_wh_nutr");

        let fixed_now = Timestamp::from_second(1_795_540_800).unwrap(); // 2026-11-24T17:20:00Z
        let clock = || fixed_now;
        let sleep_fn = |_: Duration| {};

        let mut http = MockHttp {
            token_responses: [Ok(RawResponse {
                status: 200,
                body: make_jwt(fixed_now.as_second(), fixed_now.as_second() + 86_400).into_bytes(),
                retry_after_secs: None,
            })]
            .into(),
            token_calls: 0,
            get_calls: Vec::new(),
            responder: Box::new(move |path, _query| {
                if path == "/periodichealth-service/menstrualcycle/summary/2026-11-24" {
                    return ok_json(json!({
                        "calendarDate": "2026-11-24",
                        "cycleLength": 28,
                        "currentPhase": "FOLLICULAR"
                    }));
                }
                if path == "/periodichealth-service/menstrualcycle/dayview/2026-11-24" {
                    return ok_json(json!({
                        "calendarDate": "2026-11-24",
                        "flow": "LIGHT",
                        "symptoms": ["FATIGUE"]
                    }));
                }
                if path == "/nutrition-service/food/logs/2026-11-24" {
                    return ok_json(json!({
                        "foodLogs": [{
                            "foodLogId": "meal-1",
                            "calendarDate": "2026-11-24",
                            "loggedTimeLocal": "2026-11-24 08:15:00",
                            "loggedTimeGMT": "2026-11-24 15:15:00",
                            "foodName": "Oatmeal with berries",
                            "calories": 380
                        }]
                    }));
                }
                status_resp(204)
            }),
        };

        let mut req = GarminSyncRequest::new("20261124_102000", None, &mut http);
        req.clock = Some(&clock);
        req.sleep_fn = Some(&sleep_fn);

        let outcome = sync_garmin(root, req).unwrap();
        assert!(outcome.import_id.is_some());
        assert!(outcome.rows_written >= 3);
        // Trailing 7d had rows; the 6 backward 31d chunks were empty -> backfill completes cleanly!
        assert!(outcome.backfill.complete);
        assert!(outcome.backfill.reached_day.is_some());

        // Verify Women's Health & Nutrition rows landed intact in import.garmin on 20261124.
        let windows = stored_windows_of_day(root, &GARMIN, "20261124").unwrap();
        let all_rows: Vec<solstone_core_format::content::body_records::BodyRow> = windows
            .iter()
            .filter_map(|w| stored_rows(root, &GARMIN, w).ok().flatten())
            .flat_map(|(_, rows)| rows)
            .collect();
        assert!(
            all_rows
                .iter()
                .any(solstone_core_format::content::body_records::is_womens_health_row),
            "expected Women's Health rows in owner journal: {all_rows:?}"
        );
        assert!(
            all_rows
                .iter()
                .any(|r| r.record_type == "nutrition"
                    && r.words.iter().any(|w| w.contains("Oatmeal"))),
            "expected Nutrition row in owner journal: {all_rows:?}"
        );
    }
}
