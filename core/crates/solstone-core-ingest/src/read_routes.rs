// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashSet};
use std::fs;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use solstone_core_callosum::{DeviceIngestEvent, read_device_ingest_events};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_segment::lookup_stream;
use solstone_core_segment::{list_days, list_segments, list_segments_in};

use crate::model::ReasonCode;
use crate::router::{IngestState, refusal};
use crate::validation::{validate_access, validate_day, validate_protocol, validate_source};

pub async fn ingest_manifest(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(query): Query<SourceQuery>,
) -> Response {
    let did = match admitted(&basis, &headers) {
        Ok(did) => did,
        Err((code, status, detail)) => return refusal(code, status, detail),
    };
    let stream = match resolved_stream(&state, &did, &query) {
        Ok(Some(stream)) => stream,
        Ok(None) => return Json(json!({"days": {}})).into_response(),
        Err((code, status)) => return refusal(code, status, "cannot resolve journal stream"),
    };
    let days = match list_days(&state.journal_root) {
        Ok(days) => days,
        Err(_) => {
            return refusal(
                ReasonCode::JournalReadFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot read journal",
            );
        }
    };
    let mut result = Map::new();
    for (day, path) in days {
        let count = match list_segments_in(&state.journal_root, &path) {
            Ok(segments) => {
                let mut keys = HashSet::new();
                for segment in segments
                    .into_iter()
                    .filter(|segment| segment.stream == stream)
                {
                    let events = match read_device_ingest_events(&segment.path) {
                        Ok(report) => report.records,
                        Err(_) => {
                            return refusal(
                                ReasonCode::JournalReadFailed,
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "cannot read journal",
                            );
                        }
                    };
                    keys.extend(
                        events
                            .into_iter()
                            .filter(|event| event.did == did)
                            .map(|event| event.segment),
                    );
                }
                keys.len()
            }
            Err(_) => {
                return refusal(
                    ReasonCode::JournalReadFailed,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cannot read segments",
                );
            }
        };
        if count > 0 {
            result.insert(day, json!({"segments": count}));
        }
    }
    Json(json!({"days": result})).into_response()
}

pub async fn ingest_manifest_day(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(day): Path<String>,
    Query(query): Query<SourceQuery>,
) -> Response {
    let did = match admitted(&basis, &headers) {
        Ok(did) => did,
        Err((code, status, detail)) => return refusal(code, status, detail),
    };
    if let Err(code) = validate_day(&day) {
        return refusal(code, StatusCode::BAD_REQUEST, "invalid day");
    }
    let stream = match resolved_stream(&state, &did, &query) {
        Ok(Some(stream)) => stream,
        Ok(None) => {
            return Json(json!({"version": 1, "day": day, "segments": Map::new()})).into_response();
        }
        Err((code, status)) => return refusal(code, status, "cannot resolve journal stream"),
    };
    let events = match stream_events(&state, &day, &stream, &did) {
        Ok(events) => events,
        Err(code) => {
            return refusal(
                code,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot read journal",
            );
        }
    };
    let mut segments = Map::new();
    for event in events {
        segments.insert(event.segment.clone(), json!({"files": event.files}));
    }
    Json(json!({"version": 1, "day": day, "segments": segments})).into_response()
}

pub async fn ingest_segments(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(day): Path<String>,
    Query(query): Query<SourceQuery>,
) -> Response {
    let did = match admitted(&basis, &headers) {
        Ok(did) => did,
        Err((code, status, detail)) => return refusal(code, status, detail),
    };
    if let Err(code) = validate_day(&day) {
        return refusal(code, StatusCode::BAD_REQUEST, "invalid day");
    }
    let stream = match resolved_stream(&state, &did, &query) {
        Ok(Some(stream)) => stream,
        Ok(None) => {
            return Json(json!({"protocol_version": 3, "total": 0, "items": []})).into_response();
        }
        Err((code, status)) => return refusal(code, status, "cannot resolve journal stream"),
    };
    let events = match stream_events(&state, &day, &stream, &did) {
        Ok(events) => events,
        Err(code) => {
            return refusal(
                code,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot read journal",
            );
        }
    };
    let mut unique_events = BTreeMap::new();
    for event in events {
        unique_events.entry(event.segment.clone()).or_insert(event);
    }
    let mut items = Vec::new();
    for (_, event) in unique_events {
        let path = state
            .journal_root
            .join("chronicle")
            .join(&day)
            .join(&stream)
            .join(&event.segment);
        let files: Vec<Value> = event
            .files
            .into_iter()
            .map(|file| {
                let status = match fs::read(path.join(&file.written)) {
                    Ok(bytes)
                        if bytes.len() as u64 == file.size && sha256(&bytes) == file.sha256 =>
                    {
                        "present"
                    }
                    _ => "missing",
                };
                let mut value = serde_json::to_value(file).unwrap_or(Value::Null);
                if let Value::Object(ref mut object) = value {
                    object.insert("status".to_owned(), Value::String(status.to_owned()));
                }
                value
            })
            .collect();
        items.push(json!({"key": event.segment, "observed": true, "files": files}));
    }
    Json(json!({"protocol_version": 3, "total": items.len(), "items": items})).into_response()
}

#[derive(serde::Deserialize)]
pub struct SourceQuery {
    pub source: Option<String>,
}

fn admitted(
    basis: &AccessBasis,
    headers: &HeaderMap,
) -> Result<String, (ReasonCode, StatusCode, String)> {
    validate_protocol(headers)?;
    validate_access(basis)
}

/// Resolve the (did, source)-bound stream for a read request, if any content
/// has ever been written for it. `Ok(None)` means the caller can return an
/// empty result directly rather than guessing a directory name to scan.
fn resolved_stream(
    state: &IngestState,
    did: &str,
    query: &SourceQuery,
) -> Result<Option<String>, (ReasonCode, StatusCode)> {
    let source = match query
        .source
        .as_deref()
        .map(|value| validate_source(value.as_bytes()))
        .transpose()
    {
        Ok(source) => source.unwrap_or_default(),
        Err(code) => return Err((code, StatusCode::BAD_REQUEST)),
    };
    lookup_stream(&state.journal_root, did, &source).map_err(|_| {
        (
            ReasonCode::JournalReadFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })
}

fn stream_events(
    state: &IngestState,
    day: &str,
    stream: &str,
    did: &str,
) -> Result<Vec<DeviceIngestEvent>, ReasonCode> {
    let segments =
        list_segments(&state.journal_root, day).map_err(|_| ReasonCode::JournalReadFailed)?;
    let mut events = Vec::new();
    for segment in segments
        .into_iter()
        .filter(|segment| segment.stream == stream)
    {
        let report =
            read_device_ingest_events(&segment.path).map_err(|_| ReasonCode::JournalReadFailed)?;
        for event in report.records {
            if event.did == did {
                events.push(event);
            }
        }
    }
    Ok(events)
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}
