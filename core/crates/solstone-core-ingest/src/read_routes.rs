// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_segment::{list_days, lookup_stream};

use crate::listing::{DayListing, ListingError, ListingFile, merge_day_listing, native_events};
use crate::model::ReasonCode;
use crate::observer_evidence::{
    ObserverEvidenceError, ResolvedObserver, observer_history_days, read_history_day,
    resolve_device_observer,
};
use crate::router::{IngestState, refusal};
use crate::validation::{validate_access, validate_day, validate_protocol, validate_source};

pub async fn ingest_manifest(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(query): Query<SourceQuery>,
) -> Response {
    let context = match listing_context(&state, &basis, &headers, &query) {
        Ok(value) => value,
        Err((code, status, detail)) => return refusal(code, status, detail),
    };
    let mut days = match list_days(&state.journal_root) {
        Ok(days) => days
            .into_iter()
            .map(|(day, _)| day)
            .collect::<BTreeSet<_>>(),
        Err(_) => {
            return refusal(
                ReasonCode::JournalReadFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot read journal",
            );
        }
    };
    match observer_history_days(&state.journal_root, context.observer.as_ref()) {
        Ok(history_days) => days.extend(history_days),
        Err(error) => return evidence_refusal(error),
    }
    let mut result = Map::new();
    for day in days {
        let listing = match day_listing(
            &state,
            &context.cid,
            context.observer.as_ref(),
            context.native_stream.as_deref(),
            &day,
        ) {
            Ok(listing) => listing,
            Err(error) => {
                result.insert(day, json!({"error": day_read_reason(error).as_str()}));
                continue;
            }
        };
        if !listing.segments.is_empty() {
            result.insert(day, json!({"segments": listing.segments.len()}));
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
    let context = match listing_context(&state, &basis, &headers, &query) {
        Ok(value) => value,
        Err((code, status, detail)) => return refusal(code, status, detail),
    };
    if let Err(code) = validate_day(&day) {
        return refusal(code, StatusCode::BAD_REQUEST, "invalid day");
    }
    let listing = match day_listing(
        &state,
        &context.cid,
        context.observer.as_ref(),
        context.native_stream.as_deref(),
        &day,
    ) {
        Ok(listing) => listing,
        Err(error) => return day_refusal(error),
    };
    let segments = listing
        .segments
        .into_iter()
        .map(|segment| (segment.key, json!({"files": files_value(&segment.files)})))
        .collect::<Map<_, _>>();
    Json(json!({"version": 1, "day": day, "segments": segments})).into_response()
}

pub async fn ingest_segments(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(day): Path<String>,
    Query(query): Query<SourceQuery>,
) -> Response {
    let context = match listing_context(&state, &basis, &headers, &query) {
        Ok(value) => value,
        Err((code, status, detail)) => return refusal(code, status, detail),
    };
    if let Err(code) = validate_day(&day) {
        return refusal(code, StatusCode::BAD_REQUEST, "invalid day");
    }
    let listing = match day_listing(
        &state,
        &context.cid,
        context.observer.as_ref(),
        context.native_stream.as_deref(),
        &day,
    ) {
        Ok(listing) => listing,
        Err(error) => return day_refusal(error),
    };
    let items = listing
        .segments
        .iter()
        .map(|segment| {
            let mut item = Map::new();
            item.insert("key".to_owned(), Value::String(segment.key.clone()));
            item.insert("observed".to_owned(), Value::Bool(segment.observed));
            item.insert("files".to_owned(), files_value(&segment.files));
            if let Some(original_key) = &segment.original_key {
                item.insert(
                    "original_key".to_owned(),
                    Value::String(original_key.clone()),
                );
            }
            Value::Object(item)
        })
        .collect::<Vec<_>>();
    Json(json!({"protocol_version": 3, "total": items.len(), "items": items})).into_response()
}

struct ListingContext {
    cid: String,
    observer: Option<ResolvedObserver>,
    native_stream: Option<String>,
}

fn listing_context(
    state: &IngestState,
    basis: &AccessBasis,
    headers: &HeaderMap,
    query: &SourceQuery,
) -> Result<ListingContext, (ReasonCode, StatusCode, String)> {
    let cid = admitted(basis, headers)?;
    let source = query
        .source
        .as_deref()
        .map(|source| validate_source(source.as_bytes()))
        .transpose()
        .map_err(|code| (code, StatusCode::BAD_REQUEST, "invalid source".to_owned()))?
        .unwrap_or_default();
    let native_stream = lookup_stream(&state.journal_root, &cid, &source).map_err(|_| {
        (
            ReasonCode::JournalReadFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot resolve journal stream".to_owned(),
        )
    })?;
    let observer = resolve_device_observer(&state.journal_root, &cid).map_err(evidence_error)?;
    Ok(ListingContext {
        cid,
        observer,
        native_stream,
    })
}

fn day_listing(
    state: &IngestState,
    cid: &str,
    observer: Option<&ResolvedObserver>,
    native_stream: Option<&str>,
    day: &str,
) -> Result<DayListing, DayReadError> {
    let history =
        read_history_day(&state.journal_root, observer, day).map_err(DayReadError::Evidence)?;
    let events = native_events(&state.journal_root, day, native_stream, cid)
        .map_err(DayReadError::Listing)?;
    merge_day_listing(&state.journal_root, day, observer, history, events)
        .map_err(DayReadError::Listing)
}

fn files_value(files: &[ListingFile]) -> Value {
    Value::Array(
        files
            .iter()
            .map(|file| {
                let mut value = Map::new();
                value.insert("name".to_owned(), Value::String(file.name.clone()));
                value.insert("size".to_owned(), Value::from(file.size));
                value.insert("sha256".to_owned(), Value::String(file.sha256.clone()));
                value.insert(
                    "status".to_owned(),
                    Value::String(file.status.as_str().to_owned()),
                );
                if let Some(submitted_name) = &file.submitted_name {
                    value.insert(
                        "submitted_name".to_owned(),
                        Value::String(submitted_name.clone()),
                    );
                }
                Value::Object(value)
            })
            .collect(),
    )
}

pub(crate) fn evidence_error(error: ObserverEvidenceError) -> (ReasonCode, StatusCode, String) {
    match error {
        ObserverEvidenceError::RegistryUnreadable => (
            ReasonCode::ObserverRegistryUnreadable,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot enumerate observer registry".to_owned(),
        ),
        ObserverEvidenceError::RecordUnreadable => (
            ReasonCode::ObserverRecordUnreadable,
            StatusCode::CONFLICT,
            "observer registry contains unreadable records".to_owned(),
        ),
        ObserverEvidenceError::Ambiguous { prefixes } => (
            ReasonCode::AmbiguousDeviceObserver,
            StatusCode::CONFLICT,
            format!(
                "multiple observers bind this device ({}); resolve with observer revoke <prefix>",
                prefixes.join(", ")
            ),
        ),
        ObserverEvidenceError::HistoryUnreadable => (
            ReasonCode::ObserverHistoryUnreadable,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot read observer history".to_owned(),
        ),
        ObserverEvidenceError::HistoryTorn => (
            ReasonCode::ObserverHistoryTorn,
            StatusCode::CONFLICT,
            "observer history is torn".to_owned(),
        ),
        ObserverEvidenceError::Malformed => (
            ReasonCode::MalformedEvidenceRow,
            StatusCode::CONFLICT,
            "observer evidence has an unsupported shape".to_owned(),
        ),
        ObserverEvidenceError::JournalRead => (
            ReasonCode::JournalReadFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot read observer history".to_owned(),
        ),
    }
}

fn evidence_refusal(error: ObserverEvidenceError) -> Response {
    let (code, status, detail) = evidence_error(error);
    refusal(code, status, detail)
}

enum DayReadError {
    Evidence(ObserverEvidenceError),
    Listing(ListingError),
}

fn day_refusal(error: DayReadError) -> Response {
    match error {
        DayReadError::Evidence(error) => evidence_refusal(error),
        DayReadError::Listing(error) => listing_refusal(error),
    }
}

fn day_read_reason(error: DayReadError) -> ReasonCode {
    match error {
        DayReadError::Evidence(error) => evidence_error(error).0,
        DayReadError::Listing(error) => match error {
            ListingError::Malformed => ReasonCode::MalformedEvidenceRow,
            ListingError::AmbiguousName => ReasonCode::AmbiguousSegmentFileName,
            ListingError::JournalRead => ReasonCode::JournalReadFailed,
        },
    }
}

fn listing_refusal(error: ListingError) -> Response {
    let (code, detail) = match error {
        ListingError::Malformed => (
            ReasonCode::MalformedEvidenceRow,
            "observer evidence has an unsupported shape",
        ),
        ListingError::AmbiguousName => (
            ReasonCode::AmbiguousSegmentFileName,
            "multiple files have the same effective name",
        ),
        ListingError::JournalRead => (ReasonCode::JournalReadFailed, "cannot read journal"),
    };
    refusal(
        code,
        if code == ReasonCode::JournalReadFailed {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::CONFLICT
        },
        detail,
    )
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

#[cfg(test)]
mod access_tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};

    use super::admitted;
    use crate::model::ReasonCode;
    use crate::validation::PROTOCOL_HEADER;

    const VALID_CID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn protocol_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(PROTOCOL_HEADER, HeaderValue::from_static("3"));
        headers
    }

    #[test]
    fn read_admission_accepts_linked_devices_and_refuses_pairing_peers() {
        let headers = protocol_headers();
        let linked = AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
        };
        assert_eq!(admitted(&linked, &headers), Ok(VALID_CID.to_owned()));

        let refusal = admitted(
            &AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            },
            &headers,
        )
        .unwrap_err();
        assert_eq!(refusal.0, ReasonCode::LinkedDeviceRequired);
        assert_eq!(refusal.1, StatusCode::FORBIDDEN);
    }
}
