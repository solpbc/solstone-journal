// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use axum::http::StatusCode;
use solstone_core_segment::{
    BoundStream, SegmentError, StreamHints, bind_named_stream, bind_stream,
    has_unattributed_stream_record, lookup_stream,
};

use crate::model::ReasonCode;
use crate::observer_evidence::{fallback_stream, resolve_device_observer};
use crate::read_routes::evidence_error;

/// The device display label is not carried on the wire at this protocol
/// version. An empty label lets `bind_stream` fall back to its own default,
/// disambiguated per (cid, source) exactly like any other label.
const STREAM_LABEL: &str = "";

/// Bind the stream this device should write, without advancing the chain.
pub(crate) fn bind_ingest_stream(
    journal: &Path,
    day: &str,
    segment: &str,
    cid: &str,
    source: &str,
    hints: &StreamHints,
) -> Result<BoundStream, (ReasonCode, StatusCode, String)> {
    match lookup_stream(journal, cid, source) {
        Ok(Some(name)) => {
            return map_named_bind(bind_named_stream(
                journal, day, segment, &name, cid, source, hints,
            ));
        }
        Ok(None) => {}
        Err(_) => {
            return Err((
                ReasonCode::JournalWriteFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot resolve journal stream".to_owned(),
            ));
        }
    }
    match resolve_device_observer(journal, cid) {
        Err(error) => Err(evidence_error(error)),
        Ok(Some(observer)) => {
            let name = fallback_stream(&observer).map_err(evidence_error)?;
            map_named_bind(bind_named_stream(
                journal, day, segment, &name, cid, source, hints,
            ))
        }
        Ok(None) => match has_unattributed_stream_record(journal) {
            Ok(true) => Err((
                ReasonCode::UnattributedStreamBlocksMint,
                StatusCode::CONFLICT,
                "cannot mint a stream while unattributed stream records exist".to_owned(),
            )),
            Ok(false) => bind_stream(journal, day, segment, STREAM_LABEL, cid, source, hints)
                .map_err(|_| {
                    (
                        ReasonCode::JournalWriteFailed,
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "cannot resolve journal stream".to_owned(),
                    )
                }),
            Err(_) => Err((
                ReasonCode::JournalWriteFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot resolve journal stream".to_owned(),
            )),
        },
    }
}

fn map_named_bind(
    result: Result<BoundStream, SegmentError>,
) -> Result<BoundStream, (ReasonCode, StatusCode, String)> {
    match result {
        Ok(bound) => Ok(bound),
        Err(SegmentError::StreamBindingConflict { name }) => Err((
            ReasonCode::ForeignStreamBinding,
            StatusCode::CONFLICT,
            format!("stream {name} is bound to another device"),
        )),
        Err(SegmentError::StreamInput(_)) => Err((
            ReasonCode::MalformedEvidenceRow,
            StatusCode::CONFLICT,
            "observer evidence has an unsupported shape".to_owned(),
        )),
        Err(_) => Err((
            ReasonCode::JournalWriteFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot resolve journal stream".to_owned(),
        )),
    }
}
