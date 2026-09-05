// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, NaiveDate, SecondsFormat};
use chrono_tz::Tz;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_body_source::{BodyRawRetention, BodySourceFamily, canonicalize, parse};

use crate::approval::{oura_approval, pin_journal_target};
use crate::bounded_file::open_regular_file;
use crate::bundle::{BodyIngestError, BodyIngestErrorKind, BodyIngestReport, NormalizedInput};
use crate::bundle::{RawAsset, publish};
use crate::oura_sync::hold_oura_lock;

const SCHEMA: &str = "solstone.health.oura.v1";
const MAX_SOURCE_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_SOURCE_TOTAL_BYTES: usize = 128 * 1024 * 1024;
const MAX_SOURCE_ITEMS: usize = 100_000;

pub const OURA_SYNC_ENDPOINTS: [&str; 13] = [
    "daily_readiness",
    "daily_sleep",
    "daily_stress",
    "daily_resilience",
    "daily_spo2",
    "sleep",
    "daily_activity",
    "heartrate",
    "daily_cardiovascular_age",
    "workout",
    "session",
    "enhanced_tag",
    "vO2_max",
];

const ALL_ENDPOINTS: [&str; 14] = [
    "blood_glucose",
    "daily_activity",
    "daily_cardiovascular_age",
    "daily_readiness",
    "daily_resilience",
    "daily_sleep",
    "daily_spo2",
    "daily_stress",
    "enhanced_tag",
    "heartrate",
    "session",
    "sleep",
    "vO2_max",
    "workout",
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OuraDocuments {
    items: BTreeMap<String, Vec<Value>>,
    pages: BTreeMap<String, Vec<Value>>,
}

impl OuraDocuments {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(&mut self, endpoint: String, items: Vec<Value>, pages: Vec<Value>) {
        self.items.insert(endpoint.clone(), items);
        self.pages.insert(endpoint, pages);
    }

    fn items(&self) -> impl Iterator<Item = (&String, &Vec<Value>)> {
        self.items.iter()
    }

    pub(crate) fn endpoint_items(&self, endpoint: &str) -> &[Value] {
        self.items.get(endpoint).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OuraNormalizedRow {
    row: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OuraImportOptions {
    pub timezone: String,
    pub confirm_body_save: bool,
    pub scheduled: bool,
    pub force: bool,
}

impl Default for OuraImportOptions {
    fn default() -> Self {
        Self {
            timezone: "UTC".to_owned(),
            confirm_body_save: false,
            scheduled: false,
            force: false,
        }
    }
}

impl OuraNormalizedRow {
    pub fn row(&self) -> &Map<String, Value> {
        &self.row
    }
}

pub fn parse_oura_source(path: &Path) -> Result<OuraDocuments, BodyIngestError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| source("path"))?;
    if metadata.file_type().is_file() {
        let endpoint = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| source("endpoint"))?;
        let mut documents = OuraDocuments::new();
        let bytes = read_document(path)?;
        let (items, page) = parse_page(endpoint, &bytes)?;
        if items.len() > MAX_SOURCE_ITEMS {
            return Err(source("item_budget"));
        }
        documents.insert(endpoint.to_owned(), items, vec![page]);
        return Ok(documents);
    }
    if !metadata.file_type().is_dir() {
        return Err(source("path"));
    }
    let mut documents = OuraDocuments::new();
    let mut total_bytes = 0_usize;
    let mut total_items = 0_usize;
    for endpoint in ALL_ENDPOINTS {
        let candidate = path.join(format!("{endpoint}.json"));
        if candidate.is_file() {
            let bytes = read_document(&candidate)?;
            total_bytes = total_bytes
                .checked_add(bytes.len())
                .filter(|total| *total <= MAX_SOURCE_TOTAL_BYTES)
                .ok_or_else(|| source("response_budget"))?;
            let (items, page) = parse_page(endpoint, &bytes)?;
            total_items = total_items
                .checked_add(items.len())
                .filter(|total| *total <= MAX_SOURCE_ITEMS)
                .ok_or_else(|| source("item_budget"))?;
            documents.insert(endpoint.to_owned(), items, vec![page]);
        }
    }
    if documents.items.is_empty() {
        return Err(source("documents"));
    }
    Ok(documents)
}

fn read_document(path: &Path) -> Result<Vec<u8>, BodyIngestError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| source("read"))?;
    if !metadata.file_type().is_file()
        || metadata.len() > u64::try_from(MAX_SOURCE_DOCUMENT_BYTES).unwrap_or(u64::MAX)
    {
        return Err(source("document_size_limit"));
    }
    let file = open_regular_file(path).map_err(|_| source("read"))?;
    if !file
        .metadata()
        .map_err(|_| source("read"))?
        .file_type()
        .is_file()
    {
        return Err(source("read"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_SOURCE_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| source("read"))?;
    if bytes.len() > MAX_SOURCE_DOCUMENT_BYTES {
        return Err(source("document_size_limit"));
    }
    Ok(bytes)
}

pub fn preview_oura_source(
    source_path: &Path,
    timezone: &str,
) -> Result<BodyIngestReport, BodyIngestError> {
    let documents = parse_oura_source(source_path)?;
    let rows = normalize_oura_documents(&documents, timezone)?;
    let days = rows
        .iter()
        .filter_map(|row| {
            row.row
                .get("day")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    Ok(BodyIngestReport::preview(rows.len() as u64, days))
}

pub fn save_oura_source(
    source_path: &Path,
    journal: &Path,
    options: &OuraImportOptions,
) -> Result<BodyIngestReport, BodyIngestError> {
    let journal = pin_journal_target(journal)?;
    let journal = journal.as_path();
    oura_approval(journal, options.confirm_body_save, options.scheduled)?;
    let _lock = hold_oura_lock(journal)?;
    let retention = oura_approval(journal, options.confirm_body_save, options.scheduled)?;
    let documents = parse_oura_source(source_path)?;
    save_documents(journal, &documents, retention, options)
}

pub(crate) fn save_documents(
    journal: &Path,
    documents: &OuraDocuments,
    retention: BodyRawRetention,
    options: &OuraImportOptions,
) -> Result<BodyIngestReport, BodyIngestError> {
    let raw_root = (retention == BodyRawRetention::RetainParsed).then_some("oura");
    let rows = normalize_inputs(documents, &options.timezone, raw_root)?;
    let raw_assets = if retention == BodyRawRetention::RetainParsed {
        retained_pages(documents)?
    } else {
        Vec::new()
    };
    publish(
        journal,
        BodySourceFamily::OuraApi,
        source_hash(documents, &options.timezone)?,
        retention,
        rows,
        raw_assets,
        options.force,
    )
}

pub fn normalize_oura_documents(
    documents: &OuraDocuments,
    timezone: &str,
) -> Result<Vec<OuraNormalizedRow>, BodyIngestError> {
    Ok(normalize_inputs(documents, timezone, None)?
        .into_iter()
        .map(|input| OuraNormalizedRow { row: input.row })
        .collect())
}

pub(crate) fn normalize_inputs(
    documents: &OuraDocuments,
    timezone: &str,
    raw_root: Option<&str>,
) -> Result<Vec<NormalizedInput>, BodyIngestError> {
    let timezone = timezone.parse::<Tz>().map_err(|_| normalize("timezone"))?;
    let mut rows = Vec::new();
    for (endpoint, items) in documents.items() {
        if !ALL_ENDPOINTS.contains(&endpoint.as_str()) {
            return Err(source("endpoint"));
        }
        for (index, item) in items.iter().enumerate() {
            let object = item.as_object().ok_or_else(|| source("item"))?;
            let raw_locator = raw_root.map(|root| format!("{root}/{endpoint}.jsonl#{}", index + 1));
            normalize_item(endpoint, object, timezone, raw_locator, &mut rows)?;
        }
    }
    Ok(rows)
}

fn parse_page(endpoint: &str, bytes: &[u8]) -> Result<(Vec<Value>, Value), BodyIngestError> {
    if !ALL_ENDPOINTS.contains(&endpoint) {
        return Err(source("endpoint"));
    }
    let document: Value = serde_json::from_slice(bytes).map_err(|_| source("json"))?;
    let data = document
        .as_object()
        .and_then(|object| object.get("data"))
        .and_then(Value::as_array)
        .ok_or_else(|| source("data"))?;
    for item in data {
        let object = item.as_object().ok_or_else(|| source("item"))?;
        if matches!(endpoint, "heartrate" | "blood_glucose") {
            let value_field = if endpoint == "heartrate" {
                "bpm"
            } else {
                "glucose"
            };
            if object.get("timestamp").and_then(Value::as_str).is_none()
                || object.get(value_field).is_none_or(Value::is_null)
            {
                return Err(source("series_fields"));
            }
        } else {
            let day_field = if endpoint == "enhanced_tag" {
                "start_day"
            } else {
                "day"
            };
            if object.get("id").and_then(Value::as_str).is_none()
                || object.get(day_field).and_then(Value::as_str).is_none()
            {
                return Err(source("document_fields"));
            }
        }
    }
    Ok((data.clone(), document))
}

fn normalize_item(
    endpoint: &str,
    item: &Map<String, Value>,
    timezone: Tz,
    raw_locator: Option<String>,
    rows: &mut Vec<NormalizedInput>,
) -> Result<(), BodyIngestError> {
    let day_field = if endpoint == "enhanced_tag" {
        "start_day"
    } else {
        "day"
    };
    let day = item
        .get(day_field)
        .and_then(Value::as_str)
        .map(parse_day)
        .transpose()?
        .unwrap_or_default();
    let timestamp_or_day = || {
        item.get("timestamp")
            .and_then(Value::as_str)
            .or_else(|| item.get(day_field).and_then(Value::as_str))
            .map(str::to_owned)
            .ok_or_else(|| source("timestamp"))
    };
    let mut add = |record_type: &str,
                   kind: &str,
                   source_record_id: String,
                   row_day: String,
                   start: String,
                   end: Option<String>,
                   value: Option<Value>,
                   unit: Option<&str>,
                   metadata: Map<String, Value>| {
        rows.push(row(
            record_type,
            kind,
            source_record_id,
            row_day,
            start,
            end,
            value,
            unit,
            metadata,
            raw_locator.clone(),
        ));
    };
    match endpoint {
        "daily_sleep" => add(
            "oura.daily_sleep",
            "daily_summary",
            required_string(item, "id")?,
            day,
            timestamp_or_day()?,
            None,
            item.get("score").cloned(),
            Some("score"),
            pick(item, &["contributors"]),
        ),
        "daily_readiness" => {
            let document_id = required_string(item, "id")?;
            let start = timestamp_or_day()?;
            add(
                "oura.daily_readiness",
                "daily_summary",
                document_id.clone(),
                day.clone(),
                start.clone(),
                None,
                item.get("score").cloned(),
                Some("score"),
                pick(item, &["contributors", "temperature_trend_deviation"]),
            );
            if item
                .get("temperature_deviation")
                .is_some_and(|value| !value.is_null())
            {
                add(
                    "oura.temperature_deviation",
                    "daily_summary",
                    format!("{document_id}/temperature_deviation"),
                    day,
                    start,
                    None,
                    item.get("temperature_deviation").cloned(),
                    Some("degC"),
                    Map::new(),
                );
            }
        }
        "daily_resilience" => add(
            "oura.daily_resilience",
            "daily_summary",
            required_string(item, "id")?,
            day,
            timestamp_or_day()?,
            None,
            item.get("level").cloned(),
            None,
            pick(item, &["contributors"]),
        ),
        "daily_stress" => add(
            "oura.daily_stress",
            "daily_summary",
            required_string(item, "id")?,
            day,
            timestamp_or_day()?,
            None,
            item.get("day_summary").cloned(),
            None,
            pick(item, &["stress_high", "recovery_high"]),
        ),
        "daily_spo2" => add(
            "oura.daily_spo2",
            "daily_summary",
            required_string(item, "id")?,
            day,
            timestamp_or_day()?,
            None,
            item.get("spo2_percentage")
                .and_then(Value::as_object)
                .and_then(|object| object.get("average"))
                .cloned(),
            Some("%"),
            pick(item, &["breathing_disturbance_index"]),
        ),
        "sleep" => add(
            "oura.sleep",
            "sleep_period",
            required_string(item, "id")?,
            day,
            optional_string(item, "bedtime_start").unwrap_or(timestamp_or_day()?),
            optional_string(item, "bedtime_end"),
            item.get("total_sleep_duration").cloned(),
            Some("s"),
            pick(
                item,
                &[
                    "type",
                    "deep_sleep_duration",
                    "rem_sleep_duration",
                    "light_sleep_duration",
                    "awake_time",
                    "time_in_bed",
                    "efficiency",
                    "latency",
                    "average_heart_rate",
                    "lowest_heart_rate",
                    "average_hrv",
                    "average_breath",
                    "sleep_phase_5_min",
                ],
            ),
        ),
        "daily_activity" => add(
            "oura.daily_activity",
            "daily_summary",
            required_string(item, "id")?,
            day,
            timestamp_or_day()?,
            None,
            item.get("score").cloned(),
            Some("score"),
            pick(
                item,
                &[
                    "contributors",
                    "steps",
                    "active_calories",
                    "total_calories",
                    "equivalent_walking_distance",
                    "high_activity_time",
                    "medium_activity_time",
                    "low_activity_time",
                    "sedentary_time",
                    "resting_time",
                    "non_wear_time",
                    "average_met_minutes",
                ],
            ),
        ),
        "heartrate" => {
            let timestamp = required_string(item, "timestamp")?;
            let local = owner_timestamp(&timestamp, timezone)?;
            let local_day = parse_day(&local[..10])?;
            let source_name =
                optional_string(item, "source").unwrap_or_else(|| "unknown".to_owned());
            add(
                "oura.heartrate",
                "sample",
                format!("heartrate/{timestamp}/{source_name}"),
                local_day,
                local,
                None,
                item.get("bpm").cloned(),
                Some("bpm"),
                Map::from_iter([
                    ("source".to_owned(), Value::String(source_name)),
                    ("raw_timestamp".to_owned(), Value::String(timestamp)),
                    (
                        "timezone".to_owned(),
                        Value::String(timezone.name().to_owned()),
                    ),
                ]),
            );
        }
        "daily_cardiovascular_age" => add(
            "oura.daily_cardiovascular_age",
            "daily_summary",
            required_string(item, "id")?,
            day,
            timestamp_or_day()?,
            None,
            item.get("vascular_age").cloned(),
            Some("years"),
            pick(item, &["pulse_wave_velocity"]),
        ),
        "blood_glucose" => {
            let timestamp = required_string(item, "timestamp")?;
            let local = owner_timestamp(&timestamp, timezone)?;
            let local_day = parse_day(&local[..10])?;
            let mut metadata = Map::from_iter([
                ("raw_timestamp".to_owned(), Value::String(timestamp.clone())),
                (
                    "timezone".to_owned(),
                    Value::String(timezone.name().to_owned()),
                ),
            ]);
            if let Some(source_name) = optional_string(item, "source") {
                metadata.insert("source".to_owned(), Value::String(source_name));
            }
            add(
                "oura.blood_glucose",
                "sample",
                format!("blood_glucose/{timestamp}"),
                local_day,
                local,
                None,
                item.get("glucose").cloned(),
                Some("mg/dL"),
                metadata,
            );
        }
        "workout" => add(
            "oura.workout",
            "workout",
            required_string(item, "id")?,
            day,
            optional_string(item, "start_datetime").unwrap_or(timestamp_or_day()?),
            optional_string(item, "end_datetime"),
            None,
            None,
            pick(
                item,
                &[
                    "activity",
                    "intensity",
                    "source",
                    "label",
                    "calories",
                    "distance",
                ],
            ),
        ),
        "session" => add(
            "oura.session",
            "session",
            required_string(item, "id")?,
            day,
            optional_string(item, "start_datetime").unwrap_or(timestamp_or_day()?),
            optional_string(item, "end_datetime"),
            None,
            None,
            pick(item, &["type", "mood"]),
        ),
        "enhanced_tag" => add(
            "oura.enhanced_tag",
            "tag",
            required_string(item, "id")?,
            day,
            optional_string(item, "start_time").unwrap_or(timestamp_or_day()?),
            optional_string(item, "end_time"),
            None,
            None,
            pick(
                item,
                &["tag_type_code", "comment", "custom_name", "end_day"],
            ),
        ),
        "vO2_max" => add(
            "oura.vo2_max",
            "daily_summary",
            required_string(item, "id")?,
            day,
            timestamp_or_day()?,
            None,
            item.get("vo2_max").cloned(),
            Some("mL/kg/min"),
            Map::new(),
        ),
        _ => return Err(source("endpoint")),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn row(
    record_type: &str,
    kind: &str,
    source_record_id: String,
    day: String,
    start: String,
    end: Option<String>,
    value: Option<Value>,
    unit: Option<&str>,
    metadata: Map<String, Value>,
    raw_locator: Option<String>,
) -> NormalizedInput {
    let mut object = Map::from_iter([
        ("schema".to_owned(), Value::String(SCHEMA.to_owned())),
        (
            "source_family".to_owned(),
            Value::String("oura_api".to_owned()),
        ),
        ("kind".to_owned(), Value::String(kind.to_owned())),
        (
            "record_type".to_owned(),
            Value::String(record_type.to_owned()),
        ),
        ("day".to_owned(), Value::String(day)),
        ("start_date".to_owned(), Value::String(start)),
        (
            "source_record_id".to_owned(),
            Value::String(source_record_id),
        ),
        ("metadata".to_owned(), Value::Object(metadata)),
    ]);
    if let Some(end) = end {
        object.insert("end_date".to_owned(), Value::String(end));
    }
    if let Some(value) = value.filter(|value| !value.is_null()) {
        object.insert("value".to_owned(), value);
    }
    if let Some(unit) = unit {
        object.insert("unit".to_owned(), Value::String(unit.to_owned()));
    }
    NormalizedInput {
        row: object,
        identity_metadata: None,
        raw_locator,
    }
}

fn pick(item: &Map<String, Value>, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|key| {
            item.get(*key)
                .filter(|value| !value.is_null())
                .cloned()
                .map(|value| ((*key).to_owned(), value))
        })
        .collect()
}

fn required_string(item: &Map<String, Value>, key: &str) -> Result<String, BodyIngestError> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| source("required_string"))
}

fn optional_string(item: &Map<String, Value>, key: &str) -> Option<String> {
    item.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn parse_day(value: &str) -> Result<String, BodyIngestError> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map(|day| day.format("%Y%m%d").to_string())
        .map_err(|_| normalize("day"))
}

fn owner_timestamp(value: &str, timezone: Tz) -> Result<String, BodyIngestError> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|value| {
            value
                .with_timezone(&timezone)
                .to_rfc3339_opts(SecondsFormat::AutoSi, false)
        })
        .map_err(|_| normalize("timestamp"))
}

fn source_hash(documents: &OuraDocuments, timezone: &str) -> Result<String, BodyIngestError> {
    let timezone = timezone
        .parse::<Tz>()
        .map_err(|_| normalize("timezone"))?
        .name();
    let raw = serde_json::to_vec(&serde_json::json!({
        "normalization_schema": SCHEMA,
        "normalization_timezone": timezone,
        "source_items": &documents.items,
    }))
    .map_err(|_| normalize("source_hash"))?;
    let value = parse(&raw).map_err(|_| normalize("source_hash"))?;
    let canonical = canonicalize(&value).map_err(|_| normalize("source_hash"))?;
    Ok(format!("{:x}", Sha256::digest(canonical.as_bytes())))
}

fn retained_pages(documents: &OuraDocuments) -> Result<Vec<RawAsset>, BodyIngestError> {
    let mut assets = Vec::new();
    for (endpoint, pages) in &documents.pages {
        let mut bytes = Vec::new();
        for page in pages {
            let raw = serde_json::to_vec(page).map_err(|_| normalize("raw_pages"))?;
            let value = parse(&raw).map_err(|_| normalize("raw_pages"))?;
            bytes.extend_from_slice(
                canonicalize(&value)
                    .map_err(|_| normalize("raw_pages"))?
                    .as_bytes(),
            );
            bytes.push(b'\n');
        }
        if bytes.is_empty() {
            continue;
        }
        assets.push(RawAsset::Bytes {
            bytes,
            relative: format!("oura/{endpoint}.jsonl"),
        });
    }
    Ok(assets)
}

fn source(stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(BodyIngestErrorKind::Source, stage)
}

fn normalize(stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(BodyIngestErrorKind::Normalize, stage)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn retained_parsed_pages_keep_each_validated_page_and_pagination_token() {
        let pages = vec![
            json!({"data":[{"id":"synthetic-1","day":"2026-01-01"}],"next_token":"page-2"}),
            json!({"data":[{"id":"synthetic-2","day":"2026-01-02"}],"next_token":null}),
        ];
        let mut documents = OuraDocuments::new();
        documents.insert(
            "daily_sleep".to_owned(),
            vec![pages[0]["data"][0].clone(), pages[1]["data"][0].clone()],
            pages.clone(),
        );

        let assets = retained_pages(&documents).expect("retain parsed pages");
        let [RawAsset::Bytes { bytes, relative }] = assets.as_slice() else {
            panic!("expected one in-memory raw page asset")
        };
        assert_eq!(relative, "oura/daily_sleep.jsonl");
        let restored = bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).expect("retained page JSON"))
            .collect::<Vec<_>>();
        assert_eq!(restored, pages);
    }
}
