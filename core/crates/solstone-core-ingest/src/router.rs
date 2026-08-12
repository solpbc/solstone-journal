// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Extension, FromRequest, Multipart, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Map, Value, json};
use solstone_core_callosum::{
    DeviceIngestEvent, DurableEvent, FileDescriptor, append_durable_event,
};
use solstone_core_convey_http::envelope::{error_envelope, not_found_fallback};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_ingest_resolve::{
    AppliedDisposition, AppliedFile, ApplyError, ApplyResult, ConflictPlan, FailedPlan,
    HeldEvidence, IngestFile, IngestNotice, IngestNotifier, LoggingIngestNotifier, Resolution,
    apply_plan, quarantine_failed, resolve_ingest,
};
use solstone_core_segment::{ContentName, Kind, StreamHints, advance_bound_stream, bind_stream};
use tower_http::limit::RequestBodyLimitLayer;

use crate::model::{IncomingFile, ReasonCode};
use crate::paths::{
    INGEST_MANIFEST_DAY_PATH, INGEST_MANIFEST_PATH, INGEST_SEGMENTS_DAY_PATH, INGEST_UPLOAD_PATH,
};
use crate::read_routes::{ingest_manifest, ingest_manifest_day, ingest_segments};
use crate::validation::{
    validate_access, validate_day, validate_protocol, validate_segment, validate_source,
};

const MAX_PART_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 8;
const MAX_PARTS: usize = 12;
const MAX_FILENAME_BYTES: usize = 128;
const MAX_HEADERS: usize = 16;
#[derive(Clone)]
pub(crate) struct IngestState {
    pub(crate) journal_root: PathBuf,
    pub(crate) notifier: Arc<dyn IngestNotifier>,
}

/// Build the four linked-device segment-arrival routes.
pub fn router(journal_root: impl AsRef<Path>) -> Router {
    router_with_notifier(journal_root, Arc::new(LoggingIngestNotifier))
}

pub fn router_with_notifier(
    journal_root: impl AsRef<Path>,
    notifier: Arc<dyn IngestNotifier>,
) -> Router {
    Router::new()
        .route(INGEST_UPLOAD_PATH, post(ingest_upload))
        .route(INGEST_MANIFEST_PATH, get(ingest_manifest))
        .route(INGEST_MANIFEST_DAY_PATH, get(ingest_manifest_day))
        .route(INGEST_SEGMENTS_DAY_PATH, get(ingest_segments))
        // The three 128 MiB request-body layers do not widen a file part:
        // `MAX_PART_BYTES` remains the binding 64 MiB per-file limit.
        .layer(DefaultBodyLimit::max(128 * 1024 * 1024))
        .layer(RequestBodyLimitLayer::new(128 * 1024 * 1024))
        .with_state(IngestState {
            journal_root: journal_root.as_ref().to_path_buf(),
            notifier,
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
    write_envelope(&state, &did, envelope)
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

/// The device display label is not carried on the wire at this protocol
/// version. An empty label lets `bind_stream` fall back to its own default,
/// disambiguated per (did, source) exactly like any other label.
const STREAM_LABEL: &str = "";

/// Write one multipart envelope through the resolve/apply segment boundary.
///
/// `solstone-core-segment` deliberately offers exclusive writes per file, not
/// a batch transaction. A multi-file request can therefore hold earlier files
/// before a later conflict, leaving them without an event or processing signal
/// for that attempt. This is bounded and self-healing: resolution re-enters
/// exactly once when apply detects drift, so earlier idempotent writes become
/// held files on the fresh plan. A second consecutive drift surfaces honestly
/// as an error; it is not silently retried a third time. A transactional repair
/// requires a new segment-crate batch primitive and is out of scope here.
///
/// The stream identity is bound once, up front, and advanced once, for
/// whichever segment key the content actually lands under. A rare
/// `StreamBindingConflict` at advance time (a non-native writer hijacked the
/// binding between the bind-time check and this call) surfaces as a `failed`
/// outcome; it is self-healing the same way, since the content and its event
/// are already durably written and idempotent by the time it can occur.
fn write_envelope(state: &IngestState, did: &str, envelope: Envelope) -> Response {
    let hints = StreamHints {
        kind: Some(Kind::Observed),
        host: None,
        platform: None,
    };
    // Bind the (did, source)-owned stream identity once, up front. The chain
    // is advanced separately, below, only once we know which segment key the
    // content actually landed under — never once per collision-retry attempt.
    let bound = match bind_stream(
        &state.journal_root,
        &envelope.day,
        &envelope.segment,
        STREAM_LABEL,
        did,
        &envelope.source,
        &hints,
    ) {
        Ok(bound) => bound,
        Err(_) => {
            return outcome_error(
                "failed",
                ReasonCode::JournalWriteFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot resolve journal stream",
            );
        }
    };
    let requested = envelope.segment.clone();
    let files = match envelope
        .files
        .iter()
        .map(|file| {
            ContentName::new(&file.submitted)
                .map(|name| IngestFile {
                    name,
                    bytes: file.bytes.as_slice(),
                })
                .map_err(|_| ())
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(files) => files,
        Err(()) => {
            return outcome_error(
                "failed",
                ReasonCode::FileNameInvalid,
                StatusCode::BAD_REQUEST,
                "invalid file name",
            );
        }
    };
    let applied = match resolve_and_apply(
        state,
        &envelope.day,
        &bound.stream,
        &requested,
        &files,
        true,
    ) {
        Ok(ApplyPhase::Applied(result)) => result,
        Ok(ApplyPhase::Conflict(_plan)) => {
            return outcome_error(
                "conflict",
                ReasonCode::ContentConflict,
                StatusCode::CONFLICT,
                "held sidecar bytes conflict",
            );
        }
        Ok(ApplyPhase::Failed(plan)) => {
            if quarantine_failed(&state.journal_root, &envelope.day, &plan, &files).is_err() {
                return outcome_error(
                    "failed",
                    ReasonCode::JournalWriteFailed,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cannot quarantine failed ingest",
                );
            }
            return outcome_error(
                "failed",
                ReasonCode::SegmentAllocationFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "segment allocation attempts exhausted",
            );
        }
        Err(_) => {
            return outcome_error(
                "failed",
                ReasonCode::JournalWriteFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot resolve or write journal content",
            );
        }
    };
    let descriptors = descriptors(&envelope.files, &applied.files);
    let outcome = match applied.status {
        solstone_core_ingest_resolve::PlanStatus::Ok => "ok",
        solstone_core_ingest_resolve::PlanStatus::Collision => "collision",
        solstone_core_ingest_resolve::PlanStatus::Duplicate => "duplicate",
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
        stream: bound.stream.clone(),
        day: envelope.day.clone(),
        segment: applied.landed_segment.clone(),
        files: descriptors.clone(),
        meta: envelope.meta.clone(),
        extra: Map::new(),
    };
    let durable_event = DurableEvent::DeviceIngest(event);
    if append_durable_event(applied.segment.path(), &durable_event).is_err() {
        return outcome_error(
            "failed",
            ReasonCode::EventAppendFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot append ingest event",
        );
    }
    let advance_error = if applied.should_advance {
        advance_bound_stream(
            &bound.stream,
            &envelope.day,
            &applied.landed_segment,
            &applied.segment,
            hints.clone(),
            did,
            &envelope.source,
        )
        .err()
    } else {
        None
    };
    // Notification deliberately follows stream advancement, matching the
    // Python route order even though today's payload needs no advance data.
    if applied.should_advance {
        let notice_files: Vec<AppliedFile> = applied
            .files
            .iter()
            .filter(|file| {
                matches!(file.disposition, AppliedDisposition::Written)
                    || (matches!(file.disposition, AppliedDisposition::AlreadyHeld)
                        && file.evidence == Some(HeldEvidence::OnDisk))
            })
            .cloned()
            .collect();
        let notice = IngestNotice {
            did,
            source: &envelope.source,
            day: &envelope.day,
            stream: &bound.stream,
            segment: &applied.landed_segment,
            files: &notice_files,
            meta: &envelope.meta,
        };
        if let Err(error) = state.notifier.notify(&notice) {
            eprintln!("observer ingest notification degraded: {error}");
        }
    }
    if advance_error.is_some() {
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
            json!({"status":"duplicate", "existing_segment": applied.landed_segment, "message":"All files already received", "file_descriptors":descriptors, "meta": envelope.meta})
        }
        "collision" => {
            json!({"status":"collision", "segment":applied.landed_segment, "segment_original":requested, "files":written_names, "bytes":total_size(&envelope.files), "file_descriptors":descriptors, "meta":envelope.meta})
        }
        _ => {
            json!({"status":"ok", "segment":applied.landed_segment, "files":written_names, "bytes":total_size(&envelope.files), "file_descriptors":descriptors, "meta":envelope.meta})
        }
    };
    Json(body).into_response()
}

enum ApplyPhase {
    Applied(ApplyResult),
    Conflict(ConflictPlan),
    Failed(FailedPlan),
}

#[cfg(test)]
type BeforeApplyHook = Box<dyn FnMut(&solstone_core_ingest_resolve::ApplyPlan)>;

#[cfg(test)]
thread_local! {
    static BEFORE_APPLY_HOOK: std::cell::RefCell<Option<BeforeApplyHook>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn set_before_apply_hook(hook: impl FnMut(&solstone_core_ingest_resolve::ApplyPlan) + 'static) {
    BEFORE_APPLY_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "test apply hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn clear_before_apply_hook() {
    BEFORE_APPLY_HOOK.with(|slot| *slot.borrow_mut() = None);
}

#[cfg(test)]
fn run_before_apply_hook(plan: &solstone_core_ingest_resolve::ApplyPlan) {
    BEFORE_APPLY_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(plan);
        }
    });
}

fn resolve_and_apply(
    state: &IngestState,
    day: &str,
    stream: &str,
    requested_segment: &str,
    files: &[IngestFile<'_>],
    retry_stale: bool,
) -> Result<ApplyPhase, ()> {
    match resolve_ingest(&state.journal_root, day, stream, requested_segment, files)
        .map_err(|_| ())?
    {
        Resolution::Conflict(plan) => Ok(ApplyPhase::Conflict(plan)),
        Resolution::Failed(plan) => Ok(ApplyPhase::Failed(plan)),
        Resolution::Apply(plan) => {
            #[cfg(test)]
            run_before_apply_hook(&plan);
            match apply_plan(&plan, files) {
                Ok(result) => Ok(ApplyPhase::Applied(result)),
                Err(ApplyError::Stale) if retry_stale => {
                    resolve_and_apply(state, day, stream, requested_segment, files, false)
                }
                Err(_) => Err(()),
            }
        }
    }
}

fn descriptors(files: &[IncomingFile], applied: &[AppliedFile]) -> Vec<FileDescriptor> {
    files
        .iter()
        .zip(applied)
        .map(|(file, applied)| {
            let mut extra = file.descriptor_extra.clone();
            extra.insert(
                "disposition".to_owned(),
                Value::String(
                    match applied.disposition {
                        AppliedDisposition::Written => "written",
                        AppliedDisposition::AlreadyHeld => "already_held",
                        AppliedDisposition::Unwritten => "received_not_written",
                    }
                    .to_owned(),
                ),
            );
            FileDescriptor {
                submitted: file.submitted.clone(),
                written: applied.name.as_str().to_owned(),
                size: applied.size,
                sha256: applied.sha256.clone(),
                extra,
            }
        })
        .collect()
}
fn total_size(files: &[IncomingFile]) -> u64 {
    files.iter().map(|file| file.bytes.len() as u64).sum()
}
fn outcome_error(outcome: &str, code: ReasonCode, status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({"status":outcome,"error":"Ingest request failed","reason_code":code.as_str(),"detail":detail}))).into_response()
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use sha2::Digest;
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceDid};
    use solstone_core_convey_http::serve::{REQUEST_BODY_LIMIT, mux_builder, serve_connection};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    use super::{
        ApplyPhase, IngestState, MAX_PART_BYTES, clear_before_apply_hook, resolve_and_apply,
        router, router_with_notifier, set_before_apply_hook,
    };

    const DID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    struct SpyNotifier {
        calls: AtomicUsize,
        fails: bool,
    }

    struct CapturingNotifier {
        files: Mutex<Vec<Vec<String>>>,
    }

    impl solstone_core_ingest_resolve::IngestNotifier for CapturingNotifier {
        fn notify(
            &self,
            notice: &solstone_core_ingest_resolve::IngestNotice<'_>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.files.lock().unwrap().push(
                notice
                    .files
                    .iter()
                    .map(|file| file.name.as_str().to_owned())
                    .collect(),
            );
            Ok(())
        }
    }

    impl solstone_core_ingest_resolve::IngestNotifier for SpyNotifier {
        fn notify(
            &self,
            _notice: &solstone_core_ingest_resolve::IngestNotice<'_>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fails {
                Err(Box::new(std::io::Error::other("bus unavailable")))
            } else {
                Ok(())
            }
        }
    }

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

    async fn call_uploads(
        app: &axum::Router,
        envelope: Value,
        files: &[(&str, &[u8])],
    ) -> (StatusCode, Value) {
        let envelope = envelope.to_string();
        let mut parts = vec![("envelope", None, envelope.as_bytes(), 0)];
        for (name, bytes) in files {
            parts.push(("files", Some(*name), *bytes, 1));
        }
        let (content_type, body) = multipart_parts(&parts);
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

    #[test]
    fn resolve_and_apply_reenters_once_after_stale_drift() {
        let root = root();
        let state = IngestState {
            journal_root: root.clone(),
            notifier: Arc::new(solstone_core_ingest_resolve::LoggingIngestNotifier),
        };
        let files = [solstone_core_ingest_resolve::IngestFile {
            name: solstone_core_segment::ContentName::new("audio.flac").unwrap(),
            bytes: b"upload",
        }];
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_hook = attempts.clone();
        set_before_apply_hook(move |plan| {
            if attempts_for_hook.fetch_add(1, Ordering::SeqCst) == 0 {
                fs::create_dir_all(plan.segment.path()).unwrap();
                fs::write(plan.segment.path().join("audio.flac"), b"racer").unwrap();
            }
        });

        let result = resolve_and_apply(&state, "20260804", "device", "120000_1", &files, true);
        clear_before_apply_hook();
        let ApplyPhase::Applied(result) = result.expect("one fresh re-resolution must succeed")
        else {
            panic!("expected applied result after re-resolution");
        };
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            result.status,
            solstone_core_ingest_resolve::PlanStatus::Collision
        );
        assert_eq!(result.landed_segment, "120000_2");
        assert_eq!(
            fs::read(root.join("chronicle/20260804/device/120000_1/audio.flac")).unwrap(),
            b"racer"
        );
        assert_eq!(
            fs::read(root.join("chronicle/20260804/device/120000_2/audio.flac")).unwrap(),
            b"upload"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_and_apply_does_not_retry_a_second_stale_plan() {
        let root = root();
        let state = IngestState {
            journal_root: root.clone(),
            notifier: Arc::new(solstone_core_ingest_resolve::LoggingIngestNotifier),
        };
        let files = [solstone_core_ingest_resolve::IngestFile {
            name: solstone_core_segment::ContentName::new("audio.flac").unwrap(),
            bytes: b"upload",
        }];
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_hook = attempts.clone();
        set_before_apply_hook(move |plan| {
            attempts_for_hook.fetch_add(1, Ordering::SeqCst);
            fs::create_dir_all(plan.segment.path()).unwrap();
            fs::write(plan.segment.path().join("audio.flac"), b"racer").unwrap();
        });

        assert!(resolve_and_apply(&state, "20260804", "device", "120000_1", &files, true).is_err());
        clear_before_apply_hook();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(
            !root
                .join("chronicle/20260804/device/120000_3/audio.flac")
                .exists()
        );
        let _ = fs::remove_dir_all(root);
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
            fs::read_to_string(root.join("chronicle/20260804/device/120000_1/events.jsonl"))
                .unwrap();
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
            fs::read_to_string(root.join("chronicle/20260804/device/120000_1/events.jsonl"))
                .unwrap();
        assert_eq!(events.lines().count(), 2);
        let last: Value = serde_json::from_str(events.lines().last().unwrap()).unwrap();
        assert_eq!(last["meta"]["unknown"], true);
        assert_eq!(last["files"][0]["extension"], json!({"a": 1}));
        assert_eq!(
            last["files"][0]["sha256"],
            format!("{:x}", sha2::Sha256::digest(b"sound"))
        );
        assert_eq!(last["files"][0]["disposition"], "already_held");
        let stream_record: Value =
            serde_json::from_str(&fs::read_to_string(root.join("streams/device.json")).unwrap())
                .unwrap();
        assert_eq!(
            stream_record["seq"], 1,
            "a byte-identical duplicate upload must not advance the stream chain a second time"
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
            day["segments"]["120000_1"]["files"][0]["sha256"],
            format!("{:x}", sha2::Sha256::digest(b"sound"))
        );
        assert_eq!(
            day["segments"]["120000_1"]["files"][0]["disposition"],
            "already_held"
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
        assert_eq!(segments["total"], 1);
        assert_eq!(segments["items"][0]["key"], "120000_1");
        assert_eq!(
            segments["items"][0]["files"][0]["sha256"],
            format!("{:x}", sha2::Sha256::digest(b"sound"))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn notification_is_once_for_success_and_never_breaks_durability() {
        for fails in [false, true] {
            let root = root();
            let spy = Arc::new(SpyNotifier {
                calls: AtomicUsize::new(0),
                fails,
            });
            let app = router_with_notifier(&root, spy.clone());
            let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
            let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["status"], "ok");
            assert_eq!(spy.calls.load(Ordering::SeqCst), 1);
            assert!(
                root.join("chronicle/20260804/device/120000_1/events.jsonl")
                    .exists()
            );
            let _ = fs::remove_dir_all(root);
        }
    }

    #[tokio::test]
    async fn notifier_receives_fresh_and_ondisk_held_files() {
        let root = root();
        let notifier = Arc::new(CapturingNotifier {
            files: Mutex::new(Vec::new()),
        });
        let app = router_with_notifier(&root, notifier.clone());
        let initial = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        assert_eq!(
            call_upload(&app, initial, "audio.flac", b"sound").await.0,
            StatusCode::OK
        );

        let request = envelope(
            "20260804",
            "120000_1",
            json!([{"submitted":"audio.flac"},{"submitted":"fresh.flac"}]),
        );
        assert_eq!(
            call_uploads(
                &app,
                request,
                &[("audio.flac", b"sound"), ("fresh.flac", b"fresh")],
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            notifier.files.lock().unwrap().last(),
            Some(&vec!["audio.flac".to_owned(), "fresh.flac".to_owned()])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn notifier_excludes_terminal_proof_held_files() {
        let root = root();
        let notifier = Arc::new(CapturingNotifier {
            files: Mutex::new(Vec::new()),
        });
        let app = router_with_notifier(&root, notifier.clone());
        let initial = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        assert_eq!(
            call_upload(&app, initial, "audio.flac", b"sound").await.0,
            StatusCode::OK
        );
        let segment = root.join("chronicle/20260804/device/120000_1");
        fs::remove_file(segment.join("audio.flac")).unwrap();
        fs::write(
            segment.join("audio.jsonl"),
            json!({"_solstone_processing":{
                "schema":"solstone.processing.v1",
                "state":"analyzed",
                "handler":"transcribe",
                "input_size":5
            }})
            .to_string()
                + "\n",
        )
        .unwrap();

        let request = envelope(
            "20260804",
            "120000_1",
            json!([{"submitted":"audio.flac"},{"submitted":"fresh.flac"}]),
        );
        assert_eq!(
            call_uploads(
                &app,
                request,
                &[("audio.flac", b"sound"), ("fresh.flac", b"fresh")],
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            notifier.files.lock().unwrap().last(),
            Some(&vec!["fresh.flac".to_owned()])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn exhausted_resolution_quarantines_without_history_or_notification() {
        let root = root();
        for offset in 0..solstone_core_ingest_resolve::MAX_INGEST_SEGMENT_ATTEMPTS {
            let directory = root
                .join("chronicle/20260804/device")
                .join(format!("120000_{}", 1 + offset));
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("audio.flac"), b"old").unwrap();
        }
        let spy = Arc::new(SpyNotifier {
            calls: AtomicUsize::new(0),
            fails: false,
        });
        let app = router_with_notifier(&root, spy.clone());
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request, "audio.flac", b"new").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["status"], "failed");
        let quarantines: Vec<_> =
            fs::read_dir(root.join("chronicle/20260804/observer/failed/120000_1"))
                .unwrap()
                .collect();
        assert_eq!(quarantines.len(), 1);
        assert_eq!(
            fs::read(quarantines[0].as_ref().unwrap().path().join("audio.flac")).unwrap(),
            b"new"
        );
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
        assert!(
            fs::read_dir(root.join("chronicle/20260804/device"))
                .unwrap()
                .all(|entry| !entry.unwrap().path().join("events.jsonl").exists())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn append_failure_is_a_hard_error_and_never_notifies() {
        let root = root();
        let segment = root.join("chronicle/20260804/device/120000_1");
        fs::create_dir_all(segment.join("events.jsonl")).unwrap();
        let spy = Arc::new(SpyNotifier {
            calls: AtomicUsize::new(0),
            fails: false,
        });
        let app = router_with_notifier(&root, spy.clone());
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "event_append_failed");
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn resolution_io_failure_has_no_history_or_manifest() {
        let root = root();
        let stream = root.join("chronicle/20260804/device");
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        fs::write(&stream, b"blocked").unwrap();
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["status"], "failed");
        assert!(!stream.join("120000_1/events.jsonl").exists());
        assert!(!stream.join("120000_1/ingest.json").exists());
        assert!(!root.join("chronicle/20260804/observer/failed").exists());
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
            fs::read(root.join("chronicle/20260804/device/120000_1/audio.flac")).unwrap(),
            b"first"
        );
        assert_eq!(
            fs::read(
                root.join("chronicle/20260804/device")
                    .join(body["segment"].as_str().unwrap())
                    .join("audio.flac")
            )
            .unwrap(),
            b"second"
        );
        let remapped = body["segment"].as_str().unwrap();
        let stream_record: Value =
            serde_json::from_str(&fs::read_to_string(root.join("streams/device.json")).unwrap())
                .unwrap();
        assert_eq!(
            stream_record["seq"], 2,
            "the collision-remapped write is not a duplicate, so it advances the chain"
        );
        assert_eq!(
            stream_record["last_segment"], remapped,
            "the chain must advance for the segment that actually received the write, not the \
             originally requested one"
        );
        let marker: Value = serde_json::from_str(
            &fs::read_to_string(
                root.join("chronicle/20260804/device")
                    .join(remapped)
                    .join("stream.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(marker["seq"], 2);
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
            &fs::read_to_string(root.join("chronicle/20260804/device/120000_1/events.jsonl"))
                .unwrap(),
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
            fs::read_to_string(root.join("chronicle/20260804/device/120000_1/events.jsonl"))
                .unwrap();
        let second =
            fs::read_to_string(root.join("chronicle/20260805/device_2/120001_1/events.jsonl"))
                .unwrap();
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
    async fn manifest_skips_malformed_device_ingest_events() {
        let root = root();
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        assert_eq!(
            call_upload(&app, request, "audio.flac", b"sound").await.1["status"],
            "ok"
        );
        // Simulate the event log becoming malformed after a valid write, so
        // the bound stream directory it lives under genuinely exists.
        fs::write(
            root.join("chronicle/20260804/device/120000_1/events.jsonl"),
            "{\"record_type\":\"device_ingest\"}\n",
        )
        .unwrap();
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
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["days"], json!({}));
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
