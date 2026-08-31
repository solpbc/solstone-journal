// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native trends folding deliberately matches the Python shard-only fold.
//!
//! The reference behaviour invalidates the trends cache from only the
//! `health-dedupe.sqlite` database and WAL signature. Native follows that
//! behaviour, so changing normalized shards without changing that database
//! intentionally leaves a stale payload. The shared signature is also the
//! day-page baseline lookup's compatibility seam, so widening it here would
//! diverge from the reference. A later wave must record this divergence in the
//! corpus generator's `native_deviations`; this wave leaves the frozen fixture
//! unchanged.
//!
//! One Convey process serves one journal root, so its warm flight is deliberately
//! process-global rather than per-journal. The signature is captured before the
//! fold: an import landing during the fold then makes the completed payload stale
//! and causes the next request to retry.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime};
use serde_json::{Value, json};
use solstone_core_body_source::{BodyValue, FieldState, ValueState};

use crate::{
    DaySleep, NormalizedRow, SLEEP_SESSION_GAP_MINUTES, ShardReadError, TrendsSignature,
    coverage_month_keys, health_dedupe_database_path, pick_day_sleep, read_normalized_rows,
    trends_signature,
};

type TrendsCache = BTreeMap<String, (TrendsSignature, Arc<TrendsPayload>)>;
type SleepRows = BTreeMap<String, Vec<(String, NaiveDateTime, NaiveDateTime, Option<String>)>>;
static TRENDS_CACHE: OnceLock<Mutex<TrendsCache>> = OnceLock::new();
static TRENDS_WARM_FLIGHT: AtomicBool = AtomicBool::new(false);

thread_local! {
    static TRENDS_WARM_INVOCATIONS: Cell<u64> = const { Cell::new(0) };
}

pub const TYPICAL_BASELINE_DAYS: i64 = 90;
pub const TYPICAL_MIN_VALUES: usize = 14;
const TYPICAL_SIGNAL_KEYS: [&str; 4] = ["readiness", "sleep_score", "asleep_minutes", "resting_hr"];
const TREND_ANNOTATION_LIMIT: usize = 6;
const TREND_SIGNALS: [(&str, &str, &str); 10] = [
    ("resting_hr", "Resting heart rate", "bpm"),
    ("vascular_age", "Vascular age", ""),
    ("asleep_minutes", "Asleep", "h"),
    ("sleep_score", "Sleep score", ""),
    ("readiness", "Readiness", ""),
    ("temp_deviation", "Temperature deviation", "°C"),
    ("stress_high_minutes", "Daytime stress high", "h"),
    ("steps", "Steps", "steps"),
    ("body_mass", "Body mass", "lb"),
    ("glucose_avg", "Glucose average", "mg/dL"),
];

#[derive(Debug, Clone, PartialEq)]
pub enum TrendValue {
    Integer(i64),
    Real(f64),
}

impl TrendValue {
    fn as_f64(&self) -> f64 {
        match self {
            Self::Integer(value) => *value as f64,
            Self::Real(value) => *value,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrendsPayload {
    pub signals: Vec<TrendSignal>,
    pub annotations: Vec<TrendAnnotation>,
    pub generated_at_day: String,
}
#[derive(Debug, Clone, PartialEq)]
pub struct TrendSignal {
    pub key: String,
    pub label: String,
    pub unit_label: String,
    pub daily: Vec<(String, TrendValue)>,
    pub coverage: TrendCoverage,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrendCoverage {
    pub first_day: String,
    pub last_day: String,
    pub days: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrendAnnotation {
    pub day: String,
    pub label: String,
}

#[derive(Debug)]
pub enum TrendsCacheError {
    CachePoisoned,
}
impl std::fmt::Display for TrendsCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("trends cache lock was poisoned")
    }
}
impl std::error::Error for TrendsCacheError {}

#[derive(Debug)]
pub enum TrendsFoldError {
    Signature(crate::DatabaseSignatureError),
    Shards(ShardReadError),
}
impl std::fmt::Display for TrendsFoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Signature(error) => write!(f, "trends signature failed: {error}"),
            Self::Shards(error) => write!(f, "trends shard fold failed: {error}"),
        }
    }
}
impl std::error::Error for TrendsFoldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Signature(error) => Some(error),
            Self::Shards(error) => Some(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrendsWarmOutcome {
    Cached,
    Succeeded,
    Failed,
    Panicked,
}
type Fold = Arc<dyn Fn() -> Result<TrendsPayload, TrendsFoldError> + Send + Sync>;
type Completion = Arc<dyn Fn(TrendsWarmOutcome) + Send + Sync>;
type FailureSink = Arc<Mutex<Box<dyn Write + Send>>>;

pub fn read_trends_cache(
    database_path: impl AsRef<Path>,
    signature: TrendsSignature,
) -> Result<Option<Arc<TrendsPayload>>, TrendsCacheError> {
    let key = database_path.as_ref().to_string_lossy().into_owned();
    let cache = TRENDS_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| TrendsCacheError::CachePoisoned)?;
    Ok(cache
        .get(&key)
        .and_then(|(cached, payload)| (*cached == signature).then(|| Arc::clone(payload))))
}
pub fn replace_trends_cache(
    database_path: impl AsRef<Path>,
    signature: TrendsSignature,
    payload: TrendsPayload,
) -> Result<Arc<TrendsPayload>, TrendsCacheError> {
    let key = database_path.as_ref().to_string_lossy().into_owned();
    let payload = Arc::new(payload);
    TRENDS_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| TrendsCacheError::CachePoisoned)?
        .insert(key, (signature, Arc::clone(&payload)));
    Ok(payload)
}

/// Observes `warm_trends` calls on the constructing thread only.
///
/// [`Drop`] restores the thread's captured base. That is correct only for LIFO
/// nesting (inner probe dropped before an outer one); dropping an earlier probe
/// while a later one is still live rewinds the counter under the later probe.
pub struct TrendsWarmProbe {
    base: u64,
}

impl TrendsWarmProbe {
    pub fn new() -> Self {
        Self {
            base: TRENDS_WARM_INVOCATIONS.with(Cell::get),
        }
    }

    pub fn count(&self) -> u64 {
        TRENDS_WARM_INVOCATIONS
            .with(Cell::get)
            .saturating_sub(self.base)
    }
}

impl Default for TrendsWarmProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TrendsWarmProbe {
    fn drop(&mut self) {
        TRENDS_WARM_INVOCATIONS.with(|cell| cell.set(self.base));
    }
}

/// Starts the single-flight background fold. Calls count even when another fold owns the flight.
pub fn warm_trends(journal_root: impl Into<PathBuf>) {
    TRENDS_WARM_INVOCATIONS.with(|cell| cell.set(cell.get() + 1));
    let root = journal_root.into();
    warm_with(
        root.clone(),
        Arc::new(move || build_trends_payload(&root)),
        Arc::new(|_| {}),
        Arc::new(Mutex::new(Box::new(std::io::stderr()))),
    );
}

fn warm_with(root: PathBuf, fold: Fold, completion: Completion, failure_sink: FailureSink) {
    if TRENDS_WARM_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    std::thread::spawn(move || {
        let flight = FlightGuard;
        let database_path = health_dedupe_database_path(&root);
        let result = catch_unwind(AssertUnwindSafe(
            || -> Result<TrendsWarmOutcome, TrendsFoldError> {
                if let Ok(signature) = trends_signature(&root)
                    && read_trends_cache(&database_path, signature)
                        .ok()
                        .flatten()
                        .is_some()
                {
                    return Ok(TrendsWarmOutcome::Cached);
                }
                // Keep this before the fold: an import that lands mid-fold forces a retry.
                let signature = trends_signature(&root).map_err(TrendsFoldError::Signature)?;
                let payload = fold()?;
                replace_trends_cache(&database_path, signature, payload).map_err(|_| {
                    TrendsFoldError::Shards(ShardReadError::Read {
                        path: database_path.clone(),
                        source: std::io::Error::other("trends cache poisoned"),
                    })
                })?;
                Ok::<_, TrendsFoldError>(TrendsWarmOutcome::Succeeded)
            },
        ));
        let outcome = match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => {
                write_failure(
                    &failure_sink,
                    &format!("body trends warm failed: {error}\n"),
                );
                TrendsWarmOutcome::Failed
            }
            Err(_) => {
                write_failure(&failure_sink, "body trends warm panicked\n");
                TrendsWarmOutcome::Panicked
            }
        };
        drop(flight);
        completion(outcome);
    });
}
struct FlightGuard;
impl Drop for FlightGuard {
    fn drop(&mut self) {
        TRENDS_WARM_FLIGHT.store(false, Ordering::Release);
    }
}
fn write_failure(sink: &FailureSink, message: &str) {
    if let Ok(mut sink) = sink.lock() {
        let _ = sink.write_all(message.as_bytes());
        let _ = sink.flush();
    }
}

pub fn trends_response(root: Arc<PathBuf>) -> Value {
    let signature = match trends_signature(&*root) {
        Ok(value) => value,
        Err(_) => {
            warm_trends((*root).clone());
            return json!({"warming": true});
        }
    };
    let database = health_dedupe_database_path(&root);
    match read_trends_cache(database, signature) {
        Ok(Some(payload)) => {
            let mut response = signal_payload_json(&payload);
            response
                .as_object_mut()
                .unwrap()
                .insert("warming".into(), json!(false));
            response
        }
        _ => {
            warm_trends((*root).clone());
            json!({"warming": true})
        }
    }
}
fn signal_payload_json(payload: &TrendsPayload) -> Value {
    json!({"signals": payload.signals.iter().map(signal_json).collect::<Vec<_>>(), "annotations": payload.annotations.iter().map(|annotation| json!({"day": annotation.day, "label": annotation.label})).collect::<Vec<_>>(), "generated_at_day": payload.generated_at_day})
}
fn signal_json(signal: &TrendSignal) -> Value {
    json!({"key": signal.key, "label": signal.label, "unit_label": signal.unit_label, "daily": signal.daily.iter().map(|(day,value)| match value { TrendValue::Integer(value) => json!([day,value]), TrendValue::Real(value) => json!([day,value]) }).collect::<Vec<_>>(), "coverage": {"first_day": signal.coverage.first_day, "last_day": signal.coverage.last_day, "days": signal.coverage.days}})
}

pub fn typical_by_signal(payload: Option<&TrendsPayload>, day: &str) -> BTreeMap<String, f64> {
    let Some(payload) = payload else {
        return BTreeMap::new();
    };
    let Ok(day) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return BTreeMap::new();
    };
    let start = (day - Duration::days(TYPICAL_BASELINE_DAYS))
        .format("%Y%m%d")
        .to_string();
    let day = day.format("%Y%m%d").to_string();
    let mut typical = BTreeMap::new();
    for signal in &payload.signals {
        if !TYPICAL_SIGNAL_KEYS.contains(&signal.key.as_str()) {
            continue;
        }
        let mut values = signal
            .daily
            .iter()
            .filter(|(value_day, _)| {
                start.as_str() <= value_day.as_str() && value_day.as_str() < day.as_str()
            })
            .map(|(_, value)| value.as_f64())
            .collect::<Vec<_>>();
        if values.len() < TYPICAL_MIN_VALUES {
            continue;
        }
        values.sort_by(f64::total_cmp);
        let middle = values.len() / 2;
        let median = if values.len().is_multiple_of(2) {
            (values[middle - 1] + values[middle]) / 2.0
        } else {
            values[middle]
        };
        typical.insert(signal.key.clone(), median);
    }
    typical
}

fn build_trends_payload(root: &Path) -> Result<TrendsPayload, TrendsFoldError> {
    let mut latest = BTreeMap::<&str, BTreeMap<String, (String, f64)>>::new();
    let mut steps = BTreeMap::<String, i64>::new();
    let mut glucose = BTreeMap::<String, (f64, u64)>::new();
    let mut ring_resting = BTreeMap::<String, f64>::new();
    let mut sleep_rows = SleepRows::new();
    let mut first_source = BTreeMap::<String, String>::new();
    let mut first_glucose = None;
    let mut first_day = None;
    for month in coverage_month_keys(root).map_err(TrendsFoldError::Shards)? {
        let mut by_day = BTreeMap::<String, Vec<NormalizedRow>>::new();
        for row in read_normalized_rows(root, Some(&month)).map_err(TrendsFoldError::Shards)? {
            let Some(day) = field_text(&row.day) else {
                continue;
            };
            if !valid_day(day) {
                continue;
            }
            let day = day.to_owned();
            first_day = Some(first_day.map_or(day.clone(), |value: String| value.min(day.clone())));
            let source = source_label(&row);
            first_source
                .entry(source)
                .and_modify(|value| *value = value.clone().min(day.clone()))
                .or_insert(day.clone());
            by_day.entry(day).or_default().push(row);
        }
        for (day, rows) in by_day {
            let mut day_steps = Vec::new();
            for row in canonical_rows(rows) {
                let record = field_text(&row.record_type).unwrap_or("");
                if is_sleep(record) {
                    if let Some(low) = ring_lowest(&row) {
                        ring_resting
                            .entry(day.clone())
                            .and_modify(|value| *value = value.min(low))
                            .or_insert(low);
                    }
                    if let Some(start) = parse_time(row.row_time()) {
                        let end = parse_time(field_text(&row.end_date)).unwrap_or(start);
                        sleep_rows
                            .entry(day.clone())
                            .or_default()
                            .extend(sleep_entries(&row, start, end));
                    }
                } else if field_text(&row.kind) == Some("workout") {
                } else if record.contains("StepCount") {
                    day_steps.push(row);
                } else if is_glucose(record) {
                    if let Some(value) = value_float(&row) {
                        let entry = glucose.entry(day.clone()).or_insert((0.0, 0));
                        entry.0 += value;
                        entry.1 += 1;
                        first_glucose = Some(
                            first_glucose.map_or(day.clone(), |old: String| old.min(day.clone())),
                        );
                    }
                } else if record.contains("RestingHeartRate") {
                    fold_latest(latest.entry("resting_hr").or_default(), &day, &row, None);
                } else if let Some(key) = oura_key(record) {
                    let override_value = if record == "oura.daily_stress" {
                        metadata_float(&row, "stress_high").map(|value| round_even(value / 60.0, 1))
                    } else {
                        None
                    };
                    fold_latest(latest.entry(key).or_default(), &day, &row, override_value);
                } else if record == "oura.daily_activity" {
                    if let Some(value) = metadata_float(&row, "steps") {
                        day_steps.push(synthetic_step(&row, value));
                    }
                } else if is_body_mass(record) {
                    fold_latest(latest.entry("body_mass").or_default(), &day, &row, None);
                }
            }
            if let Some(total) = primary_steps(&day_steps) {
                steps.insert(day, total);
            }
        }
    }
    let mut daily = BTreeMap::<&str, BTreeMap<String, TrendValue>>::new();
    for (key, values) in latest {
        for (day, (_, value)) in values {
            daily
                .entry(key)
                .or_default()
                .insert(day, TrendValue::Real(value));
        }
    }
    for (day, value) in ring_resting {
        daily
            .entry("resting_hr")
            .or_default()
            .entry(day)
            .or_insert(TrendValue::Real(value));
    }
    for (day, value) in asleep_by_day(&sleep_rows) {
        daily
            .entry("asleep_minutes")
            .or_default()
            .insert(day, TrendValue::Real(value));
    }
    for (day, value) in steps {
        daily
            .entry("steps")
            .or_default()
            .insert(day, TrendValue::Integer(value));
    }
    for (day, (sum, count)) in glucose {
        daily
            .entry("glucose_avg")
            .or_default()
            .insert(day, TrendValue::Real(round_even(sum / count as f64, 1)));
    }
    let signals = TREND_SIGNALS
        .iter()
        .filter_map(|(key, label, unit)| {
            let values = daily.remove(key)?;
            (!values.is_empty()).then(|| TrendSignal {
                key: (*key).into(),
                label: (*label).into(),
                unit_label: (*unit).into(),
                coverage: TrendCoverage {
                    first_day: values.first_key_value().unwrap().0.clone(),
                    last_day: values.last_key_value().unwrap().0.clone(),
                    days: values.len(),
                },
                daily: values.into_iter().collect(),
            })
        })
        .collect();
    Ok(TrendsPayload {
        signals,
        annotations: trend_annotations(first_source, first_glucose, first_day),
        generated_at_day: local_day(Local::now()),
    })
}

fn field_text(field: &FieldState<Value>) -> Option<&str> {
    if let FieldState::Present(Value::String(value)) = field {
        Some(value)
    } else {
        None
    }
}
fn source_label(row: &NormalizedRow) -> String {
    field_text(&row.source_name)
        .map(str::to_owned)
        .unwrap_or_else(|| match field_text(&row.source_family).unwrap_or("") {
            "oura_api" => "Oura (API)".into(),
            "" => "unknown".into(),
            value => value.into(),
        })
}
fn body_string(value: &solstone_core_body_source::BodyString) -> String {
    value
        .code_points()
        .iter()
        .filter_map(|point| char::from_u32(*point))
        .collect()
}
fn value_float(row: &NormalizedRow) -> Option<f64> {
    match &row.value {
        ValueState::Present(BodyValue::Number(value)) => Some(*value),
        ValueState::Present(BodyValue::Integer(value)) => format!(
            "{}{}",
            if value.is_negative() { "-" } else { "" },
            value.digits()
        )
        .parse()
        .ok(),
        ValueState::Present(BodyValue::String(value)) => body_string(value).parse().ok(),
        _ => None,
    }
}
fn metadata_float(row: &NormalizedRow, key: &str) -> Option<f64> {
    match &row.metadata {
        FieldState::Present(Value::Object(metadata)) => {
            metadata.get(key).and_then(|value| match value {
                Value::Number(value) => value.as_f64(),
                Value::String(value) => value.parse().ok(),
                _ => None,
            })
        }
        _ => None,
    }
}
fn valid_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn parse_time(value: Option<&str>) -> Option<NaiveDateTime> {
    let value = value?.trim();
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.naive_local())
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .ok()
}
fn fold_latest(
    values: &mut BTreeMap<String, (String, f64)>,
    day: &str,
    row: &NormalizedRow,
    override_value: Option<f64>,
) {
    let Some(value) = override_value.or_else(|| value_float(row)) else {
        return;
    };
    let order = row.row_time().unwrap_or("").to_owned();
    if values.get(day).is_none_or(|(kept, _)| order > *kept) {
        values.insert(day.into(), (order, value));
    }
}
fn is_glucose(record: &str) -> bool {
    record.contains("BloodGlucose") || record.ends_with("Glucose")
}
fn is_sleep(record: &str) -> bool {
    record.contains("SleepAnalysis") || record == "oura.sleep"
}
fn is_body_mass(record: &str) -> bool {
    record.contains("BodyMass")
        && !record.contains("LeanBodyMass")
        && !record.contains("BodyMassIndex")
}
fn oura_key(record: &str) -> Option<&'static str> {
    match record {
        "oura.daily_cardiovascular_age" => Some("vascular_age"),
        "oura.daily_readiness" => Some("readiness"),
        "oura.daily_sleep" => Some("sleep_score"),
        "oura.temperature_deviation" => Some("temp_deviation"),
        "oura.daily_stress" => Some("stress_high_minutes"),
        _ => None,
    }
}
fn ring_lowest(row: &NormalizedRow) -> Option<f64> {
    (field_text(&row.record_type) == Some("oura.sleep"))
        .then(|| metadata_float(row, "lowest_heart_rate"))
        .flatten()
        .filter(|value| *value > 0.0)
}
fn sleep_entries(
    row: &NormalizedRow,
    start: NaiveDateTime,
    mut end: NaiveDateTime,
) -> Vec<(String, NaiveDateTime, NaiveDateTime, Option<String>)> {
    let source = source_label(row);
    if field_text(&row.record_type) != Some("oura.sleep") {
        return vec![(
            source,
            start,
            end,
            match &row.value {
                ValueState::Present(value) => Some(format_body_value(value)),
                _ => None,
            },
        )];
    }
    if end <= start
        && let Some(seconds) = metadata_float(row, "time_in_bed").filter(|value| *value > 0.0)
    {
        end = start + Duration::seconds(seconds as i64);
    }
    let mut rows = vec![(source.clone(), start, end, None)];
    if let Some(seconds) = oura_asleep(row).filter(|value| *value > 0.0) {
        rows.push((
            source,
            start,
            (start + Duration::seconds(seconds as i64)).min(end),
            Some("asleep".into()),
        ));
    }
    rows
}
fn format_body_value(value: &BodyValue) -> String {
    match value {
        BodyValue::String(value) => body_string(value),
        BodyValue::Integer(value) => format!(
            "{}{}",
            if value.is_negative() { "-" } else { "" },
            value.digits()
        ),
        BodyValue::Number(value) => value.to_string(),
        BodyValue::Bool(value) => value.to_string(),
        BodyValue::Null => "None".into(),
        _ => String::new(),
    }
}
fn oura_asleep(row: &NormalizedRow) -> Option<f64> {
    value_float(row).filter(|value| *value > 0.0).or_else(|| {
        [
            "deep_sleep_duration",
            "light_sleep_duration",
            "rem_sleep_duration",
        ]
        .into_iter()
        .filter_map(|key| metadata_float(row, key))
        .reduce(|sum, value| sum + value)
    })
}
fn canonical_rows(rows: Vec<NormalizedRow>) -> Vec<NormalizedRow> {
    let mut fragments = BTreeSet::new();
    for row in &rows {
        if field_text(&row.source_family) == Some("oura_api") {
            match field_text(&row.record_type).unwrap_or("") {
                "oura.sleep" => {
                    fragments.insert("SleepAnalysis");
                }
                "oura.heartrate" => {
                    fragments.insert("=HKQuantityTypeIdentifierHeartRate");
                }
                "oura.daily_activity" => {
                    for value in [
                        "StepCount",
                        "ActiveEnergyBurned",
                        "BasalEnergyBurned",
                        "DistanceWalkingRunning",
                    ] {
                        fragments.insert(value);
                    }
                }
                "oura.workout" => {
                    fragments.insert("WorkoutActivityType");
                }
                _ => {}
            }
        }
    }
    rows.into_iter()
        .filter(|row| {
            !(field_text(&row.source_family) == Some("apple_health")
                && field_text(&row.source_name)
                    .is_some_and(|name| name.to_lowercase().contains("oura"))
                && fragments.iter().any(|fragment| {
                    if let Some(exact) = fragment.strip_prefix('=') {
                        field_text(&row.record_type) == Some(exact)
                    } else {
                        field_text(&row.record_type).is_some_and(|record| record.contains(fragment))
                    }
                }))
        })
        .collect()
}
fn synthetic_step(row: &NormalizedRow, value: f64) -> NormalizedRow {
    let mut row = row.clone();
    row.record_type = FieldState::Present(Value::String("StepCount/oura.daily_activity".into()));
    row.source_family = FieldState::Present(Value::String("oura_api".into()));
    row.value = ValueState::Present(BodyValue::Number(value));
    let day = field_text(&row.day).unwrap();
    row.start_date = FieldState::Present(Value::String(format!(
        "{}-{}-{}T00:00:00",
        &day[..4],
        &day[4..6],
        &day[6..]
    )));
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").unwrap() + Duration::days(1);
    row.end_date = FieldState::Present(Value::String(format!(
        "{}T00:00:00",
        date.format("%Y-%m-%d")
    )));
    row
}
fn primary_steps(rows: &[NormalizedRow]) -> Option<i64> {
    let mut per_source = BTreeMap::<String, (f64, f64)>::new();
    for row in rows {
        let value = value_float(row)?;
        let source = source_label(row);
        let entry = per_source.entry(source).or_insert((0.0, 0.0));
        entry.0 += value;
        if let (Some(start), Some(end)) = (
            parse_time(row.row_time()),
            parse_time(field_text(&row.end_date)),
        ) {
            entry.1 += (end - start).num_seconds() as f64;
        }
    }
    if per_source.is_empty() {
        return None;
    }
    let primary = if per_source.len() == 1 {
        per_source.first_key_value()?.0
    } else {
        per_source
            .iter()
            .filter(|(_, (_, coverage))| *coverage > 0.0)
            .max_by(|(a, (_, x)), (b, (_, y))| x.total_cmp(y).then_with(|| b.cmp(a)))
            .map(|(source, _)| source)?
    };
    Some(round_even(per_source[primary].0, 0) as i64)
}
fn asleep_by_day(rows: &SleepRows) -> BTreeMap<String, f64> {
    let mut candidates = BTreeSet::new();
    for day in rows.keys() {
        candidates.insert(day.clone());
        if let Ok(date) = NaiveDate::parse_from_str(day, "%Y%m%d") {
            candidates.insert((date + Duration::days(1)).format("%Y%m%d").to_string());
        }
    }
    let mut result = BTreeMap::new();
    for day in candidates {
        let Ok(date) = NaiveDate::parse_from_str(&day, "%Y%m%d") else {
            continue;
        };
        let mut nearby = BTreeMap::new();
        for offset in [-1, 0, 1] {
            let key = (date + Duration::days(offset)).format("%Y%m%d").to_string();
            for (source, start, end, stage) in rows.get(&key).into_iter().flatten() {
                nearby.entry(source.clone()).or_insert_with(Vec::new).push((
                    *start,
                    *end,
                    stage.clone(),
                ));
            }
        }
        if let Some(DaySleep {
            asleep_minutes: Some(value),
            ..
        }) = pick_day_sleep(&nearby, date, SLEEP_SESSION_GAP_MINUTES)
        {
            result.insert(day, round_even(value, 1));
        }
    }
    result
}
fn trend_annotations(
    sources: BTreeMap<String, String>,
    glucose: Option<String>,
    first: Option<String>,
) -> Vec<TrendAnnotation> {
    let Some(first) = first else { return vec![] };
    let mut candidates = sources
        .into_iter()
        .filter(|(_, day)| *day > first)
        .map(|(source, day)| TrendAnnotation {
            day,
            label: format!("{source} data begins"),
        })
        .collect::<Vec<_>>();
    if let Some(day) = glucose.filter(|day| *day > first) {
        candidates.push(TrendAnnotation {
            day,
            label: "CGM readings begin".into(),
        })
    }
    candidates.sort_by(|left, right| (&left.day, &left.label).cmp(&(&right.day, &right.label)));
    candidates.truncate(TREND_ANNOTATION_LIMIT);
    candidates
}
pub(crate) fn local_day(now: DateTime<Local>) -> String {
    now.format("%Y%m%d").to_string()
}
pub(crate) fn round_even(value: f64, digits: u32) -> f64 {
    let factor = 10_f64.powi(digits as i32);
    let scaled = value * factor;
    let floor = scaled.floor();
    let fraction = scaled - floor;
    let rounded = if (fraction - 0.5).abs() <= f64::EPSILON * scaled.abs().max(1.0) {
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        scaled.round()
    };
    rounded / factor
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, AtomicUsize};
    use std::sync::mpsc;

    use chrono::TimeZone;
    use serde_json::{Map, json};
    use sha2::{Digest, Sha256};

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static TEST_SERIAL: Mutex<()> = Mutex::new(());

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-trends-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn database(&self) -> PathBuf {
            self.0.join("imports/health-dedupe.sqlite")
        }
        fn create_database(&self) {
            fs::create_dir_all(self.0.join("imports")).unwrap();
            fs::write(self.database(), []).unwrap();
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn clear_cache() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(!TRENDS_WARM_FLIGHT.load(Ordering::Acquire));
        TRENDS_CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap()
            .clear();
        guard
    }
    fn payload(label: &str) -> TrendsPayload {
        TrendsPayload {
            signals: vec![signal(
                "readiness",
                vec![("20240101".into(), TrendValue::Real(7.0))],
            )],
            annotations: vec![TrendAnnotation {
                day: "20240101".into(),
                label: label.into(),
            }],
            generated_at_day: "20240101".into(),
        }
    }
    fn sink() -> (FailureSink, Arc<Mutex<Vec<u8>>>) {
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(value);
                Ok(value.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let bytes = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(Mutex::new(Box::new(Capture(Arc::clone(&bytes))))),
            bytes,
        )
    }
    fn completion() -> (Completion, mpsc::Receiver<TrendsWarmOutcome>) {
        let (send, receive) = mpsc::channel();
        (
            Arc::new(move |outcome| send.send(outcome).unwrap()),
            receive,
        )
    }
    fn route_value(root: &TempDir) -> Value {
        trends_response(Arc::new(root.0.clone()))
    }

    fn canonical_json(value: &Value) -> Vec<u8> {
        fn encode(value: &Value, output: &mut String) {
            match value {
                Value::Null => output.push_str("null"),
                Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
                Value::Number(value) => output.push_str(&value.to_string()),
                Value::String(value) => string(value, output),
                Value::Array(values) => {
                    output.push('[');
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 {
                            output.push(',');
                        }
                        encode(value, output);
                    }
                    output.push(']');
                }
                Value::Object(values) => {
                    let mut entries = values.iter().collect::<Vec<_>>();
                    entries.sort_by_key(|(key, _)| *key);
                    output.push('{');
                    for (index, (key, value)) in entries.into_iter().enumerate() {
                        if index > 0 {
                            output.push(',');
                        }
                        string(key, output);
                        output.push(':');
                        encode(value, output);
                    }
                    output.push('}');
                }
            }
        }
        fn string(value: &str, output: &mut String) {
            output.push('"');
            for character in value.chars() {
                match character {
                    '"' => output.push_str("\\\""),
                    '\\' => output.push_str("\\\\"),
                    '\u{08}' => output.push_str("\\b"),
                    '\u{0c}' => output.push_str("\\f"),
                    '\n' => output.push_str("\\n"),
                    '\r' => output.push_str("\\r"),
                    '\t' => output.push_str("\\t"),
                    character if character <= '\u{1f}' => {
                        use std::fmt::Write as _;
                        write!(output, "\\u{:04x}", character as u32).unwrap();
                    }
                    character if character.is_ascii() => output.push(character),
                    character => {
                        for unit in character.encode_utf16(&mut [0; 2]) {
                            use std::fmt::Write as _;
                            write!(output, "\\u{:04x}", unit).unwrap();
                        }
                    }
                }
            }
            output.push('"');
        }
        let mut output = String::new();
        encode(value, &mut output);
        output.into_bytes()
    }
    fn assert_python_bytes(value: &Value, bytes: usize, sha: &str) {
        let encoded = canonical_json(value);
        assert_eq!(encoded.len(), bytes);
        assert_eq!(format!("{:x}", Sha256::digest(encoded)), sha);
    }
    fn fixture_case(name: &str) -> Value {
        let fixture = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../core/fixtures/convey_body_corpus.json"),
        )
        .unwrap();
        let fixture: Value = serde_json::from_str(&fixture).unwrap();
        fixture["cases"][name]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["path"] == "/app/body/api/trends")
            .unwrap()["json"]
            .clone()
    }
    fn normalized(mut value: Value) -> Value {
        value["generated_at_day"] = Value::String("<TODAY>".into());
        value
    }

    fn row(
        record: &str,
        day: &str,
        start: &str,
        value: Option<String>,
        source: &str,
        source_name: Option<&str>,
    ) -> Map<String, Value> {
        let mut row = Map::new();
        row.insert("source_family".into(), json!(source));
        row.insert("record_type".into(), json!(record));
        row.insert("day".into(), json!(day));
        row.insert("start_date".into(), json!(start));
        row.insert("month".into(), json!(&start[..7]));
        row.insert("kind".into(), json!("record"));
        if let Some(value) = value {
            row.insert("value".into(), json!(value));
        }
        if let Some(name) = source_name {
            row.insert("source_name".into(), json!(name));
        }
        row
    }
    fn push(bundle: &mut BTreeMap<String, Vec<Map<String, Value>>>, value: Map<String, Value>) {
        let month = value["month"].as_str().unwrap().to_owned();
        bundle.entry(month).or_default().push(value);
    }
    fn iso(day: NaiveDate, clock: &str) -> String {
        format!("{}T{clock}+00:00", day.format("%Y-%m-%d"))
    }
    fn seed_fixed(root: &TempDir) {
        let mut apple = BTreeMap::new();
        let mut oura = BTreeMap::new();
        let mut correction = BTreeMap::new();
        let start = NaiveDate::from_ymd_opt(2026, 5, 3).unwrap();
        let anchor = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        let mut first = row(
            "oura.sleep",
            "20260502",
            &iso(NaiveDate::from_ymd_opt(2026, 5, 2).unwrap(), "23:00:00"),
            Some("28800".into()),
            "oura_api",
            None,
        );
        first.insert("end_date".into(), json!(iso(start, "07:00:00")));
        first.insert(
            "metadata".into(),
            json!({"lowest_heart_rate":53,"time_in_bed":28800}),
        );
        push(&mut oura, first);
        for offset in 0..90 {
            let day = start + Duration::days(offset);
            let key = day.format("%Y%m%d").to_string();
            let rest = 52 + offset % 5;
            let mass = format!("{:.2}", 170.0 + offset as f64 * 0.03);
            push(
                &mut apple,
                row(
                    "HKQuantityTypeIdentifierRestingHeartRate",
                    &key,
                    &iso(day, "07:00:00"),
                    Some(rest.to_string()),
                    "apple_health",
                    Some("Synthetic Watch"),
                ),
            );
            push(
                &mut apple,
                row(
                    "HKQuantityTypeIdentifierBodyMass",
                    &key,
                    &iso(day, "08:00:00"),
                    Some(mass),
                    "apple_health",
                    Some("Synthetic Scale"),
                ),
            );
            for (record, value) in [
                ("oura.daily_readiness", 70 + offset % 12),
                ("oura.daily_sleep", 78 + offset % 9),
            ] {
                push(
                    &mut oura,
                    row(
                        record,
                        &key,
                        &iso(day, "04:00:00"),
                        Some(value.to_string()),
                        "oura_api",
                        None,
                    ),
                );
            }
            let mut activity = row(
                "oura.daily_activity",
                &key,
                &iso(day, "04:00:00"),
                Some((80 + offset % 10).to_string()),
                "oura_api",
                None,
            );
            activity.insert("metadata".into(), json!({"steps":7000+offset*13}));
            push(&mut oura, activity);
            let sleep_day = if day == anchor - Duration::days(1) {
                anchor
            } else {
                day
            };
            let mut sleep = row(
                "oura.sleep",
                &sleep_day.format("%Y%m%d").to_string(),
                &iso(day, "23:00:00"),
                Some("28800".into()),
                "oura_api",
                None,
            );
            sleep.insert(
                "end_date".into(),
                json!(iso(day + Duration::days(1), "07:00:00")),
            );
            sleep.insert(
                "metadata".into(),
                json!({"lowest_heart_rate":rest,"time_in_bed":28800}),
            );
            push(&mut oura, sleep);
        }
        let anchor_key = "20260801";
        let mut readiness = row(
            "oura.daily_readiness",
            anchor_key,
            &iso(anchor, "04:00:00"),
            Some("76".into()),
            "oura_api",
            None,
        );
        readiness.insert(
            "dedupe_key".into(),
            json!("oura-api:oura.daily_readiness:20260801"),
        );
        push(&mut oura, readiness);
        let mut corrected = row(
            "oura.daily_readiness",
            anchor_key,
            &iso(anchor, "04:00:00"),
            Some("82".into()),
            "oura_api",
            None,
        );
        corrected.insert(
            "dedupe_key".into(),
            json!("oura-api:oura.daily_readiness:20260801"),
        );
        push(&mut correction, corrected);
        push(
            &mut oura,
            row(
                "oura.daily_sleep",
                anchor_key,
                &iso(anchor, "04:00:00"),
                Some("84".into()),
                "oura_api",
                None,
            ),
        );
        let mut activity = row(
            "oura.daily_activity",
            anchor_key,
            &iso(anchor, "04:00:00"),
            Some("88".into()),
            "oura_api",
            None,
        );
        activity.insert("metadata".into(), json!({"steps":11234}));
        push(&mut oura, activity);
        push(
            &mut apple,
            row(
                "HKQuantityTypeIdentifierRestingHeartRate",
                anchor_key,
                &iso(anchor, "07:00:00"),
                Some("58".into()),
                "apple_health",
                Some("Synthetic Watch"),
            ),
        );
        for (clock, value) in [
            ("07:00:00", 91),
            ("11:00:00", 104),
            ("15:00:00", 98),
            ("19:00:00", 112),
        ] {
            push(
                &mut apple,
                row(
                    "HKQuantityTypeIdentifierBloodGlucose",
                    anchor_key,
                    &iso(anchor, clock),
                    Some(value.to_string()),
                    "apple_health",
                    Some("Synthetic CGM"),
                ),
            );
        }
        let mut mirror_steps = row(
            "HKQuantityTypeIdentifierStepCount",
            anchor_key,
            &iso(anchor, "00:00:00"),
            Some("11234".into()),
            "apple_health",
            Some("Oura Ring"),
        );
        mirror_steps.insert(
            "end_date".into(),
            json!(iso(anchor + Duration::days(1), "00:00:00")),
        );
        push(&mut apple, mirror_steps);
        push(
            &mut apple,
            row(
                "HKQuantityTypeIdentifierBodyMass",
                anchor_key,
                &iso(anchor, "08:00:00"),
                Some("172.4".into()),
                "apple_health",
                Some("Synthetic Scale"),
            ),
        );
        for (id, family, shards) in [
            ("20260810_080000", "apple_health", apple),
            ("20260810_090000", "oura_api", oura),
            ("20260810_100000", "oura_api", correction),
        ] {
            crate::seed_body_journal(
                &root.0,
                &crate::BodyJournalSeed {
                    dates: BTreeSet::new(),
                    day_summaries: BTreeMap::new(),
                    bundles: vec![crate::BodySeedBundle {
                        import_id: id.into(),
                        source_family: family.into(),
                        manifest: crate::BodySeedManifest::Absent,
                        shards,
                    }],
                    aggregate: crate::BodyAggregateSeed::Absent,
                    journal_config: None,
                },
            )
            .unwrap();
        }
        root.create_database();
    }
    fn seed_direct_resting_store(root: &TempDir, value: &str) {
        let mut shards = BTreeMap::new();
        let mut row = row(
            "HKQuantityTypeIdentifierRestingHeartRate",
            "20260801",
            "2026-08-01T00:00:00+00:00",
            Some(value.into()),
            "apple_health",
            Some("Synthetic"),
        );
        row.insert("dedupe_key".into(), json!("synthetic-key"));
        shards.insert("2026-08".into(), vec![row]);
        crate::seed_body_journal(
            &root.0,
            &crate::BodyJournalSeed {
                dates: BTreeSet::new(),
                day_summaries: BTreeMap::new(),
                bundles: vec![crate::BodySeedBundle {
                    import_id: "seed".into(),
                    source_family: "apple_health".into(),
                    manifest: crate::BodySeedManifest::Absent,
                    shards,
                }],
                aggregate: crate::BodyAggregateSeed::Direct,
                journal_config: None,
            },
        )
        .unwrap();
    }
    fn signal(key: &str, values: Vec<(String, TrendValue)>) -> TrendSignal {
        TrendSignal {
            key: key.into(),
            label: key.into(),
            unit_label: String::new(),
            coverage: TrendCoverage {
                first_day: values.first().map_or(String::new(), |(day, _)| day.clone()),
                last_day: values.last().map_or(String::new(), |(day, _)| day.clone()),
                days: values.len(),
            },
            daily: values,
        }
    }
    #[test]
    fn ribbon_constant_matches_reference() {
        assert_eq!(
            TREND_SIGNALS,
            [
                ("resting_hr", "Resting heart rate", "bpm"),
                ("vascular_age", "Vascular age", ""),
                ("asleep_minutes", "Asleep", "h"),
                ("sleep_score", "Sleep score", ""),
                ("readiness", "Readiness", ""),
                ("temp_deviation", "Temperature deviation", "°C"),
                ("stress_high_minutes", "Daytime stress high", "h"),
                ("steps", "Steps", "steps"),
                ("body_mass", "Body mass", "lb"),
                ("glucose_avg", "Glucose average", "mg/dL")
            ]
        );
    }
    #[test]
    fn payload_shape_and_typical_filter_match_python_contract() {
        let target = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        let days_before = |count| {
            (1..=count)
                .map(|offset| {
                    (
                        (target - Duration::days(offset))
                            .format("%Y%m%d")
                            .to_string(),
                        TrendValue::Real(offset as f64),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut readiness = days_before(14);
        readiness.push(("20240601".into(), TrendValue::Real(999.0)));
        let signals = [
            ("resting_hr", "bpm", days_before(5)),
            ("vascular_age", "", days_before(14)),
            ("asleep_minutes", "h", days_before(13)),
            ("sleep_score", "", days_before(14)),
            ("readiness", "", readiness),
            ("temp_deviation", "°C", days_before(14)),
            ("stress_high_minutes", "h", days_before(14)),
            ("steps", "steps", days_before(14)),
            ("body_mass", "lb", days_before(14)),
            ("glucose_avg", "mg/dL", days_before(14)),
        ]
        .into_iter()
        .map(|(key, unit_label, daily)| TrendSignal {
            key: key.into(),
            label: format!("{key} label"),
            unit_label: unit_label.into(),
            coverage: TrendCoverage {
                first_day: daily.first().map_or(String::new(), |(day, _)| day.clone()),
                last_day: daily.last().map_or(String::new(), |(day, _)| day.clone()),
                days: daily.len(),
            },
            daily,
        })
        .collect::<Vec<_>>();
        let payload = TrendsPayload {
            signals,
            annotations: vec![TrendAnnotation {
                day: "20240501".into(),
                label: "source begins".into(),
            }],
            generated_at_day: "20240601".into(),
        };
        assert_eq!(payload.signals.len(), 10);
        assert_eq!(payload.signals[1].unit_label, "");
        assert_eq!(payload.annotations[0].label, "source begins");
        let typical = typical_by_signal(Some(&payload), "20240601");
        assert_eq!(
            typical.keys().cloned().collect::<Vec<_>>(),
            ["readiness", "sleep_score"]
        );
        assert_eq!(typical["readiness"], 7.5);
        assert!(!typical.contains_key("asleep_minutes"));
        assert!(!typical.contains_key("vascular_age"));
    }
    #[test]
    fn trend_values_preserve_integer_steps() {
        let value = signal_json(&signal(
            "steps",
            vec![("20240101".into(), TrendValue::Integer(7000))],
        ));
        assert_eq!(value["daily"][0][1], json!(7000));
    }
    #[test]
    fn python_rounding_is_ties_to_even() {
        assert_eq!(round_even(101.25, 1), 101.2);
        assert_eq!(round_even(101.24, 1), 101.2);
        assert_eq!(round_even(1.235, 2), 1.24);
    }
    #[test]
    fn cache_replaces_stale_signature_without_accumulating_entries() {
        let _guard = clear_cache();
        let path = Path::new("/synthetic/trends.sqlite");
        let cache = TRENDS_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
        cache.lock().unwrap().clear();
        let first = replace_trends_cache(path, (1, 1, 0, 0), payload("first")).unwrap();
        let repeated = read_trends_cache(path, (1, 1, 0, 0)).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &repeated));
        let changed = replace_trends_cache(path, (2, 1, 0, 0), payload("changed")).unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(read_trends_cache(path, (1, 1, 0, 0)).unwrap().is_none());
        assert_eq!(cache.lock().unwrap().len(), 1);
    }

    #[test]
    fn frozen_corpus_encoder_and_fixed_replay_match_python() {
        let _guard = clear_cache();
        let fixed = fixture_case("fixed");
        let first_run = fixture_case("first_run");
        assert_python_bytes(
            &fixed,
            11_440,
            "a5c9724da95eb6f350437924fd5e21f3bc824c42098cc63cb292c794fe71f874",
        );
        assert_python_bytes(
            &first_run,
            16,
            "3810d12fa060554952ad4d9ce07e5e1ec3f4107484a46300525008d54ec11e5f",
        );
        let root = TempDir::new();
        seed_fixed(&root);
        let mut actual = signal_payload_json(&build_trends_payload(&root.0).unwrap());
        actual
            .as_object_mut()
            .unwrap()
            .insert("warming".into(), json!(false));
        assert_eq!(actual["annotations"], fixed["annotations"]);
        assert_eq!(normalized(actual.clone()), fixed);
        let expected_bytes = canonical_json(&fixed);
        let actual_bytes = canonical_json(&normalized(actual.clone()));
        let index = expected_bytes
            .iter()
            .zip(&actual_bytes)
            .position(|(left, right)| left != right);
        assert_eq!(
            index,
            None,
            "canonical mismatch at {:?}: expected {:?}, actual {:?}",
            index,
            index.map(|index| &expected_bytes[index..index + 24]),
            index.map(|index| &actual_bytes[index..index + 24])
        );
        assert_python_bytes(
            &normalized(actual),
            11_440,
            "a5c9724da95eb6f350437924fd5e21f3bc824c42098cc63cb292c794fe71f874",
        );
    }

    #[test]
    fn first_run_route_is_python_exact_while_injected_fold_is_held() {
        let _guard = clear_cache();
        let root = TempDir::new();
        root.create_database();
        let count = Arc::new(AtomicUsize::new(0));
        let (started_rx, release_tx, completion_rx) = {
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let release_rx = Arc::new(Mutex::new(release_rx));
            let (completion, completion_rx) = completion();
            let fold_count = Arc::clone(&count);
            warm_with(
                root.0.clone(),
                Arc::new(move || {
                    fold_count.fetch_add(1, Ordering::SeqCst);
                    started_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                    Ok(payload("done"))
                }),
                completion,
                sink().0,
            );
            (started_rx, release_tx, completion_rx)
        };
        started_rx.recv().unwrap();
        let cold = route_value(&root);
        assert_eq!(cold, json!({"warming":true}));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_python_bytes(
            &cold,
            16,
            "3810d12fa060554952ad4d9ce07e5e1ec3f4107484a46300525008d54ec11e5f",
        );
        release_tx.send(()).unwrap();
        assert_eq!(completion_rx.recv().unwrap(), TrendsWarmOutcome::Succeeded);
    }

    #[test]
    fn global_single_flight_serves_cache_and_recovers_after_panic() {
        let _guard = clear_cache();
        let root = TempDir::new();
        root.create_database();
        let other_root = TempDir::new();
        other_root.create_database();
        let count = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (done, done_rx) = completion();
        let count_fold = Arc::clone(&count);
        warm_with(
            root.0.clone(),
            Arc::new(move || {
                count_fold.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Ok(payload("cached"))
            }),
            done,
            sink().0,
        );
        started_rx.recv().unwrap();
        let other_count = Arc::clone(&count);
        let (other_done, _other_rx) = completion();
        warm_with(
            other_root.0.clone(),
            Arc::new(move || {
                other_count.fetch_add(1, Ordering::SeqCst);
                Ok(payload("other journal"))
            }),
            other_done,
            sink().0,
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(route_value(&root), json!({"warming":true}));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        release_tx.send(()).unwrap();
        assert_eq!(done_rx.recv().unwrap(), TrendsWarmOutcome::Succeeded);
        assert_eq!(route_value(&root)["warming"], json!(false));
        TRENDS_CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap()
            .clear();
        let (panic_done, panic_rx) = completion();
        let panic_count = Arc::clone(&count);
        warm_with(
            root.0.clone(),
            Arc::new(move || -> Result<_, _> {
                panic_count.fetch_add(1, Ordering::SeqCst);
                panic!("test panic")
            }),
            panic_done,
            sink().0,
        );
        assert_eq!(panic_rx.recv().unwrap(), TrendsWarmOutcome::Panicked);
        assert!(!TRENDS_WARM_FLIGHT.load(Ordering::Acquire));
        let (retry_done, retry_rx) = completion();
        let retry_count = Arc::clone(&count);
        warm_with(
            root.0.clone(),
            Arc::new(move || {
                retry_count.fetch_add(1, Ordering::SeqCst);
                Ok(payload("retried after panic"))
            }),
            retry_done,
            sink().0,
        );
        assert_eq!(retry_rx.recv().unwrap(), TrendsWarmOutcome::Succeeded);
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn failed_warm_writes_constructed_sink_and_route_keeps_warming() {
        let _guard = clear_cache();
        let root = TempDir::new();
        root.create_database();
        let (done, rx) = completion();
        let (failure_sink, bytes) = sink();
        warm_with(
            root.0.clone(),
            Arc::new(|| {
                Err(TrendsFoldError::Shards(ShardReadError::Read {
                    path: PathBuf::from("synthetic.jsonl"),
                    source: std::io::Error::other("boom"),
                }))
            }),
            done,
            failure_sink,
        );
        assert_eq!(rx.recv().unwrap(), TrendsWarmOutcome::Failed);
        assert!(String::from_utf8(bytes.lock().unwrap().clone()).unwrap().contains("body trends warm failed: trends shard fold failed: could not read synthetic.jsonl: boom"));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (retry, retry_rx) = completion();
        warm_with(
            root.0.clone(),
            Arc::new(move || {
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Ok(payload("retry"))
            }),
            retry,
            sink().0,
        );
        started_rx.recv().unwrap();
        assert_eq!(route_value(&root), json!({"warming":true}));
        release_tx.send(()).unwrap();
        assert_eq!(retry_rx.recv().unwrap(), TrendsWarmOutcome::Succeeded);
    }

    #[test]
    fn signature_before_fold_caches_under_the_old_signature_then_refolds() {
        let _guard = clear_cache();
        let root = TempDir::new();
        root.create_database();
        let count = Arc::new(AtomicUsize::new(0));
        let before = trends_signature(&root.0).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (done, rx) = completion();
        let count_fold = Arc::clone(&count);
        warm_with(
            root.0.clone(),
            Arc::new(move || {
                count_fold.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Ok(payload("first"))
            }),
            done,
            sink().0,
        );
        started_rx.recv().unwrap();
        fs::write(root.database(), b"changed database").unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(rx.recv().unwrap(), TrendsWarmOutcome::Succeeded);
        let after = trends_signature(&root.0).unwrap();
        assert_ne!(before, after);
        assert!(
            read_trends_cache(root.database(), before)
                .unwrap()
                .is_some()
        );
        assert!(read_trends_cache(root.database(), after).unwrap().is_none());
        let (retry_done, retry_rx) = completion();
        let retry_count = Arc::clone(&count);
        warm_with(
            root.0.clone(),
            Arc::new(move || {
                retry_count.fetch_add(1, Ordering::SeqCst);
                Ok(payload("second"))
            }),
            retry_done,
            sink().0,
        );
        assert_eq!(retry_rx.recv().unwrap(), TrendsWarmOutcome::Succeeded);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(
            read_trends_cache(root.database(), after)
                .unwrap()
                .unwrap()
                .annotations[0]
                .label,
            "second"
        );
    }

    #[test]
    fn shard_only_change_returns_stale_payload_without_refolding() {
        let _guard = clear_cache();
        let root = TempDir::new();
        root.create_database();
        let count = Arc::new(AtomicUsize::new(0));
        let (done, rx) = completion();
        let fold_count = Arc::clone(&count);
        warm_with(
            root.0.clone(),
            Arc::new(move || {
                fold_count.fetch_add(1, Ordering::SeqCst);
                Ok(payload("stale"))
            }),
            done,
            sink().0,
        );
        assert_eq!(rx.recv().unwrap(), TrendsWarmOutcome::Succeeded);
        let warmed = route_value(&root);
        assert_eq!(warmed["warming"], json!(false));
        assert_eq!(warmed["annotations"][0]["label"], json!("stale"));
        let shards = root.0.join("imports/shard-only/normalized");
        fs::create_dir_all(&shards).unwrap();
        fs::write(
            shards.join("2030-01.jsonl"),
            "{\"day\":\"20300101\",\"record_type\":\"synthetic\"}\n",
        )
        .unwrap();
        assert_eq!(route_value(&root), warmed);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fixed_seed_omits_never_observed_signals_and_annotations_are_sorted_and_limited() {
        let _guard = clear_cache();
        let root = TempDir::new();
        seed_fixed(&root);
        let built = build_trends_payload(&root.0).unwrap();
        let keys = built
            .signals
            .iter()
            .map(|signal| signal.key.as_str())
            .collect::<Vec<_>>();
        assert!(
            !keys.contains(&"vascular_age")
                && !keys.contains(&"temp_deviation")
                && !keys.contains(&"stress_high_minutes")
        );
        let sources = [
            ("z", "20240104"),
            ("a", "20240102"),
            ("b", "20240103"),
            ("c", "20240105"),
            ("d", "20240106"),
            ("e", "20240107"),
            ("same", "20240101"),
        ]
        .into_iter()
        .map(|(source, day)| (source.into(), day.into()))
        .collect();
        let annotations =
            trend_annotations(sources, Some("20240102".into()), Some("20240101".into()));
        assert_eq!(
            annotations
                .iter()
                .map(|annotation| (annotation.day.as_str(), annotation.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("20240102", "CGM readings begin"),
                ("20240102", "a data begins"),
                ("20240103", "b data begins"),
                ("20240104", "z data begins"),
                ("20240105", "c data begins"),
                ("20240106", "d data begins")
            ]
        );
    }

    #[test]
    fn typical_threshold_window_and_supported_keys_match_contract() {
        let target = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        let days = |count| {
            (0..count)
                .map(|offset| {
                    (
                        (target - Duration::days(90 - offset))
                            .format("%Y%m%d")
                            .to_string(),
                        TrendValue::Real((offset + 1) as f64),
                    )
                })
                .collect::<Vec<_>>()
        };
        let at_13 = TrendsPayload {
            signals: vec![signal("readiness", days(13))],
            annotations: vec![],
            generated_at_day: String::new(),
        };
        assert!(typical_by_signal(Some(&at_13), "20240601").is_empty());
        let mut values = days(14);
        values.push(("20240601".into(), TrendValue::Real(999.0)));
        let payload = TrendsPayload {
            signals: vec![
                signal("readiness", values),
                signal("vascular_age", days(14)),
            ],
            annotations: vec![],
            generated_at_day: String::new(),
        };
        let typical = typical_by_signal(Some(&payload), "20240601");
        assert_eq!(typical["readiness"], 7.5);
        assert!(!typical.contains_key("vascular_age"));
    }

    #[test]
    fn generated_day_projects_the_local_clock_without_reading_the_clock() {
        let now = Local.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        assert_eq!(local_day(now), "20260801");
    }

    #[test]
    fn imported_store_change_refolds_to_new_route_numbers_once() {
        let _guard = clear_cache();
        let root = TempDir::new();
        seed_direct_resting_store(&root, "50");
        let count = Arc::new(AtomicUsize::new(0));
        let (first_done, first_rx) = completion();
        let first_root = root.0.clone();
        let first_count = Arc::clone(&count);
        warm_with(
            root.0.clone(),
            Arc::new(move || {
                first_count.fetch_add(1, Ordering::SeqCst);
                build_trends_payload(&first_root)
            }),
            first_done,
            sink().0,
        );
        assert_eq!(first_rx.recv().unwrap(), TrendsWarmOutcome::Succeeded);
        let first = route_value(&root);
        assert_eq!(first["signals"][0]["daily"][0][1], json!(50.0));
        assert_eq!(route_value(&root), first);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let mut imported = row(
            "HKQuantityTypeIdentifierRestingHeartRate",
            "20260801",
            "2026-08-01T01:00:00+00:00",
            Some("60".into()),
            "apple_health",
            Some("Synthetic"),
        );
        imported.insert("dedupe_key".into(), json!("synthetic-key"));
        let imported_path = root.0.join("imports/zzimported/normalized");
        fs::create_dir_all(&imported_path).unwrap();
        fs::write(
            imported_path.join("2026-08.jsonl"),
            format!(
                "{}\n",
                serde_json::to_string(&Value::Object(imported)).unwrap()
            ),
        )
        .unwrap();
        rusqlite::Connection::open(root.database())
            .unwrap()
            .execute_batch("CREATE TABLE signature_touch (value INTEGER);")
            .unwrap();
        let (changed_done, changed_rx) = completion();
        let changed_root = root.0.clone();
        let changed_count = Arc::clone(&count);
        warm_with(
            root.0.clone(),
            Arc::new(move || {
                changed_count.fetch_add(1, Ordering::SeqCst);
                build_trends_payload(&changed_root)
            }),
            changed_done,
            sink().0,
        );
        assert_eq!(changed_rx.recv().unwrap(), TrendsWarmOutcome::Succeeded);
        let changed = route_value(&root);
        assert_eq!(changed["signals"][0]["daily"][0][1], json!(60.0));
        assert_eq!(route_value(&root), changed);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn store_signature_invalidates_both_native_caches() {
        let _guard = clear_cache();
        let root = TempDir::new();
        seed_direct_resting_store(&root, "50");
        let before = trends_signature(&root.0).unwrap();
        let stats_before = crate::read_health_dedupe_stats(&root.0).unwrap().unwrap();
        replace_trends_cache(root.database(), before, payload("before")).unwrap();
        rusqlite::Connection::open(root.database())
            .unwrap()
            .execute_batch("CREATE TABLE signature_touch (value INTEGER);")
            .unwrap();
        let after = trends_signature(&root.0).unwrap();
        assert_ne!(before, after);
        assert!(read_trends_cache(root.database(), after).unwrap().is_none());
        let stats_after = crate::read_health_dedupe_stats(&root.0).unwrap().unwrap();
        assert!(!Arc::ptr_eq(&stats_before, &stats_after));
    }
}
