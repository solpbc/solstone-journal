// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Health report folds for capture, synthesis, and consumer signals.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use solstone_core_entity::load_all_journal_entities;
use solstone_core_facets::{list_declared_facet_names, load_activity_records};
use solstone_core_system_health::{
    FilesystemHealthLogSource, FilesystemSegmentSource, SegmentInput, classify_segment_completion,
    read_segment_progress, scan_day,
};

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
const FACET_SILENT_INFO_HOURS: i64 = 24;
const FACET_SILENT_WARN_HOURS: i64 = 72;
const FACET_SILENT_CRITICAL_HOURS: i64 = 168;
const INDEXER_STALE_WARN_DAYS: i64 = 7;
const SPEC_POINTER: &str = "solstone/think/surfaces/health.py";
const NO_ENGINE_ANALYSIS_TEXT: &str =
    "No thinking engine is chosen yet. Choose one in Thinking so observations can be analyzed.";

#[derive(Debug)]
pub(crate) enum HealthError {
    InvalidRequest(String),
    MissingRequiredField(String),
    Internal { context: String },
}

impl HealthError {
    pub(crate) fn internal(context: impl Into<String>) -> Self {
        Self::Internal {
            context: context.into(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct HealthReport {
    pub(crate) generated_at: i64,
    pub(crate) range: (String, String),
    pub(crate) facets: Vec<String>,
    pub(crate) capture_health: CaptureHealth,
    pub(crate) synthesis_health: SynthesisHealth,
    pub(crate) consumer_signal: ConsumerSignalHealth,
    pub(crate) segment_backlog: SegmentBacklogHealth,
    pub(crate) notes: Vec<HealthNote>,
    pub(crate) brain_health: Value,
}

#[derive(Debug, Serialize)]
pub(crate) struct CaptureHealth {
    pub(crate) hours_with_capture: u64,
    pub(crate) hours_total: u64,
    pub(crate) coverage_ratio: Option<f64>,
    pub(crate) facets_with_recent_capture: Vec<String>,
    pub(crate) facets_silent_24h: Vec<String>,
    pub(crate) last_segment_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SynthesisHealth {
    pub(crate) activities_count: u64,
    pub(crate) activities_with_participation: u64,
    pub(crate) activities_with_story: u64,
    pub(crate) activities_user_edited: u64,
    pub(crate) activities_anticipated_unfilled: u64,
    pub(crate) talent_run_failures_24h: Option<u64>,
    pub(crate) talent_degraded_outputs_24h: Option<u64>,
    pub(crate) indexer_last_rebuild_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConsumerSignalHealth {
    pub(crate) profile_entities_total: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct SegmentBacklogHealth {
    pub(crate) not_thought: u64,
    pub(crate) days_with_backlog: u64,
    pub(crate) errors: Vec<String>,
    pub(crate) not_sensed: u64,
    pub(crate) awaiting_analysis_text: Option<String>,
    pub(crate) last_drained_at: Option<i64>,
    pub(crate) drain_state: String,
    pub(crate) display_powersave_detectable: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct HealthNote {
    pub(crate) severity: String,
    pub(crate) category: String,
    pub(crate) message: String,
    pub(crate) detected_at: i64,
    pub(crate) detail_pointer: Option<String>,
}

#[derive(Debug, Default)]
struct ScanAggregate {
    capture_hour_slots: BTreeSet<(String, u32)>,
    last_segment_at: Option<i64>,
    activities_count: u64,
    activities_with_participation: u64,
    activities_with_story: u64,
    activities_user_edited: u64,
    activities_anticipated_unfilled: u64,
}

#[derive(Debug, Default)]
struct TalentHealthScan {
    failures: u64,
    degraded: u64,
    degraded_rows: Vec<(i64, Map<String, Value>)>,
    problems: BTreeMap<String, Vec<String>>,
}

pub(crate) fn resolve_day(value: &str) -> Result<NaiveDate, HealthError> {
    if value.len() != 8 || !value.as_bytes().iter().all(u8::is_ascii_digit) {
        return Err(HealthError::InvalidRequest(
            "day must match YYYYMMDD".to_owned(),
        ));
    }
    NaiveDate::parse_from_str(value, "%Y%m%d")
        .map_err(|_| HealthError::InvalidRequest("day must match YYYYMMDD".to_owned()))
}

pub(crate) fn resolve_range(
    day_from: Option<&str>,
    day_to: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(NaiveDate, NaiveDate), HealthError> {
    let (from, to) = match (day_from, day_to) {
        (None, None) => (now.date_naive() - Duration::days(6), now.date_naive()),
        (Some(from), Some(to)) => (resolve_day(from)?, resolve_day(to)?),
        _ => {
            return Err(HealthError::InvalidRequest(
                "both endpoints or neither".to_owned(),
            ));
        }
    };
    if from > to {
        return Err(HealthError::InvalidRequest(
            "day_from must be <= day_to".to_owned(),
        ));
    }
    Ok((from, to))
}

pub(crate) fn build_health_report(
    journal_root: &Path,
    range: (NaiveDate, NaiveDate),
    now: DateTime<Utc>,
) -> Result<HealthReport, HealthError> {
    let generated_at = now.timestamp_millis();
    let facets = list_declared_facet_names(journal_root)
        .map_err(|error| HealthError::internal(error.to_string()))?;
    let aggregate = scan_records(journal_root, &facets, range, now)?;
    let (capture_health, mut notes) =
        build_capture_health(journal_root, &aggregate, range, &facets, generated_at, now)?;
    let (synthesis_health, synthesis_notes) =
        build_synthesis_health(journal_root, &aggregate, now)?;
    notes.extend(synthesis_notes);
    notes.sort_by(|left, right| {
        severity_rank(&left.severity)
            .cmp(&severity_rank(&right.severity))
            .then(left.category.cmp(&right.category))
            .then(left.message.cmp(&right.message))
    });
    let consumer_signal = build_consumer_signal_health(journal_root)?;
    // Native has no display-power poller; this is the reference monitor's pre-poll state.
    let segment_backlog = build_segment_backlog_health(
        journal_root,
        now,
        DisplayPowersaveReading::UNAVAILABLE,
        false,
        Local::now().time(),
    )?;
    Ok(HealthReport {
        generated_at,
        range: (
            range.0.format("%Y%m%d").to_string(),
            range.1.format("%Y%m%d").to_string(),
        ),
        facets,
        capture_health,
        synthesis_health,
        consumer_signal,
        segment_backlog,
        brain_health: crate::brain_action::build_cli_brain_health(journal_root, now),
        notes,
    })
}

fn scan_records(
    journal_root: &Path,
    facets: &[String],
    range: (NaiveDate, NaiveDate),
    now: DateTime<Utc>,
) -> Result<ScanAggregate, HealthError> {
    let mut aggregate = ScanAggregate::default();
    for day in days_inclusive(range) {
        let day_name = day.format("%Y%m%d").to_string();
        for facet in facets {
            let records = load_activity_records(journal_root, facet, &day_name, true)
                .map_err(|error| HealthError::internal(error.to_string()))?;
            for record in records {
                add_record(&mut aggregate, &record, day, now);
            }
        }
    }
    Ok(aggregate)
}

fn add_record(
    aggregate: &mut ScanAggregate,
    record: &Map<String, Value>,
    day: NaiveDate,
    now: DateTime<Utc>,
) {
    for raw_segment in values(record.get("segments")) {
        let Some(raw_segment) = raw_segment.as_str() else {
            continue;
        };
        let Some((start, end)) = parse_segment_bounds(raw_segment, day) else {
            continue;
        };
        let clipped_end = end.min(DateTime::<Utc>::from_naive_utc_and_offset(
            (day + Duration::days(1))
                .and_hms_opt(0, 0, 0)
                .expect("midnight"),
            Utc,
        ));
        let mut hour = start
            .date_naive()
            .and_hms_opt(start.hour(), 0, 0)
            .expect("valid segment hour");
        while DateTime::<Utc>::from_naive_utc_and_offset(hour, Utc) < clipped_end {
            aggregate
                .capture_hour_slots
                .insert((day.format("%Y%m%d").to_string(), hour.hour()));
            hour += Duration::hours(1);
        }
        let end_ms = end.timestamp_millis();
        aggregate.last_segment_at = Some(
            aggregate
                .last_segment_at
                .map_or(end_ms, |last| last.max(end_ms)),
        );
    }
    if record
        .get("hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return;
    }
    aggregate.activities_count += 1;
    if record.get("participation").is_some_and(truthy) {
        aggregate.activities_with_participation += 1;
    }
    if record.get("story").is_some_and(truthy) {
        aggregate.activities_with_story += 1;
    }
    if values(record.get("edits")).iter().any(|edit| {
        edit.get("actor")
            .and_then(Value::as_str)
            .is_some_and(|actor| {
                actor.starts_with("cli:") || actor.starts_with("owner") || actor.starts_with("user")
            })
    }) {
        aggregate.activities_user_edited += 1;
    }
    if record.get("source").and_then(Value::as_str) == Some("anticipated")
        && !record
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        && record
            .get("start")
            .and_then(Value::as_str)
            .and_then(parse_start)
            .is_some_and(|start| start.timestamp_millis() <= now.timestamp_millis())
    {
        aggregate.activities_anticipated_unfilled += 1;
    }
}

fn build_capture_health(
    journal_root: &Path,
    aggregate: &ScanAggregate,
    range: (NaiveDate, NaiveDate),
    facets: &[String],
    generated_at: i64,
    now: DateTime<Utc>,
) -> Result<(CaptureHealth, Vec<HealthNote>), HealthError> {
    let last_seen = last_segment_per_facet(journal_root, facets, now)?;
    let cutoff = generated_at - FACET_SILENT_INFO_HOURS * HOUR_MS;
    let recent = facets
        .iter()
        .filter(|facet| {
            last_seen
                .get(*facet)
                .and_then(|value| *value)
                .is_some_and(|value| value >= cutoff)
        })
        .cloned()
        .collect::<Vec<_>>();
    let silent = facets
        .iter()
        .filter(|facet| !recent.contains(*facet))
        .cloned()
        .collect::<Vec<_>>();
    let mut notes = vec![note(
        "info",
        "capture",
        "coverage_ratio unavailable in v1 — expected-hours denominator arrives Sprint 5+",
        generated_at,
        Some(SPEC_POINTER),
    )];
    for facet in facets {
        match last_seen.get(facet).copied().flatten() {
            None => notes.push(note(
                "info",
                "capture",
                &format!("{facet}: no captures recorded in the last 7 days."),
                generated_at,
                None,
            )),
            Some(last_seen) => {
                let gap_hours = ((generated_at - last_seen) / HOUR_MS).max(0);
                let severity = if gap_hours >= FACET_SILENT_CRITICAL_HOURS {
                    Some("critical")
                } else if gap_hours >= FACET_SILENT_WARN_HOURS {
                    Some("warn")
                } else if gap_hours >= FACET_SILENT_INFO_HOURS {
                    Some("info")
                } else {
                    None
                };
                if let Some(severity) = severity {
                    let text = Utc
                        .timestamp_millis_opt(last_seen)
                        .single()
                        .expect("millisecond timestamp")
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                    notes.push(note(
                        severity,
                        "capture",
                        &format!("{facet}: last capture {gap_hours}h ago ({text})."),
                        generated_at,
                        None,
                    ));
                }
            }
        }
    }
    Ok((
        CaptureHealth {
            hours_with_capture: aggregate.capture_hour_slots.len() as u64,
            hours_total: ((range.1 - range.0).num_days() as u64 + 1) * 24,
            coverage_ratio: None,
            facets_with_recent_capture: recent,
            facets_silent_24h: silent,
            last_segment_at: aggregate.last_segment_at,
        },
        notes,
    ))
}

fn last_segment_per_facet(
    journal_root: &Path,
    facets: &[String],
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, Option<i64>>, HealthError> {
    let mut values_by_facet = facets
        .iter()
        .cloned()
        .map(|facet| (facet, None))
        .collect::<BTreeMap<_, Option<i64>>>();
    for offset in (0..=7).rev() {
        let day = now.date_naive() - Duration::days(offset);
        let day_name = day.format("%Y%m%d").to_string();
        for facet in facets {
            for record in load_activity_records(journal_root, facet, &day_name, true)
                .map_err(|error| HealthError::internal(error.to_string()))?
            {
                for segment in values(record.get("segments")) {
                    let Some(raw) = segment.as_str() else {
                        continue;
                    };
                    let Some((_, end)) = parse_segment_bounds(raw, day) else {
                        continue;
                    };
                    let end_ms = end.timestamp_millis();
                    values_by_facet.entry(facet.clone()).and_modify(|last| {
                        *last = Some(last.map_or(end_ms, |value| value.max(end_ms)));
                    });
                }
            }
        }
    }
    Ok(values_by_facet)
}

fn build_synthesis_health(
    journal_root: &Path,
    aggregate: &ScanAggregate,
    now: DateTime<Utc>,
) -> Result<(SynthesisHealth, Vec<HealthNote>), HealthError> {
    let generated_at = now.timestamp_millis();
    let mut notes = vec![note(
        "info",
        "synthesis",
        "corrections roll-up not available — corrections support arrives Sprint 5+",
        generated_at,
        Some(SPEC_POINTER),
    )];
    let talents = journal_root.join("talents");
    let guard_days = [
        now.date_naive().format("%Y%m%d").to_string(),
        (now.date_naive() - Duration::days(1))
            .format("%Y%m%d")
            .to_string(),
    ];
    let missing = guard_days
        .iter()
        .filter(|day| !talents.join(format!("{day}.jsonl")).exists())
        .cloned()
        .collect::<Vec<_>>();
    let scan = scan_talent_indexes(&talents, generated_at)?;
    if !missing.is_empty() {
        notes.push(note(
            "info",
            "synthesis",
            &format!(
                "talent day-index logs missing for {}; last-24h failure count unavailable.",
                missing.join(", ")
            ),
            generated_at,
            None,
        ));
    }
    for (filename, details) in scan.problems.iter().take(10) {
        let detail = if details.len() == 1 {
            details[0].clone()
        } else {
            format!("{} scan problems; first: {}", details.len(), details[0])
        };
        notes.push(note(
            "warn",
            "synthesis",
            &format!("talent day-index {filename} could not be fully read ({detail}); last-24h talent counts unavailable."),
            generated_at,
            None,
        ));
    }
    let (failures, degraded) = if missing.is_empty() && scan.problems.is_empty() {
        for (_, row) in scan.degraded_rows.iter().take(10) {
            let degraded = row.get("degraded").and_then(Value::as_object);
            if let Some(degraded) = degraded {
                notes.push(note(
                    "warn",
                    "synthesis",
                    &format!(
                        "talent '{}' finished near-empty: {} output tokens ({}/{}) on {}",
                        string(row.get("name")),
                        degraded
                            .get("output_tokens")
                            .and_then(Value::as_i64)
                            .unwrap_or(0),
                        string(row.get("provider")),
                        string(row.get("model")),
                        string(row.get("day")),
                    ),
                    generated_at,
                    None,
                ));
            }
        }
        (Some(scan.failures), Some(scan.degraded))
    } else {
        (None, None)
    };
    let indexer = journal_root.join("indexer/journal.sqlite");
    let indexer_last_rebuild_at = match fs::metadata(&indexer) {
        Ok(metadata) => {
            let value = metadata
                .modified()
                .map_err(|error| HealthError::internal(error.to_string()))?
                .duration_since(UNIX_EPOCH)
                .map_err(|error| HealthError::internal(error.to_string()))?
                .as_millis() as i64;
            if generated_at - value > INDEXER_STALE_WARN_DAYS * DAY_MS {
                notes.push(note(
                    "warn",
                    "synthesis",
                    &format!("indexer database last rebuilt {}d ago; search-backed consumers may be stale.", (generated_at - value) / DAY_MS),
                    generated_at,
                    None,
                ));
            }
            Some(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            notes.push(note("warn", "synthesis", "indexer database missing at journal/indexer/journal.sqlite; search-backed consumers may be stale.", generated_at, None));
            None
        }
        Err(error) => return Err(HealthError::internal(error.to_string())),
    };
    Ok((
        SynthesisHealth {
            activities_count: aggregate.activities_count,
            activities_with_participation: aggregate.activities_with_participation,
            activities_with_story: aggregate.activities_with_story,
            activities_user_edited: aggregate.activities_user_edited,
            activities_anticipated_unfilled: aggregate.activities_anticipated_unfilled,
            talent_run_failures_24h: failures,
            talent_degraded_outputs_24h: degraded,
            indexer_last_rebuild_at,
        },
        notes,
    ))
}

fn scan_talent_indexes(talents: &Path, generated_at: i64) -> Result<TalentHealthScan, HealthError> {
    let entries = match fs::read_dir(talents) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TalentHealthScan::default());
        }
        Err(error) => return Err(HealthError::internal(error.to_string())),
    };
    let mut paths = entries
        .map(|entry| entry.map(|value| value.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| HealthError::internal(error.to_string()))?;
    paths.sort();
    let mut scan = TalentHealthScan::default();
    let cutoff = generated_at - DAY_MS;
    for path in paths.into_iter().filter(|path| talent_index_filename(path)) {
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_owned();
        let contents = match fs::read(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                scan.problems
                    .entry(filename)
                    .or_default()
                    .push("file vanished during scan".to_owned());
                continue;
            }
            Err(error) => {
                scan.problems
                    .entry(filename)
                    .or_default()
                    .push(format!("unreadable file: {error}"));
                continue;
            }
        };
        let text = match String::from_utf8(contents) {
            Ok(text) => text,
            Err(_) => {
                scan.problems
                    .entry(filename)
                    .or_default()
                    .push("invalid UTF-8".to_owned());
                continue;
            }
        };
        for (line_number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let row = match serde_json::from_str::<Value>(line) {
                Ok(Value::Object(value)) => value,
                Ok(_) => {
                    scan.problems
                        .entry(filename.clone())
                        .or_default()
                        .push(format!("line {}: non-object row", line_number + 1));
                    continue;
                }
                Err(_) => {
                    scan.problems
                        .entry(filename.clone())
                        .or_default()
                        .push(format!("line {}: malformed JSON", line_number + 1));
                    continue;
                }
            };
            let Some(timestamp) = row.get("ts").and_then(Value::as_i64) else {
                let detail = if row.contains_key("ts") {
                    "non-integer ts"
                } else {
                    "missing ts"
                };
                scan.problems
                    .entry(filename.clone())
                    .or_default()
                    .push(format!("line {}: {detail}", line_number + 1));
                continue;
            };
            if timestamp < cutoff || timestamp > generated_at {
                continue;
            }
            let succeeded = !row.contains_key("status")
                || matches!(
                    row.get("status").and_then(Value::as_str),
                    Some("ok" | "completed")
                );
            if !succeeded {
                scan.failures += 1;
            }
            if row.get("degraded").is_some_and(truthy) {
                scan.degraded += 1;
                scan.degraded_rows.push((timestamp, row));
            }
        }
    }
    scan.degraded_rows
        .sort_by_key(|(timestamp, _)| std::cmp::Reverse(*timestamp));
    Ok(scan)
}

fn build_consumer_signal_health(journal_root: &Path) -> Result<ConsumerSignalHealth, HealthError> {
    let entities = load_all_journal_entities(journal_root)
        .map_err(|error| HealthError::internal(error.to_string()))?;
    Ok(ConsumerSignalHealth {
        profile_entities_total: entities.len() as u64,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimeWindowSettings {
    pub(crate) enabled: bool,
    pub(crate) start: NaiveTime,
    pub(crate) end: NaiveTime,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayPowersaveSettings {
    pub(crate) enabled: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GateSettings {
    pub(crate) time_window: TimeWindowSettings,
    pub(crate) display_powersave: DisplayPowersaveSettings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessingSettings {
    pub(crate) deferred: bool,
    pub(crate) gate: GateSettings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayPowersaveReading {
    pub(crate) available: bool,
    pub(crate) asleep: bool,
    pub(crate) debounced: bool,
}
impl DisplayPowersaveReading {
    pub(crate) const UNAVAILABLE: Self = Self {
        available: false,
        asleep: false,
        debounced: false,
    };
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConditionState {
    pub(crate) enabled: bool,
    pub(crate) available: bool,
    pub(crate) open: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateState {
    pub(crate) open: bool,
    pub(crate) time_window: ConditionState,
    pub(crate) display_powersave: ConditionState,
}

pub(crate) fn evaluate_time_window(settings: TimeWindowSettings, now: NaiveTime) -> ConditionState {
    let open = if settings.start < settings.end {
        now >= settings.start && now < settings.end
    } else if settings.start > settings.end {
        now >= settings.start || now < settings.end
    } else {
        false
    };
    ConditionState {
        enabled: settings.enabled,
        available: true,
        open,
    }
}
pub(crate) fn evaluate_display_powersave(
    settings: DisplayPowersaveSettings,
    reading: DisplayPowersaveReading,
) -> ConditionState {
    if !settings.enabled {
        return ConditionState {
            enabled: false,
            available: false,
            open: false,
        };
    }
    ConditionState {
        enabled: true,
        available: reading.available,
        open: reading.asleep && reading.debounced,
    }
}
pub(crate) fn evaluate_drain_gate(
    settings: ProcessingSettings,
    now: NaiveTime,
    reading: DisplayPowersaveReading,
) -> GateState {
    let time_window = evaluate_time_window(settings.gate.time_window, now);
    let display_powersave = evaluate_display_powersave(settings.gate.display_powersave, reading);
    GateState {
        open: [time_window, display_powersave]
            .into_iter()
            .any(|condition| condition.enabled && condition.available && condition.open),
        time_window,
        display_powersave,
    }
}
pub(crate) fn derive_drain_state(
    settings: ProcessingSettings,
    gate: &GateState,
    no_engine: bool,
) -> &'static str {
    if no_engine {
        "no_engine"
    } else if !settings.deferred {
        "realtime"
    } else if gate.open {
        "window_open"
    } else if [gate.time_window, gate.display_powersave]
        .into_iter()
        .any(|condition| condition.enabled && condition.available)
    {
        "waiting_for_window"
    } else {
        "no_active_condition"
    }
}

fn build_segment_backlog_health(
    journal_root: &Path,
    now: DateTime<Utc>,
    display_reading: DisplayPowersaveReading,
    display_detectable: bool,
    local_now: NaiveTime,
) -> Result<SegmentBacklogHealth, HealthError> {
    let mut not_thought = 0usize;
    let mut not_sensed = 0usize;
    let mut days_with_backlog = 0usize;
    let mut errors = Vec::new();
    let health_source = FilesystemHealthLogSource::new(journal_root);
    let segment_source = FilesystemSegmentSource;
    let updated_days = solstone_core_system::catchup::updated_days(journal_root, &BTreeSet::new())
        .map_err(|error| HealthError::internal(error.to_string()))?;
    for day in updated_days {
        let result = (|| {
            let progress =
                read_segment_progress(&health_source, &day).map_err(|error| error.to_string())?;
            let (_, _, segments) = scan_day(&segment_source, journal_root, &day, now)
                .map_err(|error| error.to_string())?;
            let inputs = segments
                .into_iter()
                .map(SegmentInput::from)
                .collect::<Vec<_>>();
            Ok::<_, String>(classify_segment_completion(&inputs, &progress.value))
        })();
        match result {
            Ok(completion) => {
                not_thought += completion.not_thought;
                not_sensed += completion.not_sensed;
                if completion.not_thought > 0 {
                    days_with_backlog += 1;
                }
            }
            Err(error) => errors.push(format!("{day}: {error}")),
        }
    }
    let config = solstone_core_thinking::read_config(journal_root)
        .map_err(|error| HealthError::internal(error.to_string()))?;
    let settings = processing_settings(&config);
    let gate = evaluate_drain_gate(settings, local_now, display_reading);
    let no_engine = no_thinking_engine_chosen(&config);
    let awaiting_total = not_sensed + not_thought;
    Ok(SegmentBacklogHealth {
        not_thought: not_thought as u64,
        days_with_backlog: days_with_backlog as u64,
        errors,
        not_sensed: not_sensed as u64,
        awaiting_analysis_text: if no_engine {
            Some(NO_ENGINE_ANALYSIS_TEXT.to_owned())
        } else if settings.deferred {
            Some(format!(
                "{awaiting_total} segments captured, awaiting analysis"
            ))
        } else {
            None
        },
        last_drained_at: read_last_drained_at(journal_root)?,
        drain_state: derive_drain_state(settings, &gate, no_engine).to_owned(),
        display_powersave_detectable: display_detectable,
    })
}

fn processing_settings(config: &Map<String, Value>) -> ProcessingSettings {
    let processing = config.get("processing").and_then(Value::as_object);
    let gate = processing
        .and_then(|value| value.get("gate"))
        .and_then(Value::as_object);
    let window = gate
        .and_then(|value| value.get("time_window"))
        .and_then(Value::as_object);
    let display = gate
        .and_then(|value| value.get("display_powersave"))
        .and_then(Value::as_object);
    ProcessingSettings {
        deferred: processing
            .and_then(|value| value.get("mode"))
            .and_then(Value::as_str)
            == Some("deferred"),
        gate: GateSettings {
            time_window: TimeWindowSettings {
                enabled: window
                    .and_then(|value| value.get("enabled"))
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                start: parse_time(
                    window
                        .and_then(|value| value.get("start"))
                        .and_then(Value::as_str),
                )
                .unwrap_or_else(|| NaiveTime::from_hms_opt(2, 0, 0).expect("valid default")),
                end: parse_time(
                    window
                        .and_then(|value| value.get("end"))
                        .and_then(Value::as_str),
                )
                .unwrap_or_else(|| NaiveTime::from_hms_opt(6, 0, 0).expect("valid default")),
            },
            display_powersave: DisplayPowersaveSettings {
                enabled: display
                    .and_then(|value| value.get("enabled"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        },
    }
}

fn no_thinking_engine_chosen(config: &Map<String, Value>) -> bool {
    config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|value| value.get("active"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("provider"))
        .and_then(Value::as_str)
        .is_none_or(|provider| provider.trim().is_empty())
}

fn read_last_drained_at(journal_root: &Path) -> Result<Option<i64>, HealthError> {
    let mut latest = None;
    for day in chronicle_days(journal_root)? {
        let marker = journal_root
            .join("chronicle")
            .join(day)
            .join("health/daily.updated");
        match fs::metadata(marker) {
            Ok(metadata) => {
                let timestamp = metadata
                    .modified()
                    .map_err(|error| HealthError::internal(error.to_string()))?
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| HealthError::internal(error.to_string()))?
                    .as_millis() as i64;
                latest = Some(latest.map_or(timestamp, |value: i64| value.max(timestamp)));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(HealthError::internal(error.to_string())),
        }
    }
    Ok(latest)
}

fn chronicle_days(journal_root: &Path) -> Result<Vec<String>, HealthError> {
    let root = journal_root.join("chronicle");
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(HealthError::internal(error.to_string())),
    };
    let mut days = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| HealthError::internal(error.to_string()))?
        .into_iter()
        .filter(|path| path.is_dir())
        .filter_map(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .filter(|value| resolve_day(value).is_ok())
        .collect::<Vec<_>>();
    days.sort();
    Ok(days)
}

fn parse_segment_bounds(raw: &str, day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let (clock, duration) = raw.split_once('_')?;
    if clock.len() != 6 || !clock.as_bytes().iter().all(u8::is_ascii_digit) {
        return None;
    }
    let hour = clock[0..2].parse().ok()?;
    let minute = clock[2..4].parse().ok()?;
    let second = clock[4..6].parse().ok()?;
    let duration = duration.parse::<i64>().ok()?;
    let start = day.and_hms_opt(hour, minute, second)?;
    let start = DateTime::<Utc>::from_naive_utc_and_offset(start, Utc);
    Some((start, start + Duration::seconds(duration)))
}

fn parse_start(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|value| DateTime::<Utc>::from_naive_utc_and_offset(value, Utc))
        })
}

fn parse_time(value: Option<&str>) -> Option<NaiveTime> {
    value.and_then(|value| NaiveTime::parse_from_str(value, "%H:%M").ok())
}
fn values(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}
fn string(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
fn days_inclusive(range: (NaiveDate, NaiveDate)) -> Vec<NaiveDate> {
    let mut days = Vec::new();
    let mut current = range.0;
    while current <= range.1 {
        days.push(current);
        current += Duration::days(1);
    }
    days
}
fn talent_index_filename(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("jsonl")
        && path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|value| resolve_day(value).is_ok())
}
fn severity_rank(value: &str) -> u8 {
    match value {
        "critical" => 0,
        "warn" => 1,
        "info" => 2,
        _ => 99,
    }
}
fn note(
    severity: &str,
    category: &str,
    message: &str,
    detected_at: i64,
    detail_pointer: Option<&str>,
) -> HealthNote {
    HealthNote {
        severity: severity.to_owned(),
        category: category.to_owned(),
        message: message.to_owned(),
        detected_at,
        detail_pointer: detail_pointer.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use chrono::{Duration, NaiveTime, TimeZone, Timelike, Utc};
    use filetime::{FileTime, set_file_mtime};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{
        DisplayPowersaveReading, DisplayPowersaveSettings, GateSettings, HealthError,
        ProcessingSettings, ScanAggregate, TimeWindowSettings, build_capture_health,
        build_consumer_signal_health, build_health_report, build_segment_backlog_health,
        build_synthesis_health, derive_drain_state, evaluate_display_powersave,
        evaluate_drain_gate, read_last_drained_at, resolve_range, scan_records,
        scan_talent_indexes,
    };

    fn temporary() -> TempDir {
        let temporary = TempDir::new_in("/var/tmp").expect("temporary journal");
        for directory in ["facets", "entities", "chronicle", "talents"] {
            fs::create_dir_all(temporary.path().join(directory)).expect("journal directory");
        }
        temporary
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0).unwrap()
    }

    fn write_json(path: &Path, value: Value) {
        fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
        fs::write(
            path,
            format!("{}\n", serde_json::to_string(&value).unwrap()),
        )
        .expect("json");
    }

    fn write_jsonl(path: &Path, rows: &[Value]) {
        fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
        let text = rows
            .iter()
            .map(|row| serde_json::to_string(row).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{text}\n")).expect("jsonl");
    }

    fn facet(root: &Path, name: &str) {
        write_json(
            &root.join("facets").join(name).join("facet.json"),
            json!({"name":name}),
        );
    }

    fn activities(root: &Path, facet: &str, day: &str, rows: &[Value]) {
        write_jsonl(
            &root
                .join("facets")
                .join(facet)
                .join("activities")
                .join(format!("{day}.jsonl")),
            rows,
        );
    }

    fn talent_rows(root: &Path, day: &str, rows: &[Value]) {
        write_jsonl(&root.join("talents").join(format!("{day}.jsonl")), rows);
    }

    fn healthy_talent_guards(root: &Path, now: chrono::DateTime<Utc>) {
        for day in [now.date_naive(), now.date_naive() - Duration::days(1)] {
            talent_rows(
                root,
                &day.format("%Y%m%d").to_string(),
                &[json!({"ts":now.timestamp_millis(),"status":"ok"})],
            );
        }
    }

    fn screen_segment(root: &Path, day: &str, segment: &str) {
        let directory = root.join("chronicle").join(day).join(segment);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("screen.jsonl"), "{}\n{\"timestamp\":0}\n").unwrap();
    }

    fn health_log(root: &Path, day: &str, rows: &[Value]) {
        write_jsonl(
            &root.join("chronicle").join(day).join("health/001.jsonl"),
            rows,
        );
    }

    fn mark_updated(root: &Path, day: &str) {
        let marker = root
            .join("chronicle")
            .join(day)
            .join("health/stream.updated");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(marker, "updated\n").unwrap();
    }

    fn configured(root: &Path, processing: Value) {
        write_json(
            &root.join("config/journal.json"),
            json!({"providers":{"active":{"provider":"test"}},"processing":processing}),
        );
    }

    fn note_severities(notes: &[super::HealthNote]) -> Vec<&str> {
        notes.iter().map(|note| note.severity.as_str()).collect()
    }

    #[test]
    fn default_range_uses_utc_day() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 3, 1, 0, 1, 0).unwrap();
        let range = resolve_range(None, None, now).unwrap();
        assert_eq!(range.0.format("%Y%m%d").to_string(), "20260223");
        assert_eq!(range.1.format("%Y%m%d").to_string(), "20260301");
    }

    #[test]
    fn utc_day_default_never_uses_host_local_calendar() {
        let fixed = Utc.with_ymd_and_hms(2026, 4, 1, 23, 30, 0).unwrap();
        let range = resolve_range(None, None, fixed).unwrap();
        assert_eq!(range.1.format("%Y%m%d").to_string(), "20260401");
    }

    #[test]
    fn range_validation_rejects_one_sided_malformed_and_inverted_windows() {
        let fixed = now();
        assert!(matches!(
            resolve_range(Some("20260401"), None, fixed),
            Err(HealthError::InvalidRequest(message)) if message == "both endpoints or neither"
        ));
        assert!(matches!(
            resolve_range(Some("bad"), Some("20260401"), fixed),
            Err(HealthError::InvalidRequest(message)) if message == "day must match YYYYMMDD"
        ));
        assert!(matches!(
            resolve_range(Some("20260402"), Some("20260401"), fixed),
            Err(HealthError::InvalidRequest(message)) if message == "day_from must be <= day_to"
        ));
    }

    #[test]
    fn capture_and_visible_activity_folds_read_real_records() {
        let temporary = temporary();
        let root = temporary.path();
        facet(root, "work");
        activities(
            root,
            "work",
            "20260409",
            &[
                json!({"id":"visible","segments":["230000_7200"],"participation":"yes","story":{"text":"story"},"edits":[{"actor":"cli:owner"}],"source":"anticipated","start":"2026-04-09T10:00:00Z"}),
                json!({"id":"hidden","hidden":true,"participation":true,"story":"hidden"}),
                json!({"id":"falsey","participation":false,"story":"","edits":[{"actor":"system"}],"source":"anticipated","start":"2026-04-11T10:00:00Z"}),
                json!({"id":"cancelled","source":"anticipated","cancelled":true,"start":"2026-04-09T10:00:00Z"}),
            ],
        );
        activities(
            root,
            "work",
            "20260410",
            &[json!({"id":"today","segments":["010000_3600"]})],
        );
        let aggregate = scan_records(
            root,
            &["work".to_owned()],
            (now().date_naive() - Duration::days(1), now().date_naive()),
            now(),
        )
        .unwrap();
        assert_eq!(
            aggregate.capture_hour_slots.len(),
            2,
            "midnight spill is clipped"
        );
        assert_eq!(aggregate.activities_count, 4);
        assert_eq!(aggregate.activities_with_participation, 1);
        assert_eq!(aggregate.activities_with_story, 1);
        assert_eq!(aggregate.activities_user_edited, 1);
        assert_eq!(aggregate.activities_anticipated_unfilled, 1);
        let (capture, _) = build_capture_health(
            root,
            &aggregate,
            (now().date_naive() - Duration::days(1), now().date_naive()),
            &["work".to_owned()],
            now().timestamp_millis(),
            now(),
        )
        .unwrap();
        assert_eq!(capture.hours_with_capture, 2);
        assert_eq!(capture.hours_total, 48);
        assert_eq!(
            capture.last_segment_at,
            Some(
                Utc.with_ymd_and_hms(2026, 4, 10, 2, 0, 0)
                    .unwrap()
                    .timestamp_millis()
            )
        );
    }

    #[test]
    fn hidden_activity_records_do_not_contribute_to_visible_counts() {
        let temporary = temporary();
        facet(temporary.path(), "work");
        activities(
            temporary.path(),
            "work",
            "20260410",
            &[
                json!({"id":"hidden","hidden":true,"participation":true,"story":"x","edits":[{"actor":"owner"}]}),
            ],
        );
        let aggregate = scan_records(
            temporary.path(),
            &["work".to_owned()],
            (now().date_naive(), now().date_naive()),
            now(),
        )
        .unwrap();
        assert_eq!(aggregate.activities_count, 0);
        assert_eq!(aggregate.activities_with_participation, 0);
        assert_eq!(aggregate.activities_with_story, 0);
        assert_eq!(aggregate.activities_user_edited, 0);
    }

    #[test]
    fn participation_and_story_follow_json_truthiness() {
        let temporary = temporary();
        facet(temporary.path(), "work");
        activities(
            temporary.path(),
            "work",
            "20260410",
            &[
                json!({"id":"truthy","participation":1,"story":["x"]}),
                json!({"id":"falsey","participation":0,"story":{}}),
            ],
        );
        let aggregate = scan_records(
            temporary.path(),
            &["work".to_owned()],
            (now().date_naive(), now().date_naive()),
            now(),
        )
        .unwrap();
        assert_eq!(aggregate.activities_with_participation, 1);
        assert_eq!(aggregate.activities_with_story, 1);
    }

    #[test]
    fn only_cli_owner_and_user_edit_prefixes_are_user_edits() {
        let temporary = temporary();
        facet(temporary.path(), "work");
        activities(
            temporary.path(),
            "work",
            "20260410",
            &[
                json!({"id":"cli","edits":[{"actor":"cli:health"}]}),
                json!({"id":"owner","edits":[{"actor":"owner"}]}),
                json!({"id":"user","edits":[{"actor":"user:1"}]}),
                json!({"id":"system","edits":[{"actor":"scheduler"}]}),
            ],
        );
        let aggregate = scan_records(
            temporary.path(),
            &["work".to_owned()],
            (now().date_naive(), now().date_naive()),
            now(),
        )
        .unwrap();
        assert_eq!(aggregate.activities_user_edited, 3);
    }

    #[test]
    fn multiple_qualifying_edits_still_count_as_one_edited_activity() {
        let temporary = temporary();
        facet(temporary.path(), "work");
        activities(
            temporary.path(),
            "work",
            "20260410",
            &[
                json!({"id":"multiple","edits":[{"actor":"cli:health"},{"actor":"owner"},{"actor":"scheduler"}]}),
            ],
        );
        let aggregate = scan_records(
            temporary.path(),
            &["work".to_owned()],
            (now().date_naive(), now().date_naive()),
            now(),
        )
        .unwrap();
        assert_eq!(aggregate.activities_user_edited, 1);
    }

    #[test]
    fn anticipated_unfilled_excludes_future_and_cancelled_rows() {
        let temporary = temporary();
        facet(temporary.path(), "work");
        activities(
            temporary.path(),
            "work",
            "20260410",
            &[
                json!({"id":"past","source":"anticipated","start":"2026-04-10T12:00:00Z"}),
                json!({"id":"future","source":"anticipated","start":"2026-04-10T12:00:01Z"}),
                json!({"id":"cancelled","source":"anticipated","start":"2026-04-09T12:00:00Z","cancelled":true}),
            ],
        );
        let aggregate = scan_records(
            temporary.path(),
            &["work".to_owned()],
            (now().date_naive(), now().date_naive()),
            now(),
        )
        .unwrap();
        assert_eq!(aggregate.activities_anticipated_unfilled, 1);
    }

    fn capture_notes_for_gap(hours: i64) -> Vec<super::HealthNote> {
        let temporary = temporary();
        let root = temporary.path();
        facet(root, "work");
        let captured = now() - Duration::hours(hours) - Duration::minutes(1);
        activities(
            root,
            "work",
            &captured.format("%Y%m%d").to_string(),
            &[
                json!({"id":"old","segments":[format!("{:02}{:02}{:02}_1", captured.hour(), captured.minute(), captured.second())]}),
            ],
        );
        let (_, notes) = build_capture_health(
            root,
            &ScanAggregate::default(),
            (now().date_naive(), now().date_naive()),
            &["work".to_owned()],
            now().timestamp_millis(),
            now(),
        )
        .unwrap();
        notes
    }

    #[test]
    fn facet_silence_under_twenty_four_hours_has_no_facet_note() {
        let notes = capture_notes_for_gap(23);
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn facet_silence_at_twenty_four_hours_is_info() {
        assert_eq!(
            note_severities(&capture_notes_for_gap(24)),
            ["info", "info"]
        );
    }

    #[test]
    fn facet_silence_at_seventy_two_hours_is_warn() {
        assert_eq!(
            note_severities(&capture_notes_for_gap(72)),
            ["info", "warn"]
        );
    }

    #[test]
    fn facet_silence_at_one_hundred_sixty_eight_hours_is_critical() {
        assert_eq!(
            note_severities(&capture_notes_for_gap(168)),
            ["info", "critical"]
        );
    }

    #[test]
    fn facet_silence_emits_one_highest_severity_note_per_facet() {
        let notes = capture_notes_for_gap(168);
        assert_eq!(
            notes
                .iter()
                .filter(|note| note.message.starts_with("work:"))
                .count(),
            1
        );
    }

    #[test]
    fn missing_talent_guard_days_fail_closed_with_info_note() {
        let temporary = temporary();
        let (health, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.talent_run_failures_24h, None);
        assert_eq!(health.talent_degraded_outputs_24h, None);
        assert!(
            notes
                .iter()
                .any(|note| note.message.contains("logs missing"))
        );
    }

    #[test]
    fn healthy_talent_indexes_count_failures_and_degraded_outputs_separately() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        talent_rows(
            temporary.path(),
            "20260410",
            &[
                json!({"ts":now().timestamp_millis(),"status":"failed"}),
                json!({"ts":now().timestamp_millis(),"status":"completed","degraded":{"output_tokens":0,"provider":"p","model":"m"},"name":"quiet","day":"20260410"}),
            ],
        );
        let (health, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.talent_run_failures_24h, Some(1));
        assert_eq!(health.talent_degraded_outputs_24h, Some(1));
        assert!(
            notes
                .iter()
                .any(|note| note.message.contains("finished near-empty"))
        );
        assert!(
            !notes
                .iter()
                .any(|note| note.message.contains("counts unavailable"))
        );
    }

    #[test]
    fn non_string_talent_status_is_a_failure() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        talent_rows(
            temporary.path(),
            "20260410",
            &[json!({"ts":now().timestamp_millis(),"status":false})],
        );
        let scan = scan_talent_indexes(&temporary.path().join("talents"), now().timestamp_millis())
            .unwrap();
        assert_eq!(scan.failures, 1);
    }

    #[test]
    fn degraded_talent_notes_keep_the_ten_newest_rows() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        let rows = (0..12)
            .map(|offset| {
                json!({
                    "ts":now().timestamp_millis() - offset * 1_000,
                    "status":"completed",
                    "degraded":{"output_tokens":0,"provider":"p","model":"m"},
                    "name":format!("degraded-{offset:02}"),
                    "day":"20260410"
                })
            })
            .collect::<Vec<_>>();
        talent_rows(temporary.path(), "20260410", &rows);
        let (_, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        let messages = notes
            .iter()
            .filter(|note| note.message.contains("finished near-empty"))
            .map(|note| note.message.as_str())
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), 10);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("degraded-00"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("degraded-09"))
        );
        assert!(
            !messages
                .iter()
                .any(|message| message.contains("degraded-10"))
        );
        assert!(
            !messages
                .iter()
                .any(|message| message.contains("degraded-11"))
        );
    }

    #[test]
    fn talent_failure_and_degraded_counts_follow_timestamps_across_day_indexes() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        talent_rows(
            temporary.path(),
            "20260410",
            &[json!({"ts":now().timestamp_millis(),"status":"failed"})],
        );
        talent_rows(
            temporary.path(),
            "20260409",
            &[
                json!({"ts":now().timestamp_millis() - 1,"status":"completed","degraded":{"output_tokens":0},"name":"quiet","day":"20260409"}),
            ],
        );
        let (health, _) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.talent_run_failures_24h, Some(1));
        assert_eq!(health.talent_degraded_outputs_24h, Some(1));
    }

    #[test]
    fn talent_timestamp_at_twenty_four_hour_boundary_is_included() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        talent_rows(
            temporary.path(),
            "20260409",
            &[json!({"ts":now().timestamp_millis() - 86_400_000,"status":"failed"})],
        );
        let scan = scan_talent_indexes(&temporary.path().join("talents"), now().timestamp_millis())
            .unwrap();
        assert_eq!(scan.failures, 1);
    }

    #[test]
    fn malformed_talent_rows_fail_closed() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        fs::write(temporary.path().join("talents/20260408.jsonl"), "[]\n").unwrap();
        let (health, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.talent_run_failures_24h, None);
        assert!(
            notes
                .iter()
                .any(|note| note.message.contains("non-object row"))
        );
    }

    #[test]
    fn talent_rows_without_integer_timestamps_fail_closed() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        talent_rows(temporary.path(), "20260408", &[json!({"status":"failed"})]);
        let (health, _) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.talent_run_failures_24h, None);
    }

    #[test]
    fn talent_rows_after_the_report_clock_are_excluded() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        talent_rows(
            temporary.path(),
            "20260410",
            &[json!({"ts":now().timestamp_millis() + 1,"status":"failed"})],
        );
        let scan = scan_talent_indexes(&temporary.path().join("talents"), now().timestamp_millis())
            .unwrap();
        assert_eq!(scan.failures, 0);
    }

    #[test]
    fn corrupt_talent_index_fails_closed_with_warning() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        fs::write(temporary.path().join("talents/20260408.jsonl"), b"{").unwrap();
        let (health, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.talent_run_failures_24h, None);
        assert!(notes.iter().any(
            |note| note.severity == "warn" && note.message.contains("could not be fully read")
        ));
    }

    #[test]
    fn invalid_utf8_talent_index_fails_closed() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        fs::write(temporary.path().join("talents/20260408.jsonl"), [0xff]).unwrap();
        let (health, _) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.talent_degraded_outputs_24h, None);
    }

    #[test]
    fn unreadable_talent_index_fails_closed() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        fs::create_dir(temporary.path().join("talents/20260408.jsonl")).unwrap();
        let (health, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.talent_run_failures_24h, None);
        assert!(
            notes
                .iter()
                .any(|note| note.message.contains("could not be fully read"))
        );
    }

    #[test]
    fn non_day_talent_files_are_ignored_even_when_corrupt() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        fs::write(temporary.path().join("talents/current.jsonl"), b"{").unwrap();
        let (health, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.talent_run_failures_24h, Some(0));
        assert!(
            !notes
                .iter()
                .any(|note| note.message.contains("current.jsonl"))
        );
    }

    #[test]
    fn missing_indexer_is_warned_and_has_null_timestamp() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        let (health, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert_eq!(health.indexer_last_rebuild_at, None);
        assert!(
            notes
                .iter()
                .any(|note| note.message.contains("indexer database missing"))
        );
    }

    #[test]
    fn indexer_at_seven_days_is_not_stale_but_older_is_warned() {
        let temporary = temporary();
        healthy_talent_guards(temporary.path(), now());
        let index = temporary.path().join("indexer/journal.sqlite");
        fs::create_dir_all(index.parent().unwrap()).unwrap();
        fs::write(&index, "sqlite").unwrap();
        set_file_mtime(
            &index,
            FileTime::from_unix_time((now().timestamp() - 7 * 86_400) as _, 0),
        )
        .unwrap();
        let (_, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert!(
            !notes
                .iter()
                .any(|note| note.message.contains("last rebuilt"))
        );
        set_file_mtime(
            &index,
            FileTime::from_unix_time((now().timestamp() - 7 * 86_400 - 1) as _, 0),
        )
        .unwrap();
        let (_, notes) =
            build_synthesis_health(temporary.path(), &ScanAggregate::default(), now()).unwrap();
        assert!(
            notes
                .iter()
                .any(|note| note.message.contains("last rebuilt"))
        );
    }

    #[test]
    fn last_drained_timestamp_uses_the_latest_daily_marker() {
        let temporary = temporary();
        for (day, timestamp) in [("20260408", 1_000_i64), ("20260409", 2_000_i64)] {
            let marker = temporary
                .path()
                .join("chronicle")
                .join(day)
                .join("health/daily.updated");
            fs::create_dir_all(marker.parent().unwrap()).unwrap();
            fs::write(&marker, "updated\n").unwrap();
            set_file_mtime(
                &marker,
                FileTime::from_unix_time(timestamp / 1_000, ((timestamp % 1_000) * 1_000_000) as _),
            )
            .unwrap();
        }
        assert_eq!(read_last_drained_at(temporary.path()).unwrap(), Some(2_000));
    }

    #[test]
    fn display_powersave_condition_requires_enabled_available_sleeping_and_debounced() {
        assert!(
            !evaluate_display_powersave(
                DisplayPowersaveSettings { enabled: false },
                DisplayPowersaveReading {
                    available: true,
                    asleep: true,
                    debounced: true
                }
            )
            .open
        );
        assert!(
            !evaluate_display_powersave(
                DisplayPowersaveSettings { enabled: true },
                DisplayPowersaveReading {
                    available: false,
                    asleep: true,
                    debounced: true
                }
            )
            .available
        );
        assert!(
            !evaluate_display_powersave(
                DisplayPowersaveSettings { enabled: true },
                DisplayPowersaveReading {
                    available: true,
                    asleep: true,
                    debounced: false
                }
            )
            .open
        );
        assert!(
            evaluate_display_powersave(
                DisplayPowersaveSettings { enabled: true },
                DisplayPowersaveReading {
                    available: true,
                    asleep: true,
                    debounced: true
                }
            )
            .open
        );
    }

    #[test]
    fn consumer_signal_counts_real_entity_files() {
        let temporary = temporary();
        let root = temporary.path();
        write_json(
            &root.join("entities/a/entity.json"),
            json!({"id":"a","name":"A"}),
        );
        write_json(
            &root.join("entities/b/entity.json"),
            json!({"id":"b","name":"B"}),
        );
        let health = build_consumer_signal_health(root).unwrap();
        assert_eq!(health.profile_entities_total, 2);
    }

    #[test]
    fn malformed_journal_config_is_an_internal_report_error() {
        let temporary = temporary();
        fs::create_dir_all(temporary.path().join("config")).unwrap();
        fs::write(temporary.path().join("config/journal.json"), "{").unwrap();
        assert!(matches!(
            build_health_report(
                temporary.path(),
                (now().date_naive(), now().date_naive()),
                now(),
            ),
            Err(HealthError::Internal { .. })
        ));
    }

    #[test]
    fn segment_backlog_counts_not_thought_and_not_sensed_from_real_progress() {
        let temporary = temporary();
        let root = temporary.path();
        let day = "20260409";
        screen_segment(root, day, "120000_60");
        mark_updated(root, day);
        health_log(
            root,
            day,
            &[
                json!({"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}),
            ],
        );
        let health = build_segment_backlog_health(
            root,
            now(),
            DisplayPowersaveReading::UNAVAILABLE,
            false,
            NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(health.not_thought, 1);
        assert_eq!(health.not_sensed, 0);
        assert_eq!(health.days_with_backlog, 1);
        assert!(health.errors.is_empty());
    }

    #[test]
    fn segment_backlog_reports_caught_up_day() {
        let temporary = temporary();
        let root = temporary.path();
        let day = "20260409";
        screen_segment(root, day, "120000_60");
        mark_updated(root, day);
        health_log(
            root,
            day,
            &[
                json!({"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}),
                json!({"event":"talent.complete","ts":2,"mode":"segment","stream":"_default","segment":"120000_60","name":"documents"}),
            ],
        );
        let health = build_segment_backlog_health(
            root,
            now(),
            DisplayPowersaveReading::UNAVAILABLE,
            false,
            NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(
            (
                health.not_thought,
                health.not_sensed,
                health.days_with_backlog
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn segment_backlog_keeps_a_per_day_scan_failure_without_aborting_other_days() {
        let temporary = temporary();
        let root = temporary.path();
        let bad_day = "20260408";
        let good_day = "20260409";
        let bad = root
            .join("chronicle")
            .join(bad_day)
            .join("_default")
            .join("120000_60");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("screen.jsonl"), "{}\n{\"timestamp\":0}\n").unwrap();
        mark_updated(root, bad_day);
        screen_segment(root, good_day, "120000_60");
        mark_updated(root, good_day);
        health_log(
            root,
            good_day,
            &[
                json!({"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}),
            ],
        );
        let health = build_segment_backlog_health(
            root,
            now(),
            DisplayPowersaveReading::UNAVAILABLE,
            false,
            NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(health.not_thought, 1, "the good day still contributes");
        assert_eq!(health.errors.len(), 1);
        assert!(health.errors[0].starts_with("20260408:"));
    }

    #[test]
    fn deferred_backlog_awaiting_text_uses_unsensed_and_unthought_total() {
        let temporary = temporary();
        let root = temporary.path();
        configured(
            root,
            json!({"mode":"deferred","gate":{"time_window":{"enabled":true,"start":"02:00","end":"06:00"}}}),
        );
        let day = "20260409";
        let segment = root.join("chronicle").join(day).join("120000_60");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("screen.jsonl"), "{}\n").unwrap();
        mark_updated(root, day);
        let health = build_segment_backlog_health(
            root,
            now(),
            DisplayPowersaveReading::UNAVAILABLE,
            false,
            NaiveTime::from_hms_opt(7, 0, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(health.not_sensed, 1);
        assert_eq!(
            health.awaiting_analysis_text.as_deref(),
            Some("1 segments captured, awaiting analysis")
        );
    }

    #[test]
    fn segment_backlog_uses_the_injected_display_snapshot_and_detectability() {
        let temporary = temporary();
        let root = temporary.path();
        configured(
            root,
            json!({"mode":"deferred","gate":{"time_window":{"enabled":false},"display_powersave":{"enabled":true}}}),
        );
        let health = build_segment_backlog_health(
            root,
            now(),
            DisplayPowersaveReading {
                available: true,
                asleep: true,
                debounced: true,
            },
            true,
            NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
        )
        .unwrap();
        assert_eq!(health.drain_state, "window_open");
        assert!(health.display_powersave_detectable);
    }

    #[test]
    fn segment_backlog_orchestration_uses_each_drain_state() {
        let temporary = temporary();
        let root = temporary.path();
        let local = NaiveTime::from_hms_opt(3, 0, 0).unwrap();
        let unavailable = DisplayPowersaveReading::UNAVAILABLE;
        let no_engine =
            build_segment_backlog_health(root, now(), unavailable, false, local).unwrap();
        assert_eq!(no_engine.drain_state, "no_engine");
        configured(root, json!({"mode":"realtime"}));
        assert_eq!(
            build_segment_backlog_health(root, now(), unavailable, false, local)
                .unwrap()
                .drain_state,
            "realtime"
        );
        configured(
            root,
            json!({"mode":"deferred","gate":{"time_window":{"enabled":true,"start":"02:00","end":"06:00"}}}),
        );
        assert_eq!(
            build_segment_backlog_health(root, now(), unavailable, false, local)
                .unwrap()
                .drain_state,
            "window_open"
        );
        assert_eq!(
            build_segment_backlog_health(
                root,
                now(),
                unavailable,
                false,
                NaiveTime::from_hms_opt(7, 0, 0).unwrap()
            )
            .unwrap()
            .drain_state,
            "waiting_for_window"
        );
        configured(
            root,
            json!({"mode":"deferred","gate":{"time_window":{"enabled":false}}}),
        );
        assert_eq!(
            build_segment_backlog_health(root, now(), unavailable, false, local)
                .unwrap()
                .drain_state,
            "no_active_condition"
        );
    }

    #[test]
    fn notes_are_sorted_by_severity_then_category_then_message() {
        let temporary = temporary();
        let root = temporary.path();
        facet(root, "zeta");
        facet(root, "alpha");
        let report =
            build_health_report(root, (now().date_naive(), now().date_naive()), now()).unwrap();
        let keys = report
            .notes
            .iter()
            .map(|note| {
                (
                    note.severity.as_str(),
                    note.category.as_str(),
                    note.message.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert!(keys.windows(2).all(|pair| {
            let rank = |severity: &str| match severity {
                "critical" => 0,
                "warn" => 1,
                "info" => 2,
                _ => 3,
            };
            (rank(pair[0].0), pair[0].1, pair[0].2) <= (rank(pair[1].0), pair[1].1, pair[1].2)
        }));
    }

    #[test]
    fn drain_gate_state_is_pure_and_injectable() {
        let settings = ProcessingSettings {
            deferred: true,
            gate: GateSettings {
                time_window: TimeWindowSettings {
                    enabled: true,
                    start: NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
                    end: NaiveTime::from_hms_opt(6, 0, 0).unwrap(),
                },
                display_powersave: DisplayPowersaveSettings { enabled: true },
            },
        };
        let gate = evaluate_drain_gate(
            settings,
            NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
            DisplayPowersaveReading::UNAVAILABLE,
        );
        assert_eq!(derive_drain_state(settings, &gate, false), "window_open");
    }
}
