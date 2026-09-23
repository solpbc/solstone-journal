// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_segment::{list_days, list_stream_segments, lookup_stream_state};

use crate::health::day_read_reason;
use crate::listing::{DayListing, ListingError, ListingFile, merge_day_listing, native_events};
use crate::model::ReasonCode;
use crate::router::{IngestState, refusal};
use crate::validation::{validate_access, validate_day, validate_protocol, validate_source};

pub async fn ingest_manifest(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(query): Query<SourceQuery>,
) -> Response {
    // One listing of this device's own stream directory per day, and nothing
    // inside any segment: no receipts are read and no file is stated. It used
    // to fold every stream's segments and every receipt in the journal, and a
    // device called it at the head of every sync cycle. The two day reads still
    // read receipts and still refuse a day whose receipts are unreadable.
    // Deprecated: a device proves custody from its upload response and reads
    // `segments/{day}` only when its own record is missing.
    spawn_blocking_response(OwnerReadRole::DeviceIngestManifest, move || {
        let context = match listing_context(&state, &basis, &headers, &query) {
            Ok(value) => value,
            Err((code, status, detail)) => return refusal(code, status, detail),
        };
        let mut result = Map::new();
        let Some(stream) = context.native_stream else {
            return Json(json!({"days": result})).into_response();
        };
        let days = match list_days(&state.journal_root) {
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
        for day in days {
            match list_stream_segments(&state.journal_root, &day, &stream) {
                Ok(segments) if segments.is_empty() => {}
                Ok(segments) => {
                    result.insert(day, json!({"segments": segments.len()}));
                }
                Err(_) => {
                    // A device told this refuses the whole day and reports itself offline,
                    // so the journal says so too; nothing else on this side records it.
                    let reason = day_read_reason(ListingError::JournalRead);
                    log::warn!(
                        "device_manifest_day_unreadable day={day} reason={}",
                        reason.as_str()
                    );
                    result.insert(day, json!({"error": reason.as_str()}));
                }
            }
        }
        Json(json!({"days": result})).into_response()
    })
    .await
}

pub async fn ingest_manifest_day(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(day): Path<String>,
    Query(query): Query<SourceQuery>,
) -> Response {
    if let Err(code) = validate_day(&day) {
        return refusal(code, StatusCode::BAD_REQUEST, "invalid day");
    }
    spawn_blocking_response(OwnerReadRole::DeviceIngestManifestDay, move || {
        let context = match listing_context(&state, &basis, &headers, &query) {
            Ok(value) => value,
            Err((code, status, detail)) => return refusal(code, status, detail),
        };
        let listing = match day_listing(
            &state,
            &context.cid,
            &context.source,
            context.native_stream.as_deref(),
            &day,
        ) {
            Ok(listing) => listing,
            Err(error) => return day_refusal(&day, error),
        };
        let segments = listing
            .segments
            .into_iter()
            .map(|segment| (segment.key, json!({"files": files_value(&segment.files)})))
            .collect::<Map<_, _>>();
        Json(json!({"version": 1, "day": day, "segments": segments})).into_response()
    })
    .await
}

pub async fn ingest_segments(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(day): Path<String>,
    Query(query): Query<SourceQuery>,
) -> Response {
    if let Err(code) = validate_day(&day) {
        return refusal(code, StatusCode::BAD_REQUEST, "invalid day");
    }
    spawn_blocking_response(OwnerReadRole::DeviceIngestSegments, move || {
        let context = match listing_context(&state, &basis, &headers, &query) {
            Ok(value) => value,
            Err((code, status, detail)) => return refusal(code, status, detail),
        };
        let listing = match day_listing(
            &state,
            &context.cid,
            &context.source,
            context.native_stream.as_deref(),
            &day,
        ) {
            Ok(listing) => listing,
            Err(error) => return day_refusal(&day, error),
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
    })
    .await
}

#[derive(Debug)]
struct ListingContext {
    cid: String,
    source: String,
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
    let binding = lookup_stream_state(&state.journal_root, &cid, &source).map_err(|_| {
        (
            ReasonCode::JournalReadFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot resolve journal stream".to_owned(),
        )
    })?;
    if let Some(bound) = &binding
        && bound.seq == 0
    {
        return Err((
            ReasonCode::StreamBindingIncomplete,
            StatusCode::CONFLICT,
            "authenticated stream binding is incomplete".to_owned(),
        ));
    }
    Ok(ListingContext {
        cid,
        source,
        native_stream: binding.map(|bound| bound.name),
    })
}

fn day_listing(
    state: &IngestState,
    cid: &str,
    source: &str,
    native_stream: Option<&str>,
    day: &str,
) -> Result<DayListing, ListingError> {
    let events = native_events(&state.journal_root, day, native_stream, cid, source)?;
    merge_day_listing(&state.journal_root, day, events)
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

fn day_refusal(day: &str, error: ListingError) -> Response {
    log::warn!(
        "device_manifest_day_unreadable day={day} reason={}",
        day_read_reason(error).as_str()
    );
    listing_refusal(error)
}

fn listing_refusal(error: ListingError) -> Response {
    let (code, detail) = match error {
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

#[cfg(test)]
mod listing_context_tests {
    use std::fs;
    use std::sync::Arc;

    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use serde_json::json;
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
    use solstone_core_ingest_resolve::IngestNotice;

    use super::{SourceQuery, listing_context};
    use crate::model::ReasonCode;
    use crate::router::IngestState;
    use crate::validation::PROTOCOL_HEADER;

    const VALID_CID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    struct SilentNotifier;

    impl solstone_core_ingest_resolve::IngestNotifier for SilentNotifier {
        fn notify(
            &self,
            _notice: &IngestNotice<'_>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }
    }

    fn protocol_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(PROTOCOL_HEADER, HeaderValue::from_static("3"));
        headers
    }

    fn linked() -> AccessBasis {
        AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
        }
    }

    fn ingest_state(root: &std::path::Path) -> IngestState {
        IngestState {
            journal_root: root.to_path_buf(),
            notifier: Arc::new(SilentNotifier),
            now_ms: Arc::new(|| 0),
        }
    }

    fn write_stream(root: &std::path::Path, seq: u64) {
        let path = root.join("streams/desk_01.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            json!({
                "name": "desk_01",
                "kind": "observer",
                "host": null,
                "platform": null,
                "created_at": 1,
                "last_day": null,
                "last_segment": null,
                "seq": seq,
                "cid": VALID_CID,
                "source": "",
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn listing_context_unbound_is_empty_native_stream() {
        let journal = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let context = listing_context(
            &ingest_state(journal.path()),
            &linked(),
            &protocol_headers(),
            &SourceQuery { source: None },
        )
        .unwrap();
        assert_eq!(context.native_stream, None);
    }

    #[test]
    fn listing_context_healthy_binding_returns_the_name() {
        let journal = tempfile::TempDir::new_in("/var/tmp").unwrap();
        write_stream(journal.path(), 1);
        let context = listing_context(
            &ingest_state(journal.path()),
            &linked(),
            &protocol_headers(),
            &SourceQuery { source: None },
        )
        .unwrap();
        assert_eq!(context.native_stream.as_deref(), Some("desk_01"));
    }

    #[test]
    fn listing_context_seq_zero_is_incomplete() {
        let journal = tempfile::TempDir::new_in("/var/tmp").unwrap();
        write_stream(journal.path(), 0);
        let error = listing_context(
            &ingest_state(journal.path()),
            &linked(),
            &protocol_headers(),
            &SourceQuery { source: None },
        )
        .unwrap_err();
        assert_eq!(error.0, ReasonCode::StreamBindingIncomplete);
        assert_eq!(error.1, StatusCode::CONFLICT);
    }
}
