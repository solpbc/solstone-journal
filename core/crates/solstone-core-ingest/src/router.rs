// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Extension, FromRequest, Multipart, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Map, Value, json};
use solstone_core_callosum::{
    CallosumEnvelope, CallosumOneShotSender, DeviceIngestEvent, DurableEvent, FileDescriptor,
    append_durable_event,
};
use solstone_core_convey_http::envelope::{error_envelope, not_found_fallback};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_ingest_contract::{CONNECTION_BODY_LIMIT, MAX_PART_BYTES};
use solstone_core_ingest_resolve::{
    AppliedDisposition, AppliedFile, ApplyError, ApplyResult, ConflictPlan, FailedPlan, IngestFile,
    IngestNotice, IngestNotifier, Resolution, apply_plan, quarantine_failed, resolve_ingest,
};
use solstone_core_observer::store::write::save_observer;
use solstone_core_observer::system_now_ms;
use solstone_core_segment::{
    ContentName, Kind, SegmentDir, StreamHints, advance_bound_stream, hold_source_mutation,
};
use solstone_core_sol_link::ledger::{AcceptedSegment, AuthorizationLedger};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tower_http::limit::RequestBodyLimitLayer;

use crate::model::{IncomingFile, ReasonCode};
use crate::observer_evidence::resolve_device_observer;
use crate::read_routes::{ingest_manifest, ingest_manifest_day, ingest_segments};
use crate::stream_identity::bind_ingest_stream;
use crate::validation::{
    validate_access, validate_day, validate_protocol, validate_segment, validate_source,
};

const MAX_FILES: usize = 8;
const MAX_PARTS: usize = 12;
const MAX_FILENAME_BYTES: usize = 128;
const MAX_HEADERS: usize = 16;
#[derive(Clone)]
pub(crate) struct IngestState {
    pub(crate) journal_root: PathBuf,
    pub(crate) notifier: Arc<dyn IngestNotifier>,
    pub(crate) now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

struct CallosumIngestNotifier {
    sender: CallosumOneShotSender,
}

impl CallosumIngestNotifier {
    fn new(journal_root: impl AsRef<Path>) -> Self {
        Self {
            sender: CallosumOneShotSender::new(
                journal_root.as_ref().join("health/callosum.sock"),
                Duration::from_secs(1),
            ),
        }
    }
}

impl IngestNotifier for CallosumIngestNotifier {
    fn notify(&self, notice: &IngestNotice<'_>) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut extra = Map::new();
        extra.insert("day".to_owned(), Value::String(notice.day.to_owned()));
        extra.insert("stream".to_owned(), Value::String(notice.stream.to_owned()));
        extra.insert(
            "segment".to_owned(),
            Value::String(notice.segment.to_owned()),
        );
        extra.insert(
            "files".to_owned(),
            Value::Array(notice.files.iter().cloned().map(Value::String).collect()),
        );
        extra.insert("batch".to_owned(), Value::Bool(false));
        if let Some(observer) = notice.observer {
            extra.insert("observer".to_owned(), Value::String(observer.to_owned()));
        }
        extra.insert("meta".to_owned(), Value::Object(notice.meta.clone()));
        let envelope = CallosumEnvelope {
            tract: "observe".to_owned(),
            event: "observing".to_owned(),
            ts: None,
            extra,
        };
        let mut line = serde_json::to_string(&envelope)?;
        line.push('\n');
        self.sender.send_line(&line)?;
        Ok(())
    }
}

#[cfg(test)]
struct TestIngestNotifier;

#[cfg(test)]
impl IngestNotifier for TestIngestNotifier {
    fn notify(&self, _notice: &IngestNotice<'_>) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

fn wall_clock_ms() -> Arc<dyn Fn() -> i64 + Send + Sync> {
    Arc::new(system_now_ms)
}

/// Build the mergeable linked-device segment-arrival routes.
pub fn api_router(journal_root: impl AsRef<Path>) -> Router {
    let journal_root = journal_root.as_ref();
    api_router_with_notifier(
        journal_root,
        Arc::new(CallosumIngestNotifier::new(journal_root)),
    )
}

fn api_router_with_notifier(
    journal_root: impl AsRef<Path>,
    notifier: Arc<dyn IngestNotifier>,
) -> Router {
    api_router_with(journal_root, notifier, wall_clock_ms())
}

fn api_router_with(
    journal_root: impl AsRef<Path>,
    notifier: Arc<dyn IngestNotifier>,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
) -> Router {
    Router::new()
        .route("/app/devices/ingest", post(ingest_upload))
        .route("/app/devices/ingest/manifest", get(ingest_manifest))
        .route(
            "/app/devices/ingest/manifest/{day}",
            get(ingest_manifest_day),
        )
        .route("/app/devices/ingest/segments/{day}", get(ingest_segments))
        .layer(DefaultBodyLimit::max(CONNECTION_BODY_LIMIT))
        .layer(RequestBodyLimitLayer::new(CONNECTION_BODY_LIMIT))
        .with_state(IngestState {
            journal_root: journal_root.as_ref().to_path_buf(),
            notifier,
            now_ms,
        })
}

// Crate-local: a public fallback would swallow the shell's unmatched surface on merge.
#[allow(dead_code)]
#[cfg(not(test))]
pub(crate) fn router(journal_root: impl AsRef<Path>) -> Router {
    api_router(journal_root).fallback(not_found_fallback)
}

#[allow(dead_code)]
#[cfg(test)]
pub(crate) fn router(journal_root: impl AsRef<Path>) -> Router {
    api_router_with_notifier(journal_root, Arc::new(TestIngestNotifier))
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
    let cid = match validate_access(&basis) {
        Ok(cid) => cid,
        Err((code, status, detail)) => return refusal(code, status, detail),
    };
    if let Err((code, status, detail)) = validate_protocol(request.headers()) {
        return refusal_with_activity(&state, &cid, code, status, detail);
    }
    let parsed = match parse_multipart(request).await {
        Ok(parsed) => parsed,
        Err((code, detail)) => {
            return refusal_with_activity(&state, &cid, code, StatusCode::BAD_REQUEST, detail);
        }
    };
    let envelope = match parse_envelope(parsed.envelope, parsed.files) {
        Ok(envelope) => envelope,
        Err((code, detail)) => {
            return refusal_with_activity(&state, &cid, code, StatusCode::BAD_REQUEST, detail);
        }
    };
    write_envelope(&state, &cid, envelope)
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

/// Write one multipart envelope through the resolve/apply segment boundary.
///
/// `solstone-core-segment` deliberately offers exclusive writes per file, not
/// a batch transaction. A multi-file request can therefore hold earlier files
/// before a later conflict. A non-Stale apply failure still emits a
/// `device_ingest` event scoped to the files that landed, then returns the
/// failure response. This is bounded and self-healing: resolution re-enters
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
fn write_envelope(state: &IngestState, cid: &str, envelope: Envelope) -> Response {
    let _location_mutation_lock = if envelope.source == "location" {
        // This outermost guard stays live through every path below. The stream,
        // registry, segment, and retention locks reached by ingest are acquired
        // inside it, never before it.
        match hold_source_mutation(&state.journal_root, "location") {
            Ok(lock) => Some(lock),
            Err(error) => {
                log::warn!("location mutation lock unavailable: {error}");
                return finish_ingest_completion(
                    state,
                    cid,
                    IngestCompletion::rejected(
                        ReasonCode::LocationLockUnavailable,
                        outcome_error(
                            "retryable",
                            ReasonCode::LocationLockUnavailable,
                            StatusCode::SERVICE_UNAVAILABLE,
                            "location ingest is temporarily unavailable; retry the request",
                        ),
                    ),
                );
            }
        }
    } else {
        None
    };
    let completion = write_envelope_inner(state, cid, envelope);
    finish_ingest_completion(state, cid, completion)
}

fn write_envelope_inner(state: &IngestState, cid: &str, envelope: Envelope) -> IngestCompletion {
    let hints = StreamHints {
        kind: Some(Kind::Observed),
        host: None,
        platform: None,
    };
    // Bind the (cid, source)-owned stream identity once, up front. The chain
    // is advanced separately, below, only once we know which segment key the
    // content actually landed under — never once per collision-retry attempt.
    let bound = match bind_ingest_stream(
        &state.journal_root,
        &envelope.day,
        &envelope.segment,
        cid,
        &envelope.source,
        &hints,
    ) {
        Ok(bound) => bound,
        Err((code, status, detail)) if status.is_client_error() => {
            return IngestCompletion::rejected(code, refusal(code, status, detail));
        }
        Err((code, status, detail)) => {
            return IngestCompletion::rejected(
                code,
                outcome_error("failed", code, status, &detail),
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
            return IngestCompletion::rejected(
                ReasonCode::FileNameInvalid,
                outcome_error(
                    "failed",
                    ReasonCode::FileNameInvalid,
                    StatusCode::BAD_REQUEST,
                    "invalid file name",
                ),
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
            return IngestCompletion::rejected(
                ReasonCode::ContentConflict,
                outcome_error(
                    "conflict",
                    ReasonCode::ContentConflict,
                    StatusCode::CONFLICT,
                    "held sidecar bytes conflict",
                ),
            );
        }
        Ok(ApplyPhase::Failed(plan)) => {
            if quarantine_failed(&state.journal_root, &envelope.day, &plan, &files).is_err() {
                return IngestCompletion::rejected(
                    ReasonCode::JournalWriteFailed,
                    outcome_error(
                        "failed",
                        ReasonCode::JournalWriteFailed,
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "cannot quarantine failed ingest",
                    ),
                );
            }
            return IngestCompletion::rejected(
                ReasonCode::SegmentAllocationFailed,
                outcome_error(
                    "failed",
                    ReasonCode::SegmentAllocationFailed,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "segment allocation attempts exhausted",
                ),
            );
        }
        Ok(ApplyPhase::Partial(partial)) => {
            return append_partial_and_fail(state, cid, &envelope, &bound.stream, partial);
        }
        Err(_) => {
            return IngestCompletion::rejected(
                ReasonCode::JournalWriteFailed,
                outcome_error(
                    "failed",
                    ReasonCode::JournalWriteFailed,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cannot resolve or write journal content",
                ),
            );
        }
    };
    let descriptors = descriptors(&envelope.files, &applied.files);
    let announced_files = written_descriptors(&applied.files)
        .into_iter()
        .map(|file| file.written)
        .collect::<Vec<_>>();
    let outcome = match applied.status {
        solstone_core_ingest_resolve::PlanStatus::Ok => "ok",
        solstone_core_ingest_resolve::PlanStatus::Collision => "collision",
        solstone_core_ingest_resolve::PlanStatus::Duplicate => "duplicate",
    };
    if !announced_files.is_empty() {
        // A later custody failure intentionally leaves this day dirty.
        if solstone_core_ingest_resolve::bump_stream_marker(&state.journal_root, &envelope.day)
            .is_err()
        {
            return IngestCompletion::rejected(
                ReasonCode::StreamMarkerBumpFailed,
                outcome_error(
                    "failed",
                    ReasonCode::StreamMarkerBumpFailed,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cannot advance stream marker",
                ),
            );
        }
    }
    if append_device_ingest(
        applied.segment.path(),
        cid,
        &envelope,
        &bound.stream,
        &applied.landed_segment,
        descriptors.clone(),
        if outcome == "duplicate" {
            "duplicate"
        } else {
            "accepted"
        },
    )
    .is_err()
    {
        return IngestCompletion::rejected(
            ReasonCode::EventAppendFailed,
            outcome_error(
                "failed",
                ReasonCode::EventAppendFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot append ingest event",
            ),
        );
    }
    if applied.should_advance
        && advance_bound_stream(
            &bound.stream,
            &envelope.day,
            &applied.landed_segment,
            &applied.segment,
            hints.clone(),
            cid,
            &envelope.source,
        )
        .is_err()
    {
        return IngestCompletion::rejected(
            ReasonCode::StreamAdvanceFailed,
            outcome_error(
                "failed",
                ReasonCode::StreamAdvanceFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot advance stream",
            ),
        );
    }
    let observer = match stamp_observer(
        state,
        cid,
        &envelope.day,
        &applied.landed_segment,
        applied.status,
    ) {
        Ok(observer) => observer,
        Err(()) => {
            return IngestCompletion::rejected(
                ReasonCode::ObserverStampFailed,
                outcome_error(
                    "failed",
                    ReasonCode::ObserverStampFailed,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cannot stamp observer receipt",
                ),
            );
        }
    };
    if !announced_files.is_empty() {
        let notice = IngestNotice {
            cid,
            observer: observer.as_deref(),
            source: &envelope.source,
            day: &envelope.day,
            stream: &bound.stream,
            segment: &applied.landed_segment,
            files: &announced_files,
            meta: &envelope.meta,
        };
        if let Err(error) = state.notifier.notify(&notice) {
            log::warn!("observer ingest notification failed: {error}");
            return IngestCompletion::rejected(
                ReasonCode::NotifyFailed,
                outcome_error(
                    "failed",
                    ReasonCode::NotifyFailed,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cannot notify ingest listeners",
                ),
            );
        }
    }
    let written_names: Vec<String> = descriptors
        .iter()
        .map(|file| file.written.clone())
        .collect();
    let accepted_day = envelope.day.clone();
    let accepted_segment = applied.landed_segment.clone();
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
    IngestCompletion::accepted(accepted_day, accepted_segment, Json(body).into_response())
}

enum ApplyPhase {
    Applied(ApplyResult),
    Conflict(ConflictPlan),
    Failed(FailedPlan),
    Partial(PartialApply),
}

struct PartialApply {
    applied: Vec<AppliedFile>,
    landed_segment: String,
    segment: SegmentDir,
}

fn append_partial_and_fail(
    state: &IngestState,
    cid: &str,
    envelope: &Envelope,
    stream: &str,
    partial: PartialApply,
) -> IngestCompletion {
    let descriptors = written_descriptors(&partial.applied);
    let announced_files = descriptors
        .iter()
        .map(|file| file.written.clone())
        .collect::<Vec<_>>();
    if !announced_files.is_empty()
        && solstone_core_ingest_resolve::bump_stream_marker(&state.journal_root, &envelope.day)
            .is_err()
    {
        return IngestCompletion::rejected(
            ReasonCode::StreamMarkerBumpFailed,
            outcome_error(
                "failed",
                ReasonCode::StreamMarkerBumpFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot advance stream marker",
            ),
        );
    }
    if append_device_ingest(
        partial.segment.path(),
        cid,
        envelope,
        stream,
        &partial.landed_segment,
        descriptors,
        "accepted",
    )
    .is_err()
    {
        return IngestCompletion::rejected(
            ReasonCode::EventAppendFailed,
            outcome_error(
                "failed",
                ReasonCode::EventAppendFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot append ingest event",
            ),
        );
    }
    if !announced_files.is_empty() {
        let observer = resolve_device_observer(&state.journal_root, cid)
            .ok()
            .flatten()
            .and_then(|observer| observer.record.name().map(str::to_owned));
        let notice = IngestNotice {
            cid,
            observer: observer.as_deref(),
            source: &envelope.source,
            day: &envelope.day,
            stream,
            segment: &partial.landed_segment,
            files: &announced_files,
            meta: &envelope.meta,
        };
        if let Err(error) = state.notifier.notify(&notice) {
            log::warn!("partial observer ingest notification failed: {error}");
        }
    }
    IngestCompletion::rejected(
        ReasonCode::JournalWriteFailed,
        outcome_error(
            "failed",
            ReasonCode::JournalWriteFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot resolve or write journal content",
        ),
    )
}

fn append_device_ingest(
    segment_path: &Path,
    cid: &str,
    envelope: &Envelope,
    stream: &str,
    landed_segment: &str,
    files: Vec<FileDescriptor>,
    outcome: &str,
) -> Result<(), ()> {
    let event = DeviceIngestEvent {
        record_type: "device_ingest".to_owned(),
        record_version: 1,
        outcome: outcome.to_owned(),
        protocol_version: 3,
        cid: cid.to_owned(),
        source: envelope.source.clone(),
        stream: stream.to_owned(),
        day: envelope.day.clone(),
        segment: landed_segment.to_owned(),
        files,
        meta: envelope.meta.clone(),
        extra: Map::new(),
    };
    append_durable_event(segment_path, &DurableEvent::DeviceIngest(event)).map_err(|_| ())
}

fn stamp_observer(
    state: &IngestState,
    cid: &str,
    day: &str,
    landed_segment: &str,
    status: solstone_core_ingest_resolve::PlanStatus,
) -> Result<Option<String>, ()> {
    let mut observer = match resolve_device_observer(&state.journal_root, cid) {
        Ok(None) => return Ok(None),
        Ok(Some(observer)) => observer,
        Err(_) => return Err(()),
    };
    let name = observer.record.name().map(str::to_owned);
    let refresh = match status {
        solstone_core_ingest_resolve::PlanStatus::Duplicate => {
            observer.record.last_segment() != Some(landed_segment)
        }
        solstone_core_ingest_resolve::PlanStatus::Ok
        | solstone_core_ingest_resolve::PlanStatus::Collision => true,
    };
    if !refresh {
        return Ok(name);
    }
    observer.record.set_last_segment(landed_segment.to_owned());
    observer.record.set_last_segment_day(day.to_owned());
    observer
        .record
        .set_last_segment_received_at((state.now_ms)());
    save_observer(&state.journal_root, &observer.record).map_err(|_| ())?;
    Ok(name)
}

fn written_descriptors(applied: &[AppliedFile]) -> Vec<FileDescriptor> {
    applied
        .iter()
        .filter(|file| file.disposition == AppliedDisposition::Written)
        .map(|file| {
            let mut extra = Map::new();
            extra.insert(
                "disposition".to_owned(),
                Value::String("written".to_owned()),
            );
            FileDescriptor {
                submitted: file.name.as_str().to_owned(),
                written: file.name.as_str().to_owned(),
                size: file.size,
                sha256: file.sha256.clone(),
                extra,
            }
        })
        .collect()
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
                Err(failure) if matches!(failure.error, ApplyError::Stale) && retry_stale => {
                    resolve_and_apply(state, day, stream, requested_segment, files, false)
                }
                Err(failure)
                    if failure
                        .applied
                        .iter()
                        .any(|file| file.disposition == AppliedDisposition::Written) =>
                {
                    Ok(ApplyPhase::Partial(PartialApply {
                        applied: failure.applied,
                        landed_segment: plan.landed_segment,
                        segment: plan.segment,
                    }))
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

enum IngestActivity {
    Accepted(AcceptedSegment),
    Rejected(ReasonCode),
}

struct IngestCompletion {
    response: Response,
    activity: IngestActivity,
}

impl IngestCompletion {
    fn accepted(day: String, name: String, response: Response) -> Self {
        Self {
            response,
            activity: IngestActivity::Accepted(AcceptedSegment { day, name }),
        }
    }

    fn rejected(code: ReasonCode, response: Response) -> Self {
        Self {
            response,
            activity: IngestActivity::Rejected(code),
        }
    }
}

fn refusal_with_activity(
    state: &IngestState,
    cid: &str,
    code: ReasonCode,
    status: StatusCode,
    detail: impl Into<String>,
) -> Response {
    finish_ingest_completion(
        state,
        cid,
        IngestCompletion::rejected(code, refusal(code, status, detail)),
    )
}

fn finish_ingest_completion(
    state: &IngestState,
    cid: &str,
    completion: IngestCompletion,
) -> Response {
    let IngestCompletion { response, activity } = completion;
    record_ingest_activity(state, cid, activity);
    response
}

fn record_ingest_activity(state: &IngestState, cid: &str, activity: IngestActivity) {
    let timestamp = activity_timestamp((state.now_ms)());
    let mut ledger = AuthorizationLedger::new(&state.journal_root);
    let (operation, result) = match activity {
        IngestActivity::Accepted(segment) => (
            "ingest_success",
            ledger.record_accepted_ingest(cid, &timestamp, segment),
        ),
        IngestActivity::Rejected(code) => (
            "ingest_rejection",
            ledger.record_ingest_rejection(cid, &timestamp, code.as_str()),
        ),
    };
    if result.is_err() {
        log::warn!("client_activity_write_failed operation={operation}");
    }
}

fn activity_timestamp(now_ms: i64) -> String {
    let timestamp = OffsetDateTime::from_unix_timestamp_nanos(i128::from(now_ms) * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    timestamp
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, Once, mpsc};
    use std::thread;
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Map, Value, json};
    use sha2::Digest;
    use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
    use solstone_core_convey_http::serve::{mux_builder, serve_connection};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    use super::{
        ApplyPhase, Envelope, IngestState, MAX_PART_BYTES, TestIngestNotifier, api_router,
        api_router_with, api_router_with_notifier, clear_before_apply_hook, resolve_and_apply,
        router, set_before_apply_hook, wall_clock_ms, write_envelope,
    };
    use crate::model::IncomingFile;
    use solstone_core_segment::hold_source_mutation;
    use solstone_core_sol_link::ledger::{
        AuthorizationLedger, ClientEntry, ClientRole, DeviceActivityRead, read_device_activity,
    };

    const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    struct SpyNotifier {
        calls: AtomicUsize,
        fail_next: AtomicBool,
        notices: Mutex<Vec<SpyNotice>>,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct SpyNotice {
        files: Vec<String>,
        observer: Option<String>,
    }

    impl SpyNotifier {
        fn succeeding() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                fail_next: AtomicBool::new(false),
                notices: Mutex::new(Vec::new()),
            })
        }

        fn fail_next() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                fail_next: AtomicBool::new(true),
                notices: Mutex::new(Vec::new()),
            })
        }
    }

    impl solstone_core_ingest_resolve::IngestNotifier for SpyNotifier {
        fn notify(
            &self,
            notice: &solstone_core_ingest_resolve::IngestNotice<'_>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.notices.lock().unwrap().push(SpyNotice {
                files: notice.files.to_vec(),
                observer: notice.observer.map(str::to_owned),
            });
            if self.fail_next.swap(false, Ordering::SeqCst) {
                Err(Box::new(std::io::Error::other("bus unavailable")))
            } else {
                Ok(())
            }
        }
    }

    struct CapturedLogger {
        messages: Mutex<Vec<String>>,
    }

    impl log::Log for CapturedLogger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() == log::Level::Warn
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                self.messages
                    .lock()
                    .unwrap()
                    .push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    static ACTIVITY_LOGGER: CapturedLogger = CapturedLogger {
        messages: Mutex::new(Vec::new()),
    };
    static ACTIVITY_LOGGER_INIT: Once = Once::new();

    fn root() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    fn seed_authorized_client(root: &Path, cid: &str) {
        AuthorizationLedger::new(root)
            .add(ClientEntry::new(
                cid,
                "Test device",
                "2026-01-01T00:00:00Z",
                "test-instance",
                ClientRole::Roleless,
            ))
            .unwrap();
    }

    fn activity_for(root: &Path, cid: &str) -> solstone_core_sol_link::ledger::ClientActivity {
        let DeviceActivityRead::Present(activity) =
            read_device_activity(&root.join("link/devices.json"))
        else {
            panic!("expected device activity for {cid}");
        };
        activity.get(cid).cloned().expect("activity for client")
    }

    fn clear_activity_logs() {
        ACTIVITY_LOGGER_INIT.call_once(|| {
            log::set_logger(&ACTIVITY_LOGGER).expect("install test logger");
            log::set_max_level(log::LevelFilter::Warn);
        });
        ACTIVITY_LOGGER.messages.lock().unwrap().clear();
    }

    fn basis(cid: &str) -> AccessBasis {
        AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(cid).unwrap(),
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
        call_upload_as(app, CID_A, envelope, name, bytes).await
    }

    async fn call_upload_as(
        app: &axum::Router,
        cid: &str,
        envelope: Value,
        name: &str,
        bytes: &[u8],
    ) -> (StatusCode, Value) {
        let (content_type, body) = multipart(envelope, name, bytes);
        call(
            app,
            "POST",
            "/app/devices/ingest",
            Some(content_type),
            body,
            basis(cid),
            Some("3"),
            &[],
        )
        .await
    }

    fn envelope(day: &str, segment: &str, files: Value) -> Value {
        json!({"day": day, "segment": segment, "files": files})
    }

    fn direct_state(root: &Path) -> IngestState {
        IngestState {
            journal_root: root.to_path_buf(),
            notifier: Arc::new(TestIngestNotifier),
            now_ms: Arc::new(|| 0),
        }
    }

    fn direct_envelope(source: &str, submitted: &str) -> Envelope {
        Envelope {
            day: "20260804".to_owned(),
            segment: "120000_1".to_owned(),
            source: source.to_owned(),
            meta: Map::new(),
            files: vec![IncomingFile {
                submitted: submitted.to_owned(),
                bytes: b"upload".to_vec(),
                descriptor_extra: Map::new(),
            }],
        }
    }

    fn stream_record_count(root: &Path) -> usize {
        fs::read_dir(root.join("streams"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(".json"))
            })
            .count()
    }

    fn seed_observer(root: &Path, prefix: &str, name: &str, cid: &str, stream: &str) {
        let directory = root.join("apps/observer/observers");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{prefix}.json")),
            json!({
                "key": format!("{prefix}-test-handle"),
                "name": name,
                "stream": stream,
                "created_at": 4,
                "revoked": false,
                "device_binding": {"device": cid, "kind": "cert"},
            })
            .to_string(),
        )
        .unwrap();
    }

    struct ObserverStamp<'a> {
        segment: &'a str,
        day: &'a str,
        received_at: i64,
    }

    fn seed_observer_stamped(
        root: &Path,
        prefix: &str,
        name: &str,
        cid: &str,
        stream: &str,
        stamp: ObserverStamp<'_>,
    ) {
        let directory = root.join("apps/observer/observers");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{prefix}.json")),
            json!({
                "key": format!("{prefix}-test-handle"),
                "name": name,
                "stream": stream,
                "created_at": 4,
                "revoked": false,
                "device_binding": {"device": cid, "kind": "cert"},
                "last_segment": stamp.segment,
                "last_segment_day": stamp.day,
                "last_segment_received_at": stamp.received_at,
            })
            .to_string(),
        )
        .unwrap();
    }

    fn seed_unattributed_stream(
        root: &Path,
        name: &str,
        created_at: u64,
        last_day: &str,
        last_segment: &str,
        seq: u64,
    ) {
        let directory = root.join("streams");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{name}.json")),
            json!({
                "name": name,
                "kind": "observer",
                "host": null,
                "platform": null,
                "created_at": created_at,
                "last_day": last_day,
                "last_segment": last_segment,
                "seq": seq,
            })
            .to_string(),
        )
        .unwrap();
    }

    fn seed_attributed_stream(
        root: &Path,
        name: &str,
        cid: &str,
        created_at: u64,
        last_day: &str,
        last_segment: &str,
        seq: u64,
    ) {
        let directory = root.join("streams");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{name}.json")),
            json!({
                "name": name,
                "kind": "observer",
                "host": null,
                "platform": null,
                "created_at": created_at,
                "last_day": last_day,
                "last_segment": last_segment,
                "seq": seq,
                "cid": cid,
                "source": "",
            })
            .to_string(),
        )
        .unwrap();
    }

    fn stream_record(root: &Path, name: &str) -> Value {
        serde_json::from_str(
            &fs::read_to_string(root.join("streams").join(format!("{name}.json"))).unwrap(),
        )
        .unwrap()
    }

    fn observer_record(root: &Path, prefix: &str) -> Value {
        serde_json::from_str(
            &fs::read_to_string(
                root.join("apps/observer/observers")
                    .join(format!("{prefix}.json")),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn stream_marker_path(root: &Path, day: &str) -> std::path::PathBuf {
        root.join("chronicle")
            .join(day)
            .join("health/stream.updated")
    }

    fn stream_marker_generation(root: &Path, day: &str) -> u64 {
        serde_json::from_slice::<Value>(&fs::read(stream_marker_path(root, day)).unwrap()).unwrap()
            ["generation"]
            .as_u64()
            .expect("stream marker generation")
    }

    async fn wait_for_callosum_client(server: &CallosumSocketServer) {
        for _ in 0..50 {
            if server.client_count() >= 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("callosum peer did not connect");
    }

    async fn next_observing(
        peer: &mut CallosumSocketConnection,
    ) -> solstone_core_callosum::CallosumEnvelope {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let message = peer.next_message().await.expect("callosum peer");
                if message.tract == "observe" && message.event == "observing" {
                    break message;
                }
            }
        })
        .await
        .expect("observe.observing event")
    }

    fn frozen_clock(start: i64) -> (Arc<AtomicI64>, Arc<dyn Fn() -> i64 + Send + Sync>) {
        let now = Arc::new(AtomicI64::new(start));
        let clock_now = now.clone();
        (now, Arc::new(move || clock_now.load(Ordering::SeqCst)))
    }

    async fn call_upload_files(
        app: &axum::Router,
        envelope: Value,
        files: &[(&str, &[u8])],
    ) -> (StatusCode, Value) {
        let envelope = envelope.to_string();
        let mut parts: Vec<(&str, Option<&str>, &[u8], usize)> =
            vec![("envelope", None, envelope.as_bytes(), 0)];
        parts.extend(
            files
                .iter()
                .map(|(name, bytes)| ("files", Some(*name), *bytes, 1)),
        );
        let (content_type, body) = multipart_parts(&parts);
        call(
            app,
            "POST",
            "/app/devices/ingest",
            Some(content_type),
            body,
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await
    }

    #[test]
    fn resolve_and_apply_reenters_once_after_stale_drift() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let state = IngestState {
            journal_root: root.clone(),
            notifier: SpyNotifier::succeeding(),
            now_ms: wall_clock_ms(),
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
    }

    #[test]
    fn resolve_and_apply_does_not_retry_a_second_stale_plan() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let state = IngestState {
            journal_root: root.clone(),
            notifier: SpyNotifier::succeeding(),
            now_ms: wall_clock_ms(),
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
    }

    #[test]
    fn location_writes_serialize_before_the_second_stream_bind() {
        let directory = root();
        let journal = directory.path().to_path_buf();
        let state = direct_state(&journal);
        let (first_at_apply_sender, first_at_apply) = mpsc::channel();
        let (release_first_sender, release_first) = mpsc::channel();
        let first_state = state.clone();
        let first = thread::spawn(move || {
            set_before_apply_hook(move |_| {
                first_at_apply_sender.send(()).unwrap();
                release_first.recv().unwrap();
            });
            write_envelope(
                &first_state,
                CID_A,
                direct_envelope("location", "first.json"),
            )
            .status()
        });
        first_at_apply
            .recv_timeout(Duration::from_secs(1))
            .expect("first location ingest reached apply while holding its source lock");

        let (second_started_sender, second_started) = mpsc::channel();
        let (second_at_apply_sender, second_at_apply) = mpsc::channel();
        let second_state = state.clone();
        let second = thread::spawn(move || {
            set_before_apply_hook(move |_| second_at_apply_sender.send(()).unwrap());
            second_started_sender.send(()).unwrap();
            write_envelope(
                &second_state,
                CID_B,
                direct_envelope("location", "second.json"),
            )
            .status()
        });
        second_started
            .recv_timeout(Duration::from_secs(1))
            .expect("second location ingest started");

        assert!(matches!(
            second_at_apply.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(
            stream_record_count(&journal),
            1,
            "the second ingest must not bind a stream while the first holds the source lock"
        );

        release_first_sender.send(()).unwrap();
        assert_eq!(first.join().unwrap(), StatusCode::OK);
        second_at_apply
            .recv_timeout(Duration::from_secs(1))
            .expect("second location ingest reaches apply after the first releases the lock");
        assert_eq!(second.join().unwrap(), StatusCode::OK);
    }

    #[test]
    fn non_location_write_bypasses_a_held_location_lock() {
        let directory = root();
        let journal = directory.path().to_path_buf();
        let _location_lock = hold_source_mutation(&journal, "location").unwrap();

        let response = write_envelope(
            &direct_state(&journal),
            CID_A,
            direct_envelope("audio", "audio.flac"),
        );

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(stream_record_count(&journal), 1);
    }

    #[tokio::test]
    async fn accepted_ingest_records_the_landed_segment_and_clears_rejection() {
        let directory = root();
        let journal = directory.path().to_path_buf();
        seed_authorized_client(&journal, CID_A);
        AuthorizationLedger::new(&journal)
            .record_ingest_rejection(CID_A, "2026-01-02T00:00:00Z", "event_append_failed")
            .unwrap();
        let (_now, clock) = frozen_clock(1_700_000_000_000);
        let app = api_router_with(&journal, Arc::new(TestIngestNotifier), clock);
        let request = json!({
            "day": "20260804",
            "segment": "120000_1",
            "source": "audio",
            "files": [{"submitted": "audio.flac"}],
        });

        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;

        assert_eq!(status, StatusCode::OK);
        let activity = activity_for(&journal, CID_A);
        assert!(activity.last_accepted_ingest_at.is_some());
        assert_eq!(
            activity.last_accepted_segment.as_ref().unwrap().day,
            "20260804"
        );
        assert_eq!(
            activity.last_accepted_segment.as_ref().unwrap().name,
            body["segment"].as_str().unwrap()
        );
        assert_eq!(activity.ingest_rejection, None);
    }

    #[tokio::test]
    async fn protocol_refusal_records_the_existing_reason_code() {
        let directory = root();
        let journal = directory.path().to_path_buf();
        seed_authorized_client(&journal, CID_A);
        let app = router(&journal);

        let (status, body) = call(
            &app,
            "POST",
            "/app/devices/ingest",
            None,
            Vec::new(),
            basis(CID_A),
            None,
            &[],
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "protocol_version_required");
        assert_eq!(
            activity_for(&journal, CID_A)
                .ingest_rejection
                .as_ref()
                .unwrap()
                .reason_code,
            "protocol_version_required"
        );
    }

    #[tokio::test]
    async fn rejection_for_an_unpaired_cid_leaves_the_refusal_unchanged() {
        let directory = root();
        let journal = directory.path().to_path_buf();
        let app = router(&journal);

        let (status, body) = call(
            &app,
            "POST",
            "/app/devices/ingest",
            None,
            Vec::new(),
            basis(CID_A),
            None,
            &[],
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["reason_code"], "protocol_version_required");
        assert!(!journal.join("link/devices.json").exists());
    }

    #[test]
    fn activity_write_failure_keeps_the_accepted_ingest_outcome() {
        let directory = root();
        let journal = directory.path().to_path_buf();
        seed_authorized_client(&journal, CID_A);
        fs::create_dir(journal.join("link/devices.json")).unwrap();
        clear_activity_logs();

        let response = write_envelope(
            &direct_state(&journal),
            CID_A,
            direct_envelope("audio", "audio.flac"),
        );

        assert_eq!(response.status(), StatusCode::OK);
        assert!(journal.join("chronicle").exists());
        assert!(
            ACTIVITY_LOGGER
                .messages
                .lock()
                .unwrap()
                .iter()
                .any(|message| message == "client_activity_write_failed operation=ingest_success")
        );
    }

    #[tokio::test]
    async fn location_lock_failure_is_retryable_before_any_durable_ingest_mutation() {
        let directory = root();
        let journal = directory.path().to_path_buf();
        seed_authorized_client(&journal, CID_A);
        fs::write(journal.join("streams"), b"not a directory").unwrap();
        let app = router(&journal);
        let request = json!({
            "day": "20260804",
            "segment": "120000_1",
            "source": "location",
            "files": [{"submitted": "location.json"}],
        });

        let (status, body) = call_upload(&app, request, "location.json", b"location").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "retryable");
        assert_eq!(body["reason_code"], "location_lock_unavailable");
        assert!(!journal.join("streams/device.json").exists());
        assert!(!journal.join("chronicle").exists());
        assert_eq!(
            activity_for(&journal, CID_A)
                .ingest_rejection
                .as_ref()
                .unwrap()
                .reason_code,
            "location_lock_unavailable"
        );
    }

    #[tokio::test]
    async fn identity_is_from_access_basis_not_envelope_meta() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let envelope = json!({"day":"20260804","segment":"120000_1","meta":{"did":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"files":[{"submitted":"audio.flac","forward":"kept"}]});
        let app = router(&root);
        let (status, body) = call_upload(&app, envelope, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        let events =
            fs::read_to_string(root.join("chronicle/20260804/device/120000_1/events.jsonl"))
                .unwrap();
        let event: Value = serde_json::from_str(&events).unwrap();
        assert_eq!(event["cid"], CID_A);
        assert_eq!(event["meta"]["did"], CID_B);
    }

    #[tokio::test]
    async fn duplicate_records_event_without_second_stream_advance() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
            "/app/devices/ingest/manifest",
            None,
            Vec::new(),
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(manifest["days"]["20260804"]["segments"], 1);
        let (_, day) = call(
            &app,
            "GET",
            "/app/devices/ingest/manifest/20260804",
            None,
            Vec::new(),
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(
            day["segments"]["120000_1"]["files"][0]["sha256"],
            format!("{:x}", sha2::Sha256::digest(b"sound"))
        );
        assert_eq!(day["segments"]["120000_1"]["files"][0]["status"], "present");
        let (_, segments) = call(
            &app,
            "GET",
            "/app/devices/ingest/segments/20260804",
            None,
            Vec::new(),
            basis(CID_A),
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
    }

    #[tokio::test]
    async fn stream_advance_follows_segment_creation_not_plan_status() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let app = router(&root);
        let stream = root.join("streams/device.json");
        let state = || -> Value {
            serde_json::from_str(&fs::read_to_string(&stream).expect("stream record"))
                .expect("stream json")
        };

        // Ok mint advances.
        assert_eq!(
            call_upload(
                &app,
                envelope("20260804", "120000_61", json!([{"submitted":"audio.flac"}])),
                "audio.flac",
                b"first",
            )
            .await
            .1["status"],
            "ok"
        );
        assert_eq!(state()["seq"], 1);
        assert_eq!(state()["last_segment"], "120000_61");
        // Ok heal does not advance.
        assert_eq!(
            call_upload(
                &app,
                envelope("20260804", "120000_61", json!([{"submitted":"notes.json"}])),
                "notes.json",
                b"notes",
            )
            .await
            .1["status"],
            "ok"
        );
        assert_eq!(state()["seq"], 1);
        assert_eq!(state()["last_segment"], "120000_61");
        // Collision heal finds the existing 120000_61 candidate for requested
        // 120000_60, writes its new file there, and still does not advance.
        assert_eq!(
            call_upload(
                &app,
                envelope("20260804", "120000_60", json!([{"submitted":"other.json"}])),
                "other.json",
                b"other",
            )
            .await
            .1["status"],
            "collision"
        );
        assert_eq!(state()["seq"], 1);
        assert_eq!(state()["last_segment"], "120000_61");
        // Collision mint advances because it creates 120000_62 after the
        // content conflict at 120000_61.
        assert_eq!(
            call_upload(
                &app,
                envelope("20260804", "120000_61", json!([{"submitted":"audio.flac"}])),
                "audio.flac",
                b"second",
            )
            .await
            .1["status"],
            "collision"
        );
        assert_eq!(state()["seq"], 2);
        assert_eq!(state()["last_segment"], "120000_62");
        // Duplicate never advances.
        assert_eq!(
            call_upload(
                &app,
                envelope("20260804", "120000_61", json!([{"submitted":"audio.flac"}])),
                "audio.flac",
                b"first",
            )
            .await
            .1["status"],
            "duplicate"
        );
        assert_eq!(state()["seq"], 2);
        assert_eq!(state()["last_segment"], "120000_62");
    }

    const FROZEN_NOW_MS: i64 = 1_700_000_000_000;

    #[tokio::test]
    async fn notification_failure_is_5xx_and_leaves_durable_writes() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "aaaaaaaa", "Desk", CID_A, "desk");
        let spy = SpyNotifier::fail_next();
        let (now, clock) = frozen_clock(FROZEN_NOW_MS);
        let app = api_router_with(&root, spy.clone(), clock);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request.clone(), "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "notify_failed");
        assert_eq!(spy.calls.load(Ordering::SeqCst), 1);
        assert!(
            root.join("chronicle/20260804/desk/120000_1/events.jsonl")
                .exists()
        );
        let first = observer_record(&root, "aaaaaaaa");
        let stamp = first["last_segment_received_at"].as_i64().expect("stamp");
        assert!(stamp > 1_000_000_000_000);
        assert!((stamp - FROZEN_NOW_MS).abs() < 60_000);
        assert_eq!(first["last_segment_day"], "20260804");
        assert_eq!(first["last_segment"], "120000_1");
        assert_eq!(stream_record(&root, "desk")["seq"], 1);
        assert_eq!(stream_marker_generation(&root, "20260804"), 1);

        now.store(FROZEN_NOW_MS + 60_000, Ordering::SeqCst);
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "duplicate");
        // All files are already held on retry, so it is not re-announced.
        assert_eq!(spy.calls.load(Ordering::SeqCst), 1);
        assert_eq!(stream_marker_generation(&root, "20260804"), 1);
        let retry = observer_record(&root, "aaaaaaaa");
        assert_eq!(retry["last_segment_received_at"], stamp);
        assert_eq!(stream_record(&root, "desk")["seq"], 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stamp_failure_is_5xx_then_duplicate_fills_absent_fields() {
        use std::os::unix::fs::PermissionsExt;

        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "aaaaaaaa", "Desk", CID_A, "desk");
        let spy = SpyNotifier::succeeding();
        let (now, clock) = frozen_clock(FROZEN_NOW_MS);
        let app = api_router_with(&root, spy.clone(), clock);
        let observers = root.join("apps/observer/observers");
        fs::set_permissions(&observers, fs::Permissions::from_mode(0o555)).unwrap();
        if save_probe_succeeds(&observers) {
            fs::set_permissions(&observers, fs::Permissions::from_mode(0o755)).unwrap();
            panic!("requires a non-root runner");
        }
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request.clone(), "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "observer_stamp_failed");
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
        assert_eq!(stream_record(&root, "desk")["seq"], 1);
        assert_eq!(stream_marker_generation(&root, "20260804"), 1);
        let first = observer_record(&root, "aaaaaaaa");
        assert!(
            first
                .get("last_segment_received_at")
                .is_none_or(Value::is_null)
        );
        assert!(first.get("last_segment_day").is_none_or(Value::is_null));
        assert!(first.get("last_segment").is_none_or(Value::is_null));

        fs::set_permissions(&observers, fs::Permissions::from_mode(0o755)).unwrap();
        now.store(FROZEN_NOW_MS + 60_000, Ordering::SeqCst);
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "duplicate");
        // The recovering duplicate has no newly written content to announce.
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
        assert_eq!(stream_marker_generation(&root, "20260804"), 1);
        assert_eq!(stream_record(&root, "desk")["seq"], 1);
        let retry = observer_record(&root, "aaaaaaaa");
        assert_eq!(retry["last_segment_received_at"], FROZEN_NOW_MS + 60_000);
        assert_eq!(retry["last_segment_day"], "20260804");
        assert_eq!(retry["last_segment"], "120000_1");
    }

    #[tokio::test]
    async fn second_mint_refreshes_already_present_stamp() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer_stamped(
            &root,
            "aaaaaaaa",
            "Desk",
            CID_A,
            "desk",
            ObserverStamp {
                segment: "090000_1",
                day: "20260803",
                received_at: FROZEN_NOW_MS - 3_600_000,
            },
        );
        let spy = SpyNotifier::succeeding();
        let (_now, clock) = frozen_clock(FROZEN_NOW_MS);
        let app = api_router_with(&root, spy, clock);
        let (status, body) = call_upload(
            &app,
            envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}])),
            "audio.flac",
            b"sound",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        let record = observer_record(&root, "aaaaaaaa");
        let stamp = record["last_segment_received_at"].as_i64().expect("stamp");
        assert!(stamp > 1_000_000_000_000);
        assert!((stamp - FROZEN_NOW_MS).abs() < 60_000);
        assert_eq!(record["last_segment_day"], "20260804");
        assert_eq!(record["last_segment"], "120000_1");
    }

    #[tokio::test]
    async fn heal_refreshes_stamp_without_changing_landed_segment() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "aaaaaaaa", "Desk", CID_A, "desk");
        let spy = SpyNotifier::succeeding();
        let (now, clock) = frozen_clock(FROZEN_NOW_MS);
        let app = api_router_with(&root, spy, clock);
        let (status, body) = call_upload(
            &app,
            envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}])),
            "audio.flac",
            b"sound",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        let first = observer_record(&root, "aaaaaaaa");
        let first_stamp = first["last_segment_received_at"].as_i64().expect("stamp");
        assert!((first_stamp - FROZEN_NOW_MS).abs() < 60_000);
        assert_eq!(first["last_segment_day"], "20260804");
        assert_eq!(first["last_segment"], "120000_1");

        now.store(FROZEN_NOW_MS + 3_600_000, Ordering::SeqCst);
        let (status, body) = call_upload(
            &app,
            envelope("20260804", "120000_1", json!([{"submitted":"notes.json"}])),
            "notes.json",
            b"notes",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        let record = observer_record(&root, "aaaaaaaa");
        let stamp = record["last_segment_received_at"].as_i64().expect("stamp");
        assert!((stamp - (FROZEN_NOW_MS + 3_600_000)).abs() < 60_000);
        assert_eq!(record["last_segment_day"], "20260804");
        assert_eq!(record["last_segment"], "120000_1");
    }

    #[tokio::test]
    async fn duplicate_of_already_stamped_segment_does_not_refresh() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "aaaaaaaa", "Desk", CID_A, "desk");
        let spy = SpyNotifier::succeeding();
        let (now, clock) = frozen_clock(FROZEN_NOW_MS);
        let app = api_router_with(&root, spy, clock);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request.clone(), "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        let first = observer_record(&root, "aaaaaaaa");
        let stamp = first["last_segment_received_at"].as_i64().expect("stamp");

        now.store(FROZEN_NOW_MS + 3_600_000, Ordering::SeqCst);
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "duplicate");
        let retry = observer_record(&root, "aaaaaaaa");
        assert_eq!(retry["last_segment_received_at"], stamp);
        assert_eq!(retry["last_segment"], "120000_1");
        assert_eq!(retry["last_segment_day"], "20260804");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn duplicate_recovers_stale_stamp_then_stops_refreshing() {
        use std::os::unix::fs::PermissionsExt;

        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer_stamped(
            &root,
            "aaaaaaaa",
            "Desk",
            CID_A,
            "desk",
            ObserverStamp {
                segment: "090000_1",
                day: "20260803",
                received_at: FROZEN_NOW_MS - 3_600_000,
            },
        );
        let spy = SpyNotifier::succeeding();
        let (now, clock) = frozen_clock(FROZEN_NOW_MS);
        let app = api_router_with(&root, spy, clock);
        let observers = root.join("apps/observer/observers");
        fs::set_permissions(&observers, fs::Permissions::from_mode(0o555)).unwrap();
        if save_probe_succeeds(&observers) {
            fs::set_permissions(&observers, fs::Permissions::from_mode(0o755)).unwrap();
            panic!("requires a non-root runner");
        }
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request.clone(), "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "observer_stamp_failed");
        let first = observer_record(&root, "aaaaaaaa");
        assert_eq!(first["last_segment"], "090000_1");
        assert_eq!(first["last_segment_day"], "20260803");
        assert_eq!(first["last_segment_received_at"], FROZEN_NOW_MS - 3_600_000);

        fs::set_permissions(&observers, fs::Permissions::from_mode(0o755)).unwrap();
        now.store(FROZEN_NOW_MS + 60_000, Ordering::SeqCst);
        let (status, body) = call_upload(&app, request.clone(), "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "duplicate");
        let recovered = observer_record(&root, "aaaaaaaa");
        let recovered_stamp = recovered["last_segment_received_at"]
            .as_i64()
            .expect("recovered stamp");
        assert!((recovered_stamp - (FROZEN_NOW_MS + 60_000)).abs() < 60_000);
        assert_eq!(recovered["last_segment_day"], "20260804");
        assert_eq!(recovered["last_segment"], "120000_1");

        now.store(FROZEN_NOW_MS + 120_000, Ordering::SeqCst);
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "duplicate");
        let retry = observer_record(&root, "aaaaaaaa");
        assert_eq!(retry["last_segment_received_at"], recovered_stamp);
    }

    #[cfg(unix)]
    fn save_probe_succeeds(observers: &Path) -> bool {
        fs::write(observers.join(".stamp-probe"), b"x").is_ok()
    }

    #[tokio::test]
    async fn partial_apply_appends_event_for_written_file_only() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let spy = SpyNotifier::succeeding();
        let app = api_router_with_notifier(&root, spy.clone());
        set_before_apply_hook(|plan| {
            fs::create_dir_all(plan.segment.path().join("notes.json")).unwrap();
        });
        let request = envelope(
            "20260804",
            "120000_1",
            json!([{"submitted":"audio.flac"},{"submitted":"notes.json"}]),
        );
        let (status, body) = call_upload_files(
            &app,
            request,
            &[
                ("audio.flac", b"sound".as_slice()),
                ("notes.json", b"notes"),
            ],
        )
        .await;
        clear_before_apply_hook();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "journal_write_failed");
        // The written prefix is announced despite the later partial failure.
        assert_eq!(spy.calls.load(Ordering::SeqCst), 1);
        assert_eq!(stream_marker_generation(&root, "20260804"), 1);
        assert_eq!(
            spy.notices.lock().unwrap().as_slice(),
            [SpyNotice {
                files: vec!["audio.flac".to_owned()],
                observer: None,
            }]
        );
        let events =
            fs::read_to_string(root.join("chronicle/20260804/device/120000_1/events.jsonl"))
                .unwrap();
        let event: Value = serde_json::from_str(events.lines().next().unwrap()).unwrap();
        assert_eq!(event["record_type"], "device_ingest");
        let names: Vec<&str> = event["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["written"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["audio.flac"]);
        let (_, listing) = call(
            &app,
            "GET",
            "/app/devices/ingest/segments/20260804",
            None,
            Vec::new(),
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await;
        let item = listing["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["key"] == "120000_1")
            .expect("landed segment");
        let listed: Vec<&str> = item["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["name"].as_str().unwrap())
            .collect();
        assert!(listed.contains(&"audio.flac"));
        assert!(!listed.contains(&"notes.json"));
        let audio = item["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["name"] == "audio.flac")
            .unwrap();
        assert_eq!(audio["status"], "present");
    }

    #[tokio::test]
    async fn partial_notify_failure_keeps_journal_write_failed() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let spy = SpyNotifier::fail_next();
        let app = api_router_with_notifier(&root, spy.clone());
        set_before_apply_hook(|plan| {
            fs::create_dir_all(plan.segment.path().join("notes.json")).unwrap();
        });
        let request = envelope(
            "20260804",
            "120000_1",
            json!([{"submitted":"audio.flac"},{"submitted":"notes.json"}]),
        );
        let (status, body) = call_upload_files(
            &app,
            request,
            &[
                ("audio.flac", b"sound".as_slice()),
                ("notes.json", b"notes"),
            ],
        )
        .await;
        clear_before_apply_hook();

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "journal_write_failed");
        assert_eq!(spy.calls.load(Ordering::SeqCst), 1);
        assert_eq!(stream_marker_generation(&root, "20260804"), 1);
        let events =
            fs::read_to_string(root.join("chronicle/20260804/device/120000_1/events.jsonl"))
                .unwrap();
        let event: Value = serde_json::from_str(events.lines().next().unwrap()).unwrap();
        let names: Vec<&str> = event["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["written"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["audio.flac"]);
    }

    #[tokio::test]
    async fn partial_append_failure_leaves_day_dirty_without_notification() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let segment = root.join("chronicle/20260804/device/120000_1");
        fs::create_dir_all(segment.join("events.jsonl")).unwrap();
        let spy = SpyNotifier::succeeding();
        let app = api_router_with_notifier(&root, spy.clone());
        set_before_apply_hook(|plan| {
            fs::create_dir_all(plan.segment.path().join("notes.json")).unwrap();
        });
        let request = envelope(
            "20260804",
            "120000_1",
            json!([{"submitted":"audio.flac"},{"submitted":"notes.json"}]),
        );
        let (status, body) = call_upload_files(
            &app,
            request,
            &[
                ("audio.flac", b"sound".as_slice()),
                ("notes.json", b"notes"),
            ],
        )
        .await;
        clear_before_apply_hook();

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "event_append_failed");
        assert_eq!(stream_marker_generation(&root, "20260804"), 1);
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn partial_notice_keeps_reserved_meta_keys_nested() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "aaaaaaaa", "Desk", CID_A, "desk");
        let socket = root.join("health/callosum.sock");
        let server = CallosumSocketServer::bind(&socket).await.unwrap();
        let mut peer = CallosumSocketConnection::new(&socket, Map::new());
        peer.start();
        wait_for_callosum_client(&server).await;
        let app = api_router(&root);
        set_before_apply_hook(|plan| {
            fs::create_dir_all(plan.segment.path().join("notes.json")).unwrap();
        });
        let meta = json!({
            "tract": "caller-tract",
            "event": "caller-event",
            "day": "caller-day",
            "segment": "caller-segment",
            "stream": "caller-stream",
            "batch": "caller-batch",
            "observer": "caller-observer",
            "files": ["caller-file"],
        });
        let request = json!({
            "day": "20260804",
            "segment": "120000_1",
            "meta": meta.clone(),
            "files": [{"submitted":"audio.flac"},{"submitted":"notes.json"}],
        });
        let (status, body) = call_upload_files(
            &app,
            request,
            &[
                ("audio.flac", b"sound".as_slice()),
                ("notes.json", b"notes"),
            ],
        )
        .await;
        clear_before_apply_hook();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "journal_write_failed");

        let notice = next_observing(&mut peer).await;
        assert_eq!(notice.tract, "observe");
        assert_eq!(notice.event, "observing");
        assert_eq!(notice.extra["day"], "20260804");
        assert_eq!(notice.extra["segment"], "120000_1");
        assert_eq!(notice.extra["stream"], "desk");
        assert_eq!(notice.extra["files"], json!(["audio.flac"]));
        assert_eq!(notice.extra["batch"], false);
        assert_eq!(notice.extra["observer"], "Desk");
        assert_eq!(notice.extra["meta"]["event"], "caller-event");
        assert_eq!(notice.extra["meta"]["batch"], "caller-batch");
        assert_eq!(notice.extra["meta"], meta);
        peer.stop().await;
        server.stop().await;
    }

    #[tokio::test]
    async fn no_observer_skips_stamp_and_still_notifies() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let spy = SpyNotifier::succeeding();
        let app = api_router_with_notifier(&root, spy.clone());
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(spy.calls.load(Ordering::SeqCst), 1);
        assert!(!root.join("apps/observer/observers").exists());
    }

    #[tokio::test]
    async fn production_router_sends_observing_to_callosum() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let socket = root.join("health/callosum.sock");
        let server = CallosumSocketServer::bind(&socket).await.unwrap();
        let mut peer = CallosumSocketConnection::new(&socket, Map::new());
        peer.start();
        wait_for_callosum_client(&server).await;
        seed_observer(&root, "aaaaaaaa", "Desk", CID_A, "desk");
        let app = api_router(&root);
        let request = json!({
            "day": "20260804",
            "segment": "120000_1",
            "source": "mobile",
            "meta": {"kind": "v3"},
            "files": [{"submitted":"audio.flac"}],
        });
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");

        let notice = next_observing(&mut peer).await;
        assert_eq!(notice.tract, "observe");
        assert_eq!(notice.event, "observing");
        assert_eq!(notice.extra["day"], "20260804");
        assert_eq!(notice.extra["stream"], "desk");
        assert_eq!(notice.extra["segment"], "120000_1");
        assert_eq!(notice.extra["files"], json!(["audio.flac"]));
        assert_eq!(notice.extra["batch"], false);
        assert_eq!(notice.extra["observer"], "Desk");
        assert_eq!(notice.extra["meta"], json!({"kind": "v3"}));
        peer.stop().await;
        server.stop().await;
    }

    #[tokio::test]
    async fn mixed_written_and_already_held_files_bump_once_and_announce_only_written() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let spy = SpyNotifier::succeeding();
        let app = api_router_with_notifier(&root, spy.clone());
        let first = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, first, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(stream_marker_generation(&root, "20260804"), 1);

        let second = envelope(
            "20260804",
            "120000_1",
            json!([{"submitted":"audio.flac"},{"submitted":"notes.json"}]),
        );
        let (status, body) = call_upload_files(
            &app,
            second,
            &[("audio.flac", b"sound"), ("notes.json", b"notes")],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(stream_marker_generation(&root, "20260804"), 2);
        assert_eq!(spy.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            spy.notices.lock().unwrap().last().unwrap(),
            &SpyNotice {
                files: vec!["notes.json".to_owned()],
                observer: None,
            }
        );
    }

    #[tokio::test]
    async fn exhausted_resolution_quarantines_without_history_or_notification() {
        let dir = root();
        let root = dir.path().to_path_buf();
        for offset in 0..solstone_core_ingest_resolve::MAX_INGEST_SEGMENT_ATTEMPTS {
            let directory = root
                .join("chronicle/20260804/device")
                .join(format!("120000_{}", 1 + offset));
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("audio.flac"), b"old").unwrap();
        }
        let spy = SpyNotifier::succeeding();
        let app = api_router_with_notifier(&root, spy.clone());
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
        assert!(!stream_marker_path(&root, "20260804").exists());
        assert!(
            fs::read_dir(root.join("chronicle/20260804/device"))
                .unwrap()
                .all(|entry| !entry.unwrap().path().join("events.jsonl").exists())
        );
    }

    #[tokio::test]
    async fn append_failure_is_a_hard_error_and_never_notifies() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let segment = root.join("chronicle/20260804/device/120000_1");
        fs::create_dir_all(segment.join("events.jsonl")).unwrap();
        let spy = SpyNotifier::succeeding();
        let app = api_router_with_notifier(&root, spy.clone());
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "event_append_failed");
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
        assert_eq!(stream_marker_generation(&root, "20260804"), 1);
    }

    #[tokio::test]
    async fn malformed_stream_marker_fails_written_request_but_held_retry_does_not_repair_it() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let marker = stream_marker_path(&root, "20260804");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"not-json").unwrap();
        let before = fs::read(&marker).unwrap();
        let spy = SpyNotifier::succeeding();
        let app = api_router_with_notifier(&root, spy.clone());
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));

        let (status, body) = call_upload(&app, request.clone(), "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "stream_marker_bump_failed");
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
        assert!(
            !root
                .join("chronicle/20260804/device/120000_1/events.jsonl")
                .exists()
        );
        assert_eq!(fs::read(&marker).unwrap(), before);

        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "duplicate");
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read(&marker).unwrap(), before);
    }

    #[tokio::test]
    async fn malformed_stream_marker_refuses_partial_custody_and_notification() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let marker = stream_marker_path(&root, "20260804");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"not-json").unwrap();
        let before = fs::read(&marker).unwrap();
        let spy = SpyNotifier::succeeding();
        let app = api_router_with_notifier(&root, spy.clone());
        set_before_apply_hook(|plan| {
            fs::create_dir_all(plan.segment.path().join("notes.json")).unwrap();
        });
        let request = envelope(
            "20260804",
            "120000_1",
            json!([{"submitted":"audio.flac"},{"submitted":"notes.json"}]),
        );

        let (status, body) = call_upload_files(
            &app,
            request,
            &[
                ("audio.flac", b"sound".as_slice()),
                ("notes.json", b"notes"),
            ],
        )
        .await;
        clear_before_apply_hook();

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "stream_marker_bump_failed");
        assert_eq!(fs::read(&marker).unwrap(), before);
        assert_eq!(spy.calls.load(Ordering::SeqCst), 0);
        let segment = root.join("chronicle/20260804/device/120000_1");
        assert_eq!(fs::read(segment.join("audio.flac")).unwrap(), b"sound");
        assert!(!segment.join("events.jsonl").exists());
    }

    #[tokio::test]
    async fn resolution_io_failure_has_no_history_or_manifest() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
    }

    #[tokio::test]
    async fn legacy_key_route_is_not_registered() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let mut request = Request::builder()
            .uri("/app/observer/api/deadbeef/key")
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(basis(CID_A));
        let response = router(&root).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let production = include_str!("router.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!production.contains("/api/"));
        assert!(!production.contains("/key"));
    }

    #[tokio::test]
    async fn observer_ingest_prefix_is_not_aliased_after_the_devices_rename() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let app = router(&root);
        for path in [
            "/app/observer/ingest",
            "/app/observer/ingest/manifest",
            "/app/observer/ingest/manifest/20260804",
            "/app/observer/ingest/segments/20260804",
        ] {
            let method = if path == "/app/observer/ingest" {
                "POST"
            } else {
                "GET"
            };
            let (status, _) = call(
                &app,
                method,
                path,
                None,
                Vec::new(),
                basis(CID_A),
                Some("3"),
                &[],
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        }
    }

    #[tokio::test]
    async fn oversized_multipart_part_has_its_own_refusal() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
                "/app/devices/ingest",
                Some(content_type),
                body,
                basis(CID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_part_too_large"
        );
    }

    #[tokio::test]
    async fn excess_multipart_files_have_their_own_refusal() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
                "/app/devices/ingest",
                Some(content_type),
                body,
                basis(CID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_too_many_files"
        );
    }

    #[tokio::test]
    async fn excess_multipart_parts_have_their_own_refusal() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
                "/app/devices/ingest",
                Some(content_type),
                body,
                basis(CID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_too_many_parts"
        );
    }

    #[tokio::test]
    async fn oversized_filename_has_its_own_refusal() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
                "/app/devices/ingest",
                Some(content_type),
                body,
                basis(CID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_filename_too_long"
        );
    }

    #[tokio::test]
    async fn excess_multipart_headers_have_their_own_refusal() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
                "/app/devices/ingest",
                Some(content_type),
                body,
                basis(CID_A),
                Some("3"),
                &[]
            )
            .await
            .1["reason_code"],
            "multipart_too_many_headers"
        );
    }

    #[tokio::test]
    async fn collision_preserves_original_content_and_remaps_segment() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
            "/app/devices/ingest/manifest/20260804",
            None,
            Vec::new(),
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(day["segments"][remapped]["files"][0]["name"], "audio.flac");
        let (_, segments) = call(
            &app,
            "GET",
            "/app/devices/ingest/segments/20260804",
            None,
            Vec::new(),
            basis(CID_A),
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
    }

    #[tokio::test]
    async fn sidecar_conflict_is_distinct_from_media_collision() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
                "/app/devices/ingest",
                Some(content_type),
                body,
                basis(CID_A),
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
            "/app/devices/ingest",
            Some(content_type),
            body,
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["status"], "conflict");
        assert_eq!(body["reason_code"], "content_conflict");
    }

    #[tokio::test]
    async fn localhost_and_bearer_headers_do_not_supply_identity() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (content_type, body) = multipart(request.clone(), "audio.flac", b"sound");
        let (status, body) = call(
            &app,
            "POST",
            "/app/devices/ingest",
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
            "/app/devices/ingest",
            Some(content_type),
            body,
            basis(CID_A),
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
        assert_eq!(event["cid"], CID_A);
        let (_, read_body) = call(
            &app,
            "GET",
            "/app/devices/ingest/manifest",
            None,
            Vec::new(),
            AccessBasis::Localhost,
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(read_body["reason_code"], "linked_device_required");
    }

    #[tokio::test]
    async fn legacy_fields_and_protocol_versions_are_refused() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
                "/app/devices/ingest",
                Some(content_type),
                body,
                basis(CID_A),
                version,
                &[],
            )
            .await;
            assert_eq!(actual, status);
            assert_eq!(body["reason_code"], code);
        }
    }

    #[tokio::test]
    async fn identity_is_independent_for_each_request() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let app = router(&root);
        for (cid, day, segment) in [
            (CID_A, "20260804", "120000_1"),
            (CID_B, "20260805", "120001_1"),
        ] {
            let request = envelope(day, segment, json!([{"submitted":"audio.flac"}]));
            let (content_type, body) = multipart(request, "audio.flac", b"sound");
            assert_eq!(
                call(
                    &app,
                    "POST",
                    "/app/devices/ingest",
                    Some(content_type),
                    body,
                    basis(cid),
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
        assert!(first.contains(CID_A));
        assert!(!first.contains(CID_B));
        assert!(second.contains(CID_B));
        assert!(!second.contains(CID_A));
    }

    #[tokio::test]
    async fn read_routes_return_event_provenance_and_present_custody() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
            "/app/devices/ingest/manifest",
            None,
            Vec::new(),
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(manifest["days"]["20260804"]["segments"], 1);
        let (_, day) = call(
            &app,
            "GET",
            "/app/devices/ingest/manifest/20260804",
            None,
            Vec::new(),
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(
            day["segments"]["120000_1"]["files"][0]["name"],
            "audio.flac"
        );
        assert!(
            day["segments"]["120000_1"]["files"][0]
                .get("submitted_name")
                .is_none()
        );
        assert_eq!(day["segments"]["120000_1"]["files"][0]["size"], 5);
        assert!(day["segments"]["120000_1"]["files"][0]["sha256"].is_string());
        let (_, segments) = call(
            &app,
            "GET",
            "/app/devices/ingest/segments/20260804",
            None,
            Vec::new(),
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(segments["items"][0]["files"][0]["status"], "present");
        let (status, refusal) = call(
            &app,
            "GET",
            "/app/devices/ingest/segments/20260804",
            None,
            Vec::new(),
            basis(CID_A),
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(refusal["reason_code"], "protocol_version_required");
    }

    #[tokio::test]
    async fn manifest_skips_malformed_device_ingest_events() {
        let dir = root();
        let root = dir.path().to_path_buf();
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
            "/app/devices/ingest/manifest",
            None,
            Vec::new(),
            basis(CID_A),
            Some("3"),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["days"], json!({}));
    }

    #[tokio::test]
    async fn shared_connection_limit_rejects_oversized_declared_body() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let (server, mut client) = tokio::io::duplex(128 * 1024);
        let task = tokio::spawn(async move {
            serve_connection(server, router(&root), basis(CID_A), &mux_builder())
                .await
                .unwrap();
        });
        let request = format!(
            "POST /app/devices/ingest HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            super::CONNECTION_BODY_LIMIT + 1
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

    const SEEDED_CREATED_AT: u64 = 1_700_000_000;
    const SEEDED_LAST_DAY: &str = "20260801";
    const SEEDED_LAST_SEGMENT: &str = "090000_1";
    const SEEDED_SEQ: u64 = 2;

    #[tokio::test]
    async fn ac1_adopts_unattributed_listing_stream() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "aaaaaaaa", "Desk", CID_A, "desk");
        seed_unattributed_stream(
            &root,
            "desk",
            SEEDED_CREATED_AT,
            SEEDED_LAST_DAY,
            SEEDED_LAST_SEGMENT,
            SEEDED_SEQ,
        );
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert!(
            !root.join("streams/device.json").exists(),
            "must not mint device beside the listing-resolved stream"
        );
        assert!(!root.join("streams/desk_2.json").exists());
        let record = stream_record(&root, "desk");
        assert_eq!(record["created_at"], SEEDED_CREATED_AT);
        assert_eq!(record["seq"], SEEDED_SEQ + 1);
        assert_eq!(record["last_day"], "20260804");
        assert_eq!(record["last_segment"], body["segment"]);
        assert_eq!(record["cid"], CID_A);
        assert_eq!(record["source"], "");
        let landed = body["segment"].as_str().unwrap();
        let marker: Value = serde_json::from_str(
            &fs::read_to_string(
                root.join("chronicle/20260804/desk")
                    .join(landed)
                    .join("stream.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(marker["stream"], "desk");
        assert_eq!(marker["prev_day"], SEEDED_LAST_DAY);
        assert_eq!(marker["prev_segment"], SEEDED_LAST_SEGMENT);
        assert_eq!(marker["seq"], SEEDED_SEQ + 1);
    }

    #[tokio::test]
    async fn ac2_duplicate_does_not_advance_attributed_listing_stream() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "aaaaaaaa", "Desk", CID_A, "desk");
        seed_unattributed_stream(
            &root,
            "desk",
            SEEDED_CREATED_AT,
            SEEDED_LAST_DAY,
            SEEDED_LAST_SEGMENT,
            SEEDED_SEQ,
        );
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        assert_eq!(
            call_upload(&app, request.clone(), "audio.flac", b"sound")
                .await
                .1["status"],
            "ok"
        );
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "duplicate");
        assert!(!root.join("streams/device.json").exists());
        let record = stream_record(&root, "desk");
        assert_eq!(record["seq"], SEEDED_SEQ + 1);
        assert_eq!(record["cid"], CID_A);
    }

    #[tokio::test]
    async fn ac3_second_cid_naming_the_same_stream_is_refused() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "aaaaaaaa", "Desk", CID_A, "desk");
        seed_observer(&root, "bbbbbbbb", "Desk", CID_B, "desk");
        seed_attributed_stream(
            &root,
            "desk",
            CID_A,
            SEEDED_CREATED_AT,
            SEEDED_LAST_DAY,
            SEEDED_LAST_SEGMENT,
            SEEDED_SEQ,
        );
        let before = fs::read(root.join("streams/desk.json")).unwrap();
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload_as(&app, CID_B, request, "audio.flac", b"other").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason_code"], "foreign_stream_binding");
        assert_eq!(fs::read(root.join("streams/desk.json")).unwrap(), before);
        assert!(!root.join("streams/device.json").exists());
        assert!(!root.join("streams/desk_2.json").exists());
    }

    #[tokio::test]
    async fn ac4_ambiguous_observers_refuse_the_write() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "aaaaaaaa", "One", CID_A, "one");
        seed_observer(&root, "cccccccc", "Many", CID_A, "many");
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason_code"], "ambiguous_device_observer");
        assert!(!root.join("streams/device.json").exists());
    }

    #[tokio::test]
    async fn ac5_foreign_cid_on_named_stream_is_refused() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_observer(&root, "bbbbbbbb", "Desk", CID_B, "desk");
        seed_attributed_stream(
            &root,
            "desk",
            CID_A,
            SEEDED_CREATED_AT,
            SEEDED_LAST_DAY,
            SEEDED_LAST_SEGMENT,
            SEEDED_SEQ,
        );
        let before = fs::read(root.join("streams/desk.json")).unwrap();
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload_as(&app, CID_B, request, "audio.flac", b"other").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason_code"], "foreign_stream_binding");
        assert_eq!(fs::read(root.join("streams/desk.json")).unwrap(), before);
        assert!(!root.join("streams/device.json").exists());
        assert!(!root.join("streams/desk_2.json").exists());
    }

    #[tokio::test]
    async fn ac6_no_observer_refuses_when_any_unattributed_record_exists() {
        let dir = root();
        let root = dir.path().to_path_buf();
        seed_unattributed_stream(
            &root,
            "desk",
            SEEDED_CREATED_AT,
            SEEDED_LAST_DAY,
            SEEDED_LAST_SEGMENT,
            SEEDED_SEQ,
        );
        let before = fs::read(root.join("streams/desk.json")).unwrap();
        let app = router(&root);
        let request = envelope("20260804", "120000_1", json!([{"submitted":"audio.flac"}]));
        let (status, body) = call_upload(&app, request, "audio.flac", b"sound").await;
        assert!(
            !root.join("streams/device.json").exists(),
            "must not mint device beside an unattributed record"
        );
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason_code"], "unattributed_stream_blocks_mint");
        assert_eq!(fs::read(root.join("streams/desk.json")).unwrap(), before);
        assert!(!root.join("streams/desk_2.json").exists());
    }
}
