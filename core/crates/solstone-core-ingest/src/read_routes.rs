// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_segment::{list_stream_bindings, visible_stream_names};

use crate::health::day_read_reason;
use crate::listing::{
    DayListing, ListingError, merge_day_listing, native_events, segment_item_json,
};
use crate::model::ReasonCode;
use crate::router::{IngestState, refusal};
use crate::validation::{validate_access, validate_day, validate_protocol, validate_source};

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
            &context.native_streams,
            &day,
        ) {
            Ok(listing) => listing,
            Err(error) => return day_refusal(&day, error),
        };
        let items = listing
            .segments
            .iter()
            .map(segment_item_json)
            .collect::<Vec<_>>();
        Json(json!({"protocol_version": 3, "total": items.len(), "items": items})).into_response()
    })
    .await
}

#[derive(Debug)]
struct ListingContext {
    cid: String,
    source: String,
    native_streams: Vec<String>,
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
    let visible_streams =
        visible_stream_names(&state.journal_root, &cid, &source).map_err(|_| {
            (
                ReasonCode::JournalReadFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot resolve journal stream".to_owned(),
            )
        })?;
    let mut native_streams = Vec::new();
    if !visible_streams.is_empty() {
        let bindings = list_stream_bindings(&state.journal_root).map_err(|_| {
            (
                ReasonCode::JournalReadFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot resolve journal stream".to_owned(),
            )
        })?;
        for stream in &visible_streams {
            match bindings.iter().find(|binding| binding.name == *stream) {
                Some(binding) if binding.seq > 0 => native_streams.push(stream.clone()),
                Some(_) => {}
                None => {
                    return Err((
                        ReasonCode::JournalReadFailed,
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "cannot resolve journal stream".to_owned(),
                    ));
                }
            }
        }
        if native_streams.is_empty() {
            return Err((
                ReasonCode::StreamBindingIncomplete,
                StatusCode::CONFLICT,
                "authenticated stream binding is incomplete".to_owned(),
            ));
        }
    }
    Ok(ListingContext {
        cid,
        source,
        native_streams,
    })
}

fn day_listing(
    state: &IngestState,
    cid: &str,
    source: &str,
    native_streams: &[String],
    day: &str,
) -> Result<DayListing, ListingError> {
    let mut events = Vec::new();
    for stream in native_streams {
        events.extend(native_events(
            &state.journal_root,
            day,
            Some(stream),
            cid,
            source,
        )?);
    }
    merge_day_listing(&state.journal_root, day, events)
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
            leaf_spki: vec![0x30, 0x00],
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
            leaf_spki: vec![0x30, 0x00],
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
        write_stream_record(root, "desk_01", VALID_CID, "", seq);
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
        assert!(context.native_streams.is_empty());
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
        assert_eq!(context.native_streams, vec!["desk_01"]);
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

    #[test]
    fn listing_context_omits_seq_zero_sibling_but_keeps_advanced_continuation() {
        const SELECTED_CID: &str =
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        const SOURCE: &str = "audio";
        const SELECTED_STREAM: &str = "selected_01";
        const ADOPTED_RESERVATION: &str = "adopted_01";

        let journal = tempfile::TempDir::new_in("/var/tmp").unwrap();
        write_stream_record(journal.path(), SELECTED_STREAM, SELECTED_CID, SOURCE, 3);
        write_stream_record(journal.path(), ADOPTED_RESERVATION, VALID_CID, SOURCE, 0);
        fs::write(
            journal.path().join("streams/continuity.json"),
            json!({
                "version": 1,
                "streams": {
                    SELECTED_STREAM: {
                        "source": SOURCE,
                        "writers": [SELECTED_CID, VALID_CID],
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let context = listing_context(
            &ingest_state(journal.path()),
            &linked(),
            &protocol_headers(),
            &SourceQuery {
                source: Some(SOURCE.to_owned()),
            },
        )
        .unwrap();

        assert_eq!(context.native_streams, vec![SELECTED_STREAM]);
    }

    fn write_stream_record(root: &std::path::Path, name: &str, cid: &str, source: &str, seq: u64) {
        let path = root.join("streams").join(format!("{name}.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            json!({
                "name": name,
                "kind": "observer",
                "host": null,
                "platform": null,
                "created_at": 1,
                "last_day": null,
                "last_segment": null,
                "seq": seq,
                "cid": cid,
                "source": source,
            })
            .to_string(),
        )
        .unwrap();
    }
}
