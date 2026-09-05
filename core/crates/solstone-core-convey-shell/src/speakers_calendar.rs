// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as RoutePath, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::SegmentLayout;

use crate::JournalRoot;
use crate::speakers_review::is_admissible_speaker_entity;
use solstone_core_speaker_resolve::segment_catalog::{
    CatalogBuildError, CatalogedSegment, catalog_day, catalog_days,
};

#[derive(Debug, Deserialize)]
pub struct SegmentsQuery {
    limit: Option<String>,
    offset: Option<String>,
    speaker: Option<String>,
}

pub(crate) struct Segment {
    pub(crate) key: String,
    pub(crate) path: PathBuf,
    pub(crate) payload: Value,
}

type DayCounts = BTreeMap<String, usize>;

pub async fn index(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match speaker_segment_counts(&root.0, None) {
        Ok(counts) => Json(date_nav_index(&counts)).into_response(),
        Err(error) => catalog_failure(error),
    }
}

pub async fn grid(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    match speaker_grid_counts(&root.0) {
        Ok((days, activity)) => {
            let watermark = days.keys().next_back().cloned();
            Json(day_grid_payload(
                &days,
                watermark.as_deref(),
                coverage_from_counts(&activity),
                &activity,
            ))
            .into_response()
        }
        Err(error) => catalog_failure(error),
    }
}

pub async fn stats(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath(month): RoutePath<String>,
) -> Response {
    if !is_month(&month) {
        return refusal(
            "invalid_month",
            "I couldn't use that month.",
            "Invalid month format, expected YYYYMM",
        );
    }
    match speaker_segment_counts(&root.0, Some(&month)) {
        Ok(counts) => Json(counts).into_response(),
        Err(error) => catalog_failure(error),
    }
}

pub async fn segments(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath(day): RoutePath<String>,
    Query(query): Query<SegmentsQuery>,
) -> Response {
    if !is_day(&day) {
        return refusal(
            "invalid_day",
            "I couldn't use that day.",
            "Invalid day format",
        );
    }
    let limit = match parse_nonnegative(query.limit.as_deref(), 20) {
        Some(value) => value,
        None => {
            return refusal(
                "invalid_request_value",
                "I couldn't use one of those values.",
                "Invalid limit/offset parameter",
            );
        }
    };
    let offset = match parse_nonnegative(query.offset.as_deref(), 0) {
        Some(value) => value,
        None => {
            return refusal(
                "invalid_request_value",
                "I couldn't use one of those values.",
                "Invalid limit/offset parameter",
            );
        }
    };
    let speaker = match query.speaker {
        Some(value) => {
            let value = value.trim().to_owned();
            if value.is_empty() {
                return refusal(
                    "invalid_request_value",
                    "I couldn't use one of those values.",
                    "Invalid speaker parameter",
                );
            }
            Some(value)
        }
        None => None,
    };

    let admitted_speaker_ids = admitted_speaker_ids(&load_all_journal_entities(&root.0));
    let mut rows = match scan_segment_embeddings(&root.0, &day) {
        Ok(rows) => rows,
        Err(error) => return catalog_failure(error),
    };
    rows.sort_by(|left, right| left.key.cmp(&right.key));
    if let Some(speaker) = speaker.as_deref() {
        rows.retain(|segment| segment_has_speaker(&segment.path, speaker, &admitted_speaker_ids));
    }
    let total = rows.len();
    let principal_id = journal_principal_id(&root.0);
    let page = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|mut segment| {
            add_attribution_counts(
                &mut segment.payload,
                load_speaker_labels(&segment.path).as_ref(),
                principal_id.as_deref(),
                &admitted_speaker_ids,
            );
            segment.payload
        })
        .collect::<Vec<_>>();
    Json(json!({"segments": page, "total": total})).into_response()
}

fn refusal(reason_code: &str, message: &str, detail: &str) -> Response {
    error_envelope(reason_code, message, detail, StatusCode::BAD_REQUEST).into_response()
}

fn catalog_failure(error: CatalogBuildError) -> Response {
    error_envelope(
        "speaker_command_failed",
        "I couldn't finish that speaker command.",
        error.to_string(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

pub(crate) fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_month(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn parse_nonnegative(value: Option<&str>, default: usize) -> Option<usize> {
    let Some(value) = value else {
        return Some(default);
    };
    let parsed = value.trim().parse::<i128>().ok()?;
    if parsed <= 0 {
        return Some(0);
    }
    Some(usize::try_from(parsed).unwrap_or(usize::MAX))
}

fn speaker_segment_counts(
    root: &Path,
    month: Option<&str>,
) -> Result<BTreeMap<String, usize>, CatalogBuildError> {
    let mut counts = BTreeMap::new();
    for day in day_dirs(root)? {
        if month.is_some_and(|month| !day.starts_with(month)) {
            continue;
        }
        let count = scan_segment_embeddings(root, &day)?.len();
        if count > 0 {
            counts.insert(day, count);
        }
    }
    Ok(counts)
}

fn speaker_grid_counts(root: &Path) -> Result<(DayCounts, DayCounts), CatalogBuildError> {
    let admitted_speaker_ids = admitted_speaker_ids(&load_all_journal_entities(root));
    let mut days = BTreeMap::new();
    let mut activity = BTreeMap::new();
    for day in day_dirs(root)? {
        let mut activity_count = 0;
        let mut needs_review_count = 0;
        for segment in iter_segments(root, &day)? {
            if audio_embedding_sources(&segment.path).is_empty() {
                continue;
            }
            activity_count += 1;
            if segment_has_speaker_review(
                load_speaker_labels(&segment.path).as_ref(),
                &admitted_speaker_ids,
            ) {
                needs_review_count += 1;
            }
        }
        if activity_count > 0 {
            activity.insert(day.clone(), activity_count);
        }
        if needs_review_count > 0 {
            days.insert(day, needs_review_count);
        }
    }
    Ok((days, activity))
}

pub(crate) fn scan_segment_embeddings(
    root: &Path,
    day: &str,
) -> Result<Vec<Segment>, CatalogBuildError> {
    let mut rows = Vec::new();
    for segment in iter_segments(root, day)? {
        let Some((start, end, duration)) = parse_segment(&segment.time_key) else {
            continue;
        };
        let sources = audio_embedding_sources(&segment.path);
        if sources.is_empty() {
            continue;
        }
        let speakers = load_segment_speakers(&segment.path);
        rows.push(Segment {
            key: segment.key.clone(),
            path: segment.path,
            payload: json!({
                "key": segment.key,
                "stream": segment.stream,
                "stream_layout": stream_layout_json(segment.layout),
                "start": start,
                "end": end,
                "duration": duration,
                "sources": sources,
                "speakers": speakers,
                "speaker_count": speakers.len(),
            }),
        });
    }
    Ok(rows)
}

fn stream_layout_json(layout: SegmentLayout) -> &'static str {
    match layout {
        SegmentLayout::Direct => "direct",
        SegmentLayout::Named => "named",
    }
}

pub(crate) struct SegmentDirectory {
    pub(crate) stream: String,
    pub(crate) key: String,
    pub(crate) path: PathBuf,
    pub(crate) layout: SegmentLayout,
    pub(crate) time_key: String,
}

impl From<CatalogedSegment> for SegmentDirectory {
    fn from(segment: CatalogedSegment) -> Self {
        Self {
            stream: segment.stream,
            key: segment.name,
            path: segment.path,
            layout: segment.layout,
            time_key: segment.key,
        }
    }
}

pub(crate) fn day_dirs(root: &Path) -> Result<Vec<String>, CatalogBuildError> {
    catalog_days(root)
}

pub(crate) fn iter_segments(
    root: &Path,
    day: &str,
) -> Result<Vec<SegmentDirectory>, CatalogBuildError> {
    let mut segments: Vec<_> = catalog_day(root, day)?
        .into_iter()
        .map(SegmentDirectory::from)
        .collect();
    segments.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(segments)
}

fn read_dirs(path: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect()
}

pub(crate) fn parse_segment(key: &str) -> Option<(String, String, u64)> {
    let (time, length) = key.split_once('_')?;
    if time.len() != 6
        || !time.bytes().all(|byte| byte.is_ascii_digit())
        || length.is_empty()
        || !length.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hour = time[0..2].parse::<u64>().ok()?;
    let minute = time[2..4].parse::<u64>().ok()?;
    let second = time[4..6].parse::<u64>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let length = length.parse::<u64>().ok()?;
    let start = hour * 3_600 + minute * 60 + second;
    let end = start.checked_add(length)?;
    let end = if end >= 86_400 { 86_399 } else { end };
    Some((
        format!("{hour:02}:{minute:02}"),
        format!("{:02}:{:02}", end / 3_600, (end % 3_600) / 60),
        end - start,
    ))
}

pub(crate) fn audio_embedding_sources(path: &Path) -> Vec<String> {
    let mut sources = fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.is_file() && path.extension().is_some_and(|extension| extension == "npz"))
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .filter(|stem| stem == "audio" || stem.ends_with("_audio"))
        .collect::<Vec<_>>();
    sources.sort();
    sources
}

pub(crate) fn load_segment_speakers(path: &Path) -> Vec<String> {
    read_json(&path.join("talents/speakers.json"))
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .filter(|name| !name.trim().is_empty())
        .collect()
}

pub(crate) fn load_speaker_labels(path: &Path) -> Option<Value> {
    read_json(&path.join("talents/speaker_labels.json"))
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn segment_has_speaker_review(
    labels_data: Option<&Value>,
    admitted_speaker_ids: &BTreeSet<String>,
) -> bool {
    let Some(labels_data) = labels_data.filter(|value| value_truthy(value)) else {
        return false;
    };
    labels_data
        .get("labels")
        .and_then(Value::as_array)
        .is_some_and(|labels| {
            labels.iter().any(|label| {
                speaker_sentence_needs_review(Some(label), Some(labels_data), admitted_speaker_ids)
            })
        })
}

/// Return the shared speaker-review flag for an optional sentence label.
pub(crate) fn speaker_sentence_needs_review(
    label: Option<&Value>,
    labels_data: Option<&Value>,
    admitted_speaker_ids: &BTreeSet<String>,
) -> bool {
    match label.filter(|label| value_truthy(label)) {
        Some(label) => {
            label.get("confidence").and_then(Value::as_str) == Some("medium")
                || !label_has_admitted_speaker(label, admitted_speaker_ids)
        }
        None => labels_data.is_some_and(value_truthy),
    }
}

pub(crate) fn value_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn segment_has_speaker(
    path: &Path,
    speaker: &str,
    admitted_speaker_ids: &BTreeSet<String>,
) -> bool {
    admitted_speaker_ids.contains(speaker)
        && load_speaker_labels(path)
            .and_then(|labels| labels.get("labels").and_then(Value::as_array).cloned())
            .is_some_and(|labels| {
                labels
                    .iter()
                    .any(|label| label.get("speaker").and_then(Value::as_str) == Some(speaker))
            })
}

pub(crate) fn admitted_speaker_ids(entities: &[(String, Value)]) -> BTreeSet<String> {
    entities
        .iter()
        .filter(|(_, entity)| is_admissible_speaker_entity(entity))
        .map(|(entity_id, _)| entity_id.clone())
        .collect()
}

pub(crate) fn label_has_admitted_speaker(
    label: &Value,
    admitted_speaker_ids: &BTreeSet<String>,
) -> bool {
    label
        .get("speaker")
        .and_then(Value::as_str)
        .is_some_and(|entity_id| !entity_id.is_empty() && admitted_speaker_ids.contains(entity_id))
}

pub(crate) fn journal_principal_id(root: &Path) -> Option<String> {
    load_all_journal_entities(root)
        .into_iter()
        .find(|(_, entity)| entity.get("is_principal") == Some(&Value::Bool(true)))
        .map(|(entity_id, _)| entity_id)
}

/// Read parseable journal entity records in the Python scanner's sorted ID order.
pub(crate) fn load_all_journal_entities(root: &Path) -> Vec<(String, Value)> {
    let mut entities = read_dirs(&root.join("entities"));
    entities.sort_by_key(|entry| entry.file_name());
    entities
        .into_iter()
        .filter_map(|entry| {
            read_json(&entry.path().join("entity.json"))
                .map(|entity| (entry.file_name().to_string_lossy().into_owned(), entity))
        })
        .collect()
}

fn add_attribution_counts(
    payload: &mut Value,
    labels_data: Option<&Value>,
    principal_id: Option<&str>,
    admitted_speaker_ids: &BTreeSet<String>,
) {
    let Some(labels_data) = labels_data.filter(|value| value_truthy(value)) else {
        add_zero_attribution_counts(payload);
        return;
    };
    let labels = labels_data
        .get("labels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let attribution_total = labels.len();
    let attribution_needs_review = labels
        .iter()
        .filter(|label| {
            speaker_sentence_needs_review(Some(label), Some(labels_data), admitted_speaker_ids)
        })
        .count();
    let attribution_null = labels
        .iter()
        .filter(|label| !label_has_admitted_speaker(label, admitted_speaker_ids))
        .count();
    let owner_count = labels
        .iter()
        .filter(|label| {
            label.get("speaker").is_some_and(value_truthy)
                && label.get("speaker").and_then(Value::as_str) == principal_id
        })
        .count();
    let object = payload
        .as_object_mut()
        .expect("segment payload is an object");
    object.insert("attribution_total".to_owned(), json!(attribution_total));
    object.insert(
        "attribution_needs_review".to_owned(),
        json!(attribution_needs_review),
    );
    object.insert("attribution_null".to_owned(), json!(attribution_null));
    object.insert(
        "attribution_non_owner_total".to_owned(),
        json!(attribution_total - owner_count),
    );
}

fn add_zero_attribution_counts(payload: &mut Value) {
    let object = payload
        .as_object_mut()
        .expect("segment payload is an object");
    for key in [
        "attribution_total",
        "attribution_needs_review",
        "attribution_null",
        "attribution_non_owner_total",
    ] {
        object.insert(key.to_owned(), json!(0));
    }
}

fn date_nav_index(counts: &BTreeMap<String, usize>) -> Value {
    let mut months = BTreeMap::<String, usize>::new();
    let mut days = Vec::new();
    for (day, total) in counts {
        if *total == 0 {
            continue;
        }
        days.push(day.clone());
        *months.entry(day[..6].to_owned()).or_default() += total;
    }
    let coverage = days
        .first()
        .zip(days.last())
        .map(|(start, end)| json!({"start": start, "end": end}));
    json!({"coverage": coverage, "months": months})
}

fn coverage_from_counts(counts: &BTreeMap<String, usize>) -> Option<Value> {
    let mut days = counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(day, _)| day);
    let start = days.next()?;
    let end = days.next_back().unwrap_or(start);
    Some(json!({"start": start, "end": end}))
}

fn day_grid_payload(
    counts: &BTreeMap<String, usize>,
    watermark: Option<&str>,
    coverage: Option<Value>,
    activity: &BTreeMap<String, usize>,
) -> Value {
    let mut days = Map::new();
    let mut pending = Map::new();
    for (day, count) in counts {
        let target = if watermark.is_some_and(|watermark| day.as_str() <= watermark) {
            &mut days
        } else {
            &mut pending
        };
        target.insert(day.clone(), json!(count));
    }
    let activity = activity
        .iter()
        .map(|(day, count)| (day.clone(), json!(count)))
        .collect::<Map<_, _>>();
    json!({"coverage": coverage, "days": days, "pending": pending, "activity": activity})
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use super::{add_attribution_counts, speaker_sentence_needs_review};

    #[test]
    fn invalid_speakers_are_unassigned_for_review_and_attribution_counts() {
        let admitted = BTreeSet::from(["person".to_owned()]);
        let labels = json!({"labels":[
            {"speaker":"person","confidence":"high"},
            {"speaker":"tool","confidence":"high"}
        ]});
        assert!(!speaker_sentence_needs_review(
            Some(&labels["labels"][0]),
            Some(&labels),
            &admitted,
        ));
        assert!(speaker_sentence_needs_review(
            Some(&labels["labels"][1]),
            Some(&labels),
            &admitted,
        ));

        let mut payload = json!({});
        add_attribution_counts(&mut payload, Some(&labels), None, &admitted);
        assert_eq!(payload["attribution_total"], 2);
        assert_eq!(payload["attribution_needs_review"], 1);
        assert_eq!(payload["attribution_null"], 1);
    }
}
