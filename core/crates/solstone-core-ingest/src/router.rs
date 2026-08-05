// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use axum::extract::{DefaultBodyLimit, Extension, FromRequest, Multipart, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::{error_envelope, not_found_fallback};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_journal_io::DEFAULT_STREAM;
use solstone_core_segment::{
    ContentName, ContentWriteOutcome, SegmentDir, StreamHints, advance_stream, append_event,
    write_content,
};
use tower_http::limit::RequestBodyLimitLayer;

use crate::model::{DeviceIngestEvent, FileDescriptor, IncomingFile, ReasonCode};
use crate::read_routes::{ingest_manifest, ingest_manifest_day, ingest_segments};
use crate::validation::{
    validate_access, validate_day, validate_protocol, validate_segment, validate_source,
};

const MAX_PART_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 8;
const MAX_PARTS: usize = 12;
const MAX_FILENAME_BYTES: usize = 128;
const MAX_HEADERS: usize = 16;
const MAX_SEGMENT_ATTEMPTS: u64 = 1_024;

#[derive(Clone)]
pub(crate) struct IngestState {
    pub(crate) journal_root: PathBuf,
}

/// Build the four linked-device segment-arrival routes.
pub fn router(journal_root: impl AsRef<Path>) -> Router {
    Router::new()
        .route("/app/observer/ingest", post(ingest_upload))
        .route("/app/observer/ingest/manifest", get(ingest_manifest))
        .route(
            "/app/observer/ingest/manifest/{day}",
            get(ingest_manifest_day),
        )
        .route("/app/observer/ingest/segments/{day}", get(ingest_segments))
        .layer(DefaultBodyLimit::max(128 * 1024 * 1024))
        .layer(RequestBodyLimitLayer::new(128 * 1024 * 1024))
        .with_state(IngestState {
            journal_root: journal_root.as_ref().to_path_buf(),
        })
        .fallback(not_found_fallback)
}

pub(crate) fn refusal(code: ReasonCode, status: StatusCode, detail: impl Into<String>) -> Response {
    error_envelope(code.as_str(), "Ingest request refused", detail, status).into_response()
}

async fn ingest_upload(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    request: Request,
) -> Response {
    let did = match validate_access(&basis) {
        Ok(did) => did,
        Err((code, status, detail)) => return refusal(code, status, detail),
    };
    if let Err((code, status, detail)) = validate_protocol(request.headers()) {
        return refusal(code, status, detail);
    }
    let parsed = match parse_multipart(request).await {
        Ok(parsed) => parsed,
        Err((code, detail)) => return refusal(code, StatusCode::BAD_REQUEST, detail),
    };
    let envelope = match parse_envelope(parsed.envelope, parsed.files) {
        Ok(envelope) => envelope,
        Err((code, detail)) => return refusal(code, StatusCode::BAD_REQUEST, detail),
    };
    let stream = if envelope.source.is_empty() {
        DEFAULT_STREAM.to_owned()
    } else {
        envelope.source.clone()
    };
    write_envelope(&state, &did, stream, envelope)
}

struct MultipartInput {
    envelope: String,
    files: Vec<RawFile>,
}
struct RawFile {
    filename: String,
    bytes: Vec<u8>,
}

async fn parse_multipart(request: Request) -> Result<MultipartInput, (ReasonCode, String)> {
    let mut multipart = Multipart::from_request(request, &()).await.map_err(|_| {
        (
            ReasonCode::MultipartMalformed,
            "invalid multipart body".to_owned(),
        )
    })?;
    let mut parts = 0usize;
    let mut envelope = None;
    let mut files = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(|_| {
        (
            ReasonCode::MultipartMalformed,
            "cannot parse multipart part".to_owned(),
        )
    })? {
        parts += 1;
        if parts > MAX_PARTS {
            return Err((
                ReasonCode::MultipartTooManyParts,
                "too many multipart parts".to_owned(),
            ));
        }
        if field.headers().len() > MAX_HEADERS {
            return Err((
                ReasonCode::MultipartTooManyHeaders,
                "too many multipart headers".to_owned(),
            ));
        }
        let name = field.name().unwrap_or_default().to_owned();
        let filename = field.file_name().map(ToOwned::to_owned);
        let bytes = bounded_part(field).await?;
        if name == "envelope" {
            if envelope.is_some() {
                return Err((
                    ReasonCode::FieldDuplicate,
                    "envelope appears more than once".to_owned(),
                ));
            }
            if filename.is_some() {
                return Err((
                    ReasonCode::MultipartMalformed,
                    "envelope must be a text field".to_owned(),
                ));
            }
            envelope = Some(String::from_utf8(bytes).map_err(|_| {
                (
                    ReasonCode::MultipartMalformed,
                    "envelope is not UTF-8".to_owned(),
                )
            })?);
        } else if name == "files" {
            if files.len() == MAX_FILES {
                return Err((
                    ReasonCode::MultipartTooManyFiles,
                    "too many file parts".to_owned(),
                ));
            }
            let filename = filename.ok_or_else(|| {
                (
                    ReasonCode::FileNameInvalid,
                    "file part has no filename".to_owned(),
                )
            })?;
            if filename.len() > MAX_FILENAME_BYTES {
                return Err((
                    ReasonCode::MultipartFilenameTooLong,
                    "filename is too long".to_owned(),
                ));
            }
            files.push(RawFile { filename, bytes });
        }
    }
    let envelope =
        envelope.ok_or_else(|| (ReasonCode::FieldMissing, "envelope is required".to_owned()))?;
    if files.is_empty() {
        return Err((ReasonCode::FieldMissing, "files are required".to_owned()));
    }
    Ok(MultipartInput { envelope, files })
}

async fn bounded_part(
    mut field: axum::extract::multipart::Field<'_>,
) -> Result<Vec<u8>, (ReasonCode, String)> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(|_| {
        (
            ReasonCode::MultipartMalformed,
            "cannot read multipart part".to_owned(),
        )
    })? {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|len| len > MAX_PART_BYTES)
        {
            return Err((
                ReasonCode::MultipartPartTooLarge,
                "multipart part exceeds 64 MiB".to_owned(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

struct Envelope {
    day: String,
    segment: String,
    source: String,
    meta: Map<String, Value>,
    files: Vec<IncomingFile>,
}

fn parse_envelope(text: String, raw_files: Vec<RawFile>) -> Result<Envelope, (ReasonCode, String)> {
    let Value::Object(mut root) = serde_json::from_str::<Value>(&text).map_err(|_| {
        (
            ReasonCode::EnvelopeInvalid,
            "envelope is not JSON object".to_owned(),
        )
    })?
    else {
        return Err((
            ReasonCode::EnvelopeInvalid,
            "envelope is not an object".to_owned(),
        ));
    };
    if root.contains_key("observer") {
        return Err((
            ReasonCode::LegacyObserverField,
            "legacy observer field is not accepted".to_owned(),
        ));
    }
    if root.contains_key("stream") {
        return Err((
            ReasonCode::LegacyStreamField,
            "legacy stream field is not accepted".to_owned(),
        ));
    }
    let day = required_string(&mut root, "day")?;
    validate_day(&day).map_err(|code| (code, "day must be YYYYMMDD".to_owned()))?;
    let segment = required_string(&mut root, "segment")?;
    validate_segment(&segment).map_err(|code| (code, "segment must be HHMMSS_LEN".to_owned()))?;
    let source = match root.remove("source") {
        None => String::new(),
        Some(Value::String(source)) => source,
        Some(_) => {
            return Err((
                ReasonCode::SourceInvalidCharacter,
                "source must be a string".to_owned(),
            ));
        }
    };
    let source =
        validate_source(source.as_bytes()).map_err(|code| (code, "invalid source".to_owned()))?;
    let meta = match root.remove("meta") {
        None => Map::new(),
        Some(Value::Object(meta)) => meta,
        Some(_) => {
            return Err((
                ReasonCode::EnvelopeInvalid,
                "meta must be an object".to_owned(),
            ));
        }
    };
    let Value::Array(entries) = root
        .remove("files")
        .ok_or_else(|| (ReasonCode::FieldMissing, "files are required".to_owned()))?
    else {
        return Err((
            ReasonCode::FileMetadataInvalid,
            "files metadata must be an array".to_owned(),
        ));
    };
    if entries.len() != raw_files.len() {
        return Err((
            ReasonCode::FileNameMismatch,
            "envelope files do not match file parts".to_owned(),
        ));
    }
    let mut raw_by_name: BTreeMap<String, RawFile> = BTreeMap::new();
    for raw in raw_files {
        if raw_by_name.insert(raw.filename.clone(), raw).is_some() {
            return Err((
                ReasonCode::FileNameDuplicate,
                "duplicate multipart filename".to_owned(),
            ));
        }
    }
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for entry in entries {
        let Value::Object(mut entry) = entry else {
            return Err((
                ReasonCode::FileMetadataInvalid,
                "file metadata must be an object".to_owned(),
            ));
        };
        let submitted = match entry.remove("submitted") {
            Some(Value::String(value)) => value,
            Some(_) => {
                return Err((
                    ReasonCode::FileMetadataInvalid,
                    "submitted must be a string".to_owned(),
                ));
            }
            None => {
                return Err((
                    ReasonCode::FieldMissing,
                    "file submitted is required".to_owned(),
                ));
            }
        };
        if !seen.insert(submitted.clone()) {
            return Err((
                ReasonCode::FileNameDuplicate,
                "duplicate submitted filename".to_owned(),
            ));
        }
        ContentName::new(&submitted).map_err(|_| {
            (
                ReasonCode::FileNameInvalid,
                "invalid submitted filename".to_owned(),
            )
        })?;
        let raw = raw_by_name.remove(&submitted).ok_or_else(|| {
            (
                ReasonCode::FileNameMismatch,
                "missing matching file part".to_owned(),
            )
        })?;
        entry.remove("written");
        entry.remove("size");
        entry.remove("sha256");
        files.push(IncomingFile {
            submitted,
            bytes: raw.bytes,
            descriptor_extra: entry,
        });
    }
    Ok(Envelope {
        day,
        segment,
        source,
        meta,
        files,
    })
}

fn required_string(
    root: &mut Map<String, Value>,
    name: &str,
) -> Result<String, (ReasonCode, String)> {
    match root.remove(name) {
        None => Err((ReasonCode::FieldMissing, format!("{name} is required"))),
        Some(Value::String(value)) => Ok(value),
        Some(_) => Err((
            if name == "day" {
                ReasonCode::DayInvalid
            } else {
                ReasonCode::SegmentInvalid
            },
            format!("{name} must be a string"),
        )),
    }
}

/// Write one multipart envelope through the segment crate's single-file door.
///
/// `solstone-core-segment` deliberately offers exclusive writes per file, not
/// a batch transaction. A multi-file request can therefore hold earlier files
/// before a later conflict, leaving them without an event or processing signal
/// for that attempt. This is bounded and self-healing: exclusive writes are
/// idempotent, so a retry sees those files as `AlreadyHeld` and can complete
/// with its corroborating event and stream advance. A transactional repair
/// requires a new segment-crate batch primitive and is out of scope here.
fn write_envelope(state: &IngestState, did: &str, stream: String, envelope: Envelope) -> Response {
    let requested = envelope.segment.clone();
    let content_identity = envelope.files.iter().any(|file| is_media(&file.submitted));
    for offset in 0..MAX_SEGMENT_ATTEMPTS {
        let Some(segment_key) = allocated_segment(&requested, offset) else {
            return outcome_error(
                "failed",
                ReasonCode::SegmentAllocationFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "segment allocation overflow",
            );
        };
        let segment =
            match SegmentDir::resolve(&state.journal_root, &envelope.day, &segment_key, &stream) {
                Ok(segment) => segment,
                Err(_) => {
                    return outcome_error(
                        "failed",
                        ReasonCode::JournalWriteFailed,
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "cannot resolve journal segment",
                    );
                }
            };
        let mut descriptors = Vec::new();
        let mut written = 0usize;
        let mut content_conflict = false;
        let mut sidecar_conflict = false;
        for file in &envelope.files {
            let name = match ContentName::new(&file.submitted) {
                Ok(name) => name,
                Err(_) => {
                    return outcome_error(
                        "failed",
                        ReasonCode::FileNameInvalid,
                        StatusCode::BAD_REQUEST,
                        "invalid file name",
                    );
                }
            };
            match write_content(&segment, name, &file.bytes) {
                Ok(ContentWriteOutcome::Written(content)) => {
                    written += 1;
                    descriptors.push(descriptor(
                        file,
                        content.name.as_str(),
                        content.size,
                        content.sha256,
                    ));
                }
                Ok(ContentWriteOutcome::AlreadyHeld(content)) => descriptors.push(descriptor(
                    file,
                    content.name.as_str(),
                    content.size,
                    content.sha256,
                )),
                Ok(ContentWriteOutcome::Conflict { .. })
                    if is_media(&file.submitted) || !content_identity =>
                {
                    content_conflict = true;
                    break;
                }
                Ok(ContentWriteOutcome::Conflict { .. }) => {
                    sidecar_conflict = true;
                    break;
                }
                Err(_) => {
                    return outcome_error(
                        "failed",
                        ReasonCode::JournalWriteFailed,
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "cannot write journal content",
                    );
                }
            }
        }
        if content_conflict {
            continue;
        }
        if sidecar_conflict {
            return outcome_error(
                "conflict",
                ReasonCode::ContentConflict,
                StatusCode::CONFLICT,
                "held sidecar bytes conflict",
            );
        }
        let outcome = if written == 0 {
            "duplicate"
        } else if offset == 0 {
            "ok"
        } else {
            "collision"
        };
        let event = DeviceIngestEvent {
            record_type: "device_ingest".to_owned(),
            record_version: 1,
            outcome: if outcome == "duplicate" {
                "duplicate".to_owned()
            } else {
                "accepted".to_owned()
            },
            protocol_version: 3,
            did: did.to_owned(),
            source: envelope.source.clone(),
            stream: stream.clone(),
            day: envelope.day.clone(),
            segment: segment_key.clone(),
            files: descriptors.clone(),
            meta: envelope.meta.clone(),
        };
        if append_event(&segment, &event).is_err() {
            return outcome_error(
                "failed",
                ReasonCode::EventAppendFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot append ingest event",
            );
        }
        if outcome != "duplicate"
            && advance_stream(
                &stream,
                &envelope.day,
                &segment_key,
                &segment,
                StreamHints {
                    kind: Some("device_ingest".to_owned()),
                    host: None,
                    platform: None,
                },
            )
            .is_err()
        {
            return outcome_error(
                "failed",
                ReasonCode::StreamAdvanceFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot advance stream",
            );
        }
        let written_names: Vec<String> = descriptors
            .iter()
            .map(|file| file.written.clone())
            .collect();
        let body = match outcome {
            "duplicate" => {
                json!({"status":"duplicate", "existing_segment": segment_key, "message":"All files already received", "file_descriptors":descriptors, "meta": envelope.meta})
            }
            "collision" => {
                json!({"status":"collision", "segment":segment_key, "segment_original":requested, "files":written_names, "bytes":total_size(&envelope.files), "file_descriptors":descriptors, "meta":envelope.meta})
            }
            _ => {
                json!({"status":"ok", "segment":segment_key, "files":written_names, "bytes":total_size(&envelope.files), "file_descriptors":descriptors, "meta":envelope.meta})
            }
        };
        return Json(body).into_response();
    }
    outcome_error(
        "failed",
        ReasonCode::SegmentAllocationFailed,
        StatusCode::INTERNAL_SERVER_ERROR,
        "segment allocation attempts exhausted",
    )
}

fn descriptor(file: &IncomingFile, written: &str, size: u64, sha256: String) -> FileDescriptor {
    FileDescriptor {
        submitted: file.submitted.clone(),
        written: written.to_owned(),
        size,
        sha256,
        extra: file.descriptor_extra.clone(),
    }
}
fn total_size(files: &[IncomingFile]) -> u64 {
    files.iter().map(|file| file.bytes.len() as u64).sum()
}
fn is_media(name: &str) -> bool {
    matches!(name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()), Some(ref ext) if matches!(ext.as_str(), "flac" | "opus" | "ogg" | "m4a" | "mp3" | "wav" | "webm" | "mp4" | "mov"))
}
fn allocated_segment(requested: &str, offset: u64) -> Option<String> {
    if offset == 0 {
        return Some(requested.to_owned());
    }
    let (start, len) = requested.split_once('_')?;
    Some(format!(
        "{start}_{}",
        len.parse::<u64>().ok()?.checked_add(offset)?
    ))
}
fn outcome_error(outcome: &str, code: ReasonCode, status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({"status":outcome,"error":"Ingest request failed","reason_code":code.as_str(),"detail":detail}))).into_response()
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceDid};
    use solstone_core_convey_http::serve::{REQUEST_BODY_LIMIT, mux_builder, serve_connection};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    use super::{MAX_PART_BYTES, allocated_segment, router};

    const DID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn root() -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("solstone-core-ingest-{suffix}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn basis(did: &str) -> AccessBasis {
        AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            did: LinkedDeviceDid::try_from(did).unwrap(),
        }
    }

    fn multipart_parts(parts: &[(&str, Option<&str>, &[u8], usize)]) -> (String, Vec<u8>) {
        let boundary = "ingest-boundary";
        let mut body = Vec::new();
        for (name, filename, bytes, extra_headers) in parts {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"")
                    .as_bytes(),
            );
            if let Some(filename) = filename {
                body.extend_from_slice(format!("; filename=\"{filename}\"").as_bytes());
            }
            body.extend_from_slice(b"\r\n");
            for index in 0..*extra_headers {
                body.extend_from_slice(format!("X-Test-{index}: value\r\n").as_bytes());
            }
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    fn multipart(envelope: Value, name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
        let envelope = envelope.to_string();
        multipart_parts(&[
            ("envelope", None, envelope.as_bytes(), 0),
            ("files", Some(name), bytes, 1),
        ])
    }

    #[allow(clippy::too_many_arguments)]
    async fn call(
        app: &axum::Router,
        method: &str,
        uri: &str,
        content_type: Option<String>,
        body: Vec<u8>,
        basis: AccessBasis,
        version: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::from(body))
            .unwrap();
        if let Some(content_type) = content_type {
            request
                .headers_mut()
                .insert(header::CONTENT_TYPE, content_type.parse().unwrap());
        }
        if let Some(version) = version {
            request
                .headers_mut()
                .insert("X-Solstone-Protocol-Version", version.parse().unwrap());
        }
        for (name, value) in extra_headers {
            request.headers_mut().insert(
                name.parse::<header::HeaderName>().unwrap(),
                value.parse().unwrap(),
            );
        }
        request.extensions_mut().insert(basis);
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn call_upload(
        app: &axum::Router,
        envelope: Value,
        name: &str,
        bytes: &[u8],
    ) -> (StatusCode, Value) {
        let (content_type, body) = multipart(envelope, name, bytes);
        call(
            app,
            "POST",
            "/app/observer/ingest",
            Some(content_type),
            body,
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await
    }

    fn envelope(day: &str, segment: &str, files: Value) -> Value {
        json!({"day": day, "segment": segment, "files": files})
    }

    #[tokio::test]
    async fn identity_is_from_access_basis_not_envelope_meta() {
        let root = root();
        let envelope = json!({"day":"20260804","segment":"120000_1","meta":{"did":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"files":[{"submitted":"audio.flac","forward":"kept"}]});
        let app = router(&root);
        let (status, body) = call_upload(&app, envelope, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        let events =
            fs::read_to_string(root.join("chronicle/20260804/120000_1/events.jsonl")).unwrap();
        let event: Value = serde_json::from_str(&events).unwrap();
        assert_eq!(event["did"], DID_A);
        assert_eq!(event["meta"]["did"], DID_B);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn duplicate_records_event_without_second_stream_advance() {
        let root = root();
        let envelope = json!({"day":"20260804","segment":"120000_1","meta":{"unknown":true},"files":[{"submitted":"audio.flac","extension":{"a":1}}]});
        let app = router(&root);
        assert_eq!(
            call_upload(&app, envelope.clone(), "audio.flac", b"sound")
                .await
                .1["status"],
            "ok"
        );
        let (_, body) = call_upload(&app, envelope, "audio.flac", b"sound").await;
        assert_eq!(body["status"], "duplicate");
        assert_eq!(body["meta"]["unknown"], true);
        assert_eq!(body["file_descriptors"][0]["extension"], json!({"a": 1}));
        let events =
            fs::read_to_string(root.join("chronicle/20260804/120000_1/events.jsonl")).unwrap();
        assert_eq!(events.lines().count(), 2);
        let last: Value = serde_json::from_str(events.lines().last().unwrap()).unwrap();
        assert_eq!(last["meta"]["unknown"], true);
        assert_eq!(last["files"][0]["extension"], json!({"a": 1}));
        let (_, manifest) = call(
            &app,
            "GET",
            "/app/observer/ingest/manifest",
            None,
            Vec::new(),
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(manifest["days"]["20260804"]["segments"], 1);
        let (_, segments) = call(
            &app,
            "GET",
            "/app/observer/ingest/segments/20260804",
            None,
            Vec::new(),
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(segments["total"], 1);
        assert_eq!(segments["items"][0]["key"], "120000_1");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn legacy_key_route_is_not_registered() {
        let root = root();
        let mut request = Request::builder()
            .uri("/app/observer/api/deadbeef/key")
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(basis(DID_A));
        let response = router(&root).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let production = include_str!("router.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!production.contains("/api/"));
        assert!(!production.contains("/key"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn oversized_multipart_part_has_its_own_refusal() {
        let root = root();
        let app = router(&root);
        let encoded =
            envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}])).to_string();
        let large = vec![b'x'; MAX_PART_BYTES + 1];
        let (content_type, body) = multipart_parts(&[
            ("envelope", None, encoded.as_bytes(), 0),
            ("files", Some("audio.flac"), &large, 1),
        ]);
        assert_eq!(
            call(
                &app,
                "POST",
                "/app/observer/ingest",
                Some(content_type),
                body,
                basis(DID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_part_too_large"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn excess_multipart_files_have_their_own_refusal() {
        let root = root();
        let app = router(&root);
        let names: Vec<String> = (0..9).map(|index| format!("file{index}.flac")).collect();
        let files = Value::Array(names.iter().map(|name| json!({"submitted":name})).collect());
        let encoded = envelope("20260804", "120001_1", files).to_string();
        let mut parts = vec![("envelope".to_owned(), None, encoded.into_bytes(), 0)];
        for name in &names {
            parts.push(("files".to_owned(), Some(name.clone()), b"x".to_vec(), 1));
        }
        let refs: Vec<_> = parts
            .iter()
            .map(|(name, filename, bytes, headers)| {
                (
                    name.as_str(),
                    filename.as_deref(),
                    bytes.as_slice(),
                    *headers,
                )
            })
            .collect();
        let (content_type, body) = multipart_parts(&refs);
        assert_eq!(
            call(
                &app,
                "POST",
                "/app/observer/ingest",
                Some(content_type),
                body,
                basis(DID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_too_many_files"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn excess_multipart_parts_have_their_own_refusal() {
        let root = root();
        let app = router(&root);
        let encoded =
            envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}])).to_string();
        let mut parts = vec![
            ("envelope".to_owned(), None, encoded.into_bytes(), 0),
            (
                "files".to_owned(),
                Some("audio.flac".to_owned()),
                b"x".to_vec(),
                1,
            ),
        ];
        for _ in 0..11 {
            parts.push(("ignored".to_owned(), None, b"x".to_vec(), 0));
        }
        let refs: Vec<_> = parts
            .iter()
            .map(|(name, filename, bytes, headers)| {
                (
                    name.as_str(),
                    filename.as_deref(),
                    bytes.as_slice(),
                    *headers,
                )
            })
            .collect();
        let (content_type, body) = multipart_parts(&refs);
        assert_eq!(
            call(
                &app,
                "POST",
                "/app/observer/ingest",
                Some(content_type),
                body,
                basis(DID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_too_many_parts"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn oversized_filename_has_its_own_refusal() {
        let root = root();
        let app = router(&root);
        let name = "a".repeat(129);
        let encoded = envelope("20260804", "120002_1", json!([{"submitted":name}])).to_string();
        let (content_type, body) = multipart_parts(&[
            ("envelope", None, encoded.as_bytes(), 0),
            ("files", Some(&name), b"x", 1),
        ]);
        assert_eq!(
            call(
                &app,
                "POST",
                "/app/observer/ingest",
                Some(content_type),
                body,
                basis(DID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_filename_too_long"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn excess_multipart_headers_have_their_own_refusal() {
        let root = root();
        let app = router(&root);
        let encoded =
            envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}])).to_string();
        let (content_type, body) = multipart_parts(&[
            ("envelope", None, encoded.as_bytes(), 16),
            ("files", Some("audio.flac"), b"x", 1),
        ]);
        assert_eq!(
            call(
                &app,
                "POST",
                "/app/observer/ingest",
                Some(content_type),
                body,
                basis(DID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_too_many_headers"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn collision_preserves_original_content_and_remaps_segment() {
        let root = root();
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        assert_eq!(
            call_upload(&app, request.clone(), "audio.flac", b"first")
                .await
                .1["status"],
            "ok"
        );
        let (_, body) = call_upload(&app, request, "audio.flac", b"second").await;
        assert_eq!(body["status"], "collision");
        assert_ne!(body["segment"], body["segment_original"]);
        assert_eq!(
            fs::read(root.join("chronicle/20260804/120000_1/audio.flac")).unwrap(),
            b"first"
        );
        assert_eq!(
            fs::read(
                root.join("chronicle/20260804")
                    .join(body["segment"].as_str().unwrap())
                    .join("audio.flac")
            )
            .unwrap(),
            b"second"
        );
        let remapped = body["segment"].as_str().unwrap();
        let (_, day) = call(
            &app,
            "GET",
            "/app/observer/ingest/manifest/20260804",
            None,
            Vec::new(),
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(
            day["segments"][remapped]["files"][0]["submitted"],
            "audio.flac"
        );
        let (_, segments) = call(
            &app,
            "GET",
            "/app/observer/ingest/segments/20260804",
            None,
            Vec::new(),
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await;
        assert!(
            segments["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["key"] == remapped)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn sidecar_conflict_is_distinct_from_media_collision() {
        let root = root();
        let app = router(&root);
        let request = envelope(
            "20260804",
            "120000_1",
            json!([{"submitted":"audio.flac"},{"submitted":"notes.json"}]),
        );
        let raw = request.to_string();
        let (content_type, body) = multipart_parts(&[
            ("envelope", None, raw.as_bytes(), 0),
            ("files", Some("audio.flac"), b"sound", 1),
            ("files", Some("notes.json"), b"one", 1),
        ]);
        assert_eq!(
            call(
                &app,
                "POST",
                "/app/observer/ingest",
                Some(content_type),
                body,
                basis(DID_A),
                Some("3"),
                &[]
            )
            .await
            .1["status"],
            "ok"
        );
        let raw = request.to_string();
        let (content_type, body) = multipart_parts(&[
            ("envelope", None, raw.as_bytes(), 0),
            ("files", Some("audio.flac"), b"sound", 1),
            ("files", Some("notes.json"), b"two", 1),
        ]);
        let (status, body) = call(
            &app,
            "POST",
            "/app/observer/ingest",
            Some(content_type),
            body,
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["status"], "conflict");
        assert_eq!(body["reason_code"], "content_conflict");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn overflowed_segment_allocation_reaches_failed_path() {
        assert_eq!(allocated_segment("120000_18446744073709551615", 1), None);
    }

    #[tokio::test]
    async fn localhost_and_bearer_headers_do_not_supply_identity() {
        let root = root();
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (content_type, body) = multipart(request.clone(), "audio.flac", b"sound");
        let (status, body) = call(
            &app,
            "POST",
            "/app/observer/ingest",
            Some(content_type),
            body,
            AccessBasis::Localhost,
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason_code"], "linked_device_required");
        let (content_type, body) = multipart(request, "audio.flac", b"sound");
        let (status, body) = call(
            &app,
            "POST",
            "/app/observer/ingest",
            Some(content_type),
            body,
            basis(DID_A),
            Some("3"),
            &[
                ("Authorization", "Bearer spoofed"),
                ("X-Solstone-Observer", "spoofed"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        let event: Value = serde_json::from_str(
            &fs::read_to_string(root.join("chronicle/20260804/120000_1/events.jsonl")).unwrap(),
        )
        .unwrap();
        assert_eq!(event["did"], DID_A);
        let (_, read_body) = call(
            &app,
            "GET",
            "/app/observer/ingest/manifest",
            None,
            Vec::new(),
            AccessBasis::Localhost,
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(read_body["reason_code"], "linked_device_required");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn legacy_fields_and_protocol_versions_are_refused() {
        let root = root();
        let app = router(&root);
        for (field, code) in [
            ("stream", "legacy_stream_field"),
            ("observer", "legacy_observer_field"),
        ] {
            let mut request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
            request
                .as_object_mut()
                .unwrap()
                .insert(field.to_owned(), json!("legacy"));
            let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["reason_code"], code);
        }
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        for (version, code, status) in [
            (None, "protocol_version_required", StatusCode::BAD_REQUEST),
            (
                Some("three"),
                "protocol_version_malformed",
                StatusCode::BAD_REQUEST,
            ),
            (
                Some("2"),
                "protocol_version_legacy",
                StatusCode::UPGRADE_REQUIRED,
            ),
            (
                Some("4"),
                "protocol_version_future",
                StatusCode::UPGRADE_REQUIRED,
            ),
        ] {
            let (content_type, body) = multipart(request.clone(), "audio.flac", b"sound");
            let (actual, body) = call(
                &app,
                "POST",
                "/app/observer/ingest",
                Some(content_type),
                body,
                basis(DID_A),
                version,
                &[],
            )
            .await;
            assert_eq!(actual, status);
            assert_eq!(body["reason_code"], code);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn identity_is_independent_for_each_request() {
        let root = root();
        let app = router(&root);
        for (did, day, segment) in [
            (DID_A, "20260804", "120000_1"),
            (DID_B, "20260805", "120001_1"),
        ] {
            let request = envelope(day, segment, json!([{"submitted":"audio.flac"}]));
            let (content_type, body) = multipart(request, "audio.flac", b"sound");
            assert_eq!(
                call(
                    &app,
                    "POST",
                    "/app/observer/ingest",
                    Some(content_type),
                    body,
                    basis(did),
                    Some("3"),
                    &[]
                )
                .await
                .1["status"],
                "ok"
            );
        }
        let first =
            fs::read_to_string(root.join("chronicle/20260804/120000_1/events.jsonl")).unwrap();
        let second =
            fs::read_to_string(root.join("chronicle/20260805/120001_1/events.jsonl")).unwrap();
        assert!(first.contains(DID_A));
        assert!(!first.contains(DID_B));
        assert!(second.contains(DID_B));
        assert!(!second.contains(DID_A));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn read_routes_return_event_provenance_and_present_custody() {
        let root = root();
        let app = router(&root);
        let request = envelope(
            "20260804",
            "120000_1",
            json!([{"submitted":"audio.flac","forward":{"version":1}}]),
        );
        assert_eq!(
            call_upload(&app, request, "audio.flac", b"sound").await.1["status"],
            "ok"
        );
        let (_, manifest) = call(
            &app,
            "GET",
            "/app/observer/ingest/manifest",
            None,
            Vec::new(),
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(manifest["days"]["20260804"]["segments"], 1);
        let (_, day) = call(
            &app,
            "GET",
            "/app/observer/ingest/manifest/20260804",
            None,
            Vec::new(),
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(
            day["segments"]["120000_1"]["files"][0]["submitted"],
            "audio.flac"
        );
        assert_eq!(
            day["segments"]["120000_1"]["files"][0]["written"],
            "audio.flac"
        );
        assert_eq!(day["segments"]["120000_1"]["files"][0]["size"], 5);
        assert!(day["segments"]["120000_1"]["files"][0]["sha256"].is_string());
        let (_, segments) = call(
            &app,
            "GET",
            "/app/observer/ingest/segments/20260804",
            None,
            Vec::new(),
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(segments["items"][0]["files"][0]["status"], "present");
        let (status, refusal) = call(
            &app,
            "GET",
            "/app/observer/ingest/segments/20260804",
            None,
            Vec::new(),
            basis(DID_A),
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(refusal["reason_code"], "protocol_version_required");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn manifest_surfaces_malformed_device_ingest_events() {
        let root = root();
        fs::create_dir_all(root.join("chronicle/20260804/120000_1")).unwrap();
        fs::write(
            root.join("chronicle/20260804/120000_1/events.jsonl"),
            "{\"record_type\":\"device_ingest\"}\n",
        )
        .unwrap();
        let app = router(&root);
        let (status, body) = call(
            &app,
            "GET",
            "/app/observer/ingest/manifest",
            None,
            Vec::new(),
            basis(DID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "ingest_event_log_malformed");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn shared_connection_limit_rejects_oversized_declared_body() {
        let root = root();
        let (server, mut client) = tokio::io::duplex(128 * 1024);
        let task = tokio::spawn(async move {
            serve_connection(server, router(&root), basis(DID_A), &mux_builder())
                .await
                .unwrap();
        });
        let request = format!(
            "POST /app/observer/ingest HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            REQUEST_BODY_LIMIT + 1
        );
        client.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        task.await.unwrap();
        assert!(
            String::from_utf8(response)
                .unwrap()
                .starts_with("HTTP/1.1 413")
        );
    }
}
