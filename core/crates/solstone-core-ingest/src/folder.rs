// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Watched-folder ingest, per-folder received ledger, per-file validation, and
//! linked-device discovery endpoints (`/supported`, `/estimate`, `/facets`).
//!
//! Each watched folder uploads under the wire source `"folder"`, carrying one
//! `folder_<sha256>.jsonl` activity file per 5-minute window (`HHMMSS_300`) and
//! zero or more companion content parts referenced by `part` fields on lines in
//! that activity file.
//!
//! Internally, each folder binds its own stream source `folder_<12 hex>` (a
//! prefix of SHA-256 of `meta.folder.id`) so multiple folders active in the
//! same 5-minute window each keep their own segment without colliding.
//!
//! Per-folder state lives under `config/folders/<sha256(folder_id)[..32]>/`:
//! - `folder.json`: folder metadata, devices, renames, last sync time.
//! - `received/<hash[..2]>/<hash>.json`: erase-surviving record of every event
//!   and content item this folder has received, committed only after durable
//!   segment writes and read fail-closed (`503 retryable` when unreadable).
//! - `notes/<path_hash[..2]>/<path_hash>.json`: per-note latest text for
//!   first-sight + net-change diffing (same model as Obsidian).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use image::{ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_callosum::{
    DeviceIngestEvent, DurableEvent, FileDescriptor, append_durable_event,
};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_http::owner_read::{OwnerReadRole, spawn_blocking_response};
use solstone_core_format::calendar::names_calendar_file;
use solstone_core_format::folder::{
    DOCUMENT_REVISION_COOLDOWN_SECONDS, DOCUMENT_TRANSCRIPT_FILE, FOLDER_ACTIVITY_FILE,
    FOLDER_SOURCE, FOLDER_TRANSCRIPT_FILE, FOLDER_WINDOW_SECONDS, FolderActivityLine,
    FolderEventKind, FolderMeta, MAX_NOTE_BYTES, StoredFolderActivity, StoredFolderEvent,
    event_offset_ms, extract_non_folder_screen_text, folder_stream_source, is_valid_folder_id,
    merge_folder_activity_with_screen_text, names_folder_file, validate_folder_file,
    validate_folder_meta,
};
use solstone_core_format::obsidian::{
    KnownRename, NOTE_TRANSCRIPT_FILE, NoteEntry, names_obsidian_file, only_link_rewrites,
    render_edit_document, render_transcript_document,
};
use solstone_core_format::screenshots::{SCREENSHOT_STEM, names_screenshot_file};
use solstone_core_format::stored_markdown::{Document, Message};
use solstone_core_import::triage::{self, Kind as TriageKind};
use solstone_core_import_sources::document::{
    PdfCommand, PdfPayload, PdfWorkerRequest, WorkerFailure,
};
use solstone_core_ingest_resolve::IngestNotice;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};
use solstone_core_segment::{
    Kind as StreamKind, SegmentDir, StreamHints, advance_bound_stream, reclaim_for_arrival,
    retry_bound_stream,
};
use solstone_core_time::{Precision, zoned_rfc3339};

use crate::model::{IncomingFile, ReasonCode};
use crate::router::{
    IngestCompletion, IngestState, activity_timestamp, outcome_error, refusal, today,
};
use crate::screenshots::MAX_SCREENSHOT_PIXELS;
use crate::stream_identity::bind_ingest_stream;
use crate::validation::{validate_access, validate_protocol};

const STATE_DIR: &str = "config/folders";
const RENAME_MEMORY_MS: i64 = 24 * 60 * 60 * 1_000;
const PDF_WORKER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ESTIMATE_ITEMS: usize = 50_000;
const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

static PDF_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Why a PDF content part could not be extracted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PdfReadError {
    /// The file itself is corrupt, encrypted, or unreadable.
    FileInvalid(String),
    /// The journal's PDF extractor runtime is unavailable; caller should retry.
    Unavailable(String),
}

/// Injectable PDF text-extraction boundary so unit tests can exercise PDF
/// extraction, corrupt/encrypted file refusals, and worker unavailability
/// without spawning an external binary.
pub(crate) trait FolderPdfReader: Send + Sync {
    fn extract(&self, journal_root: &Path, bytes: &[u8]) -> Result<PdfPayload, PdfReadError>;
}

/// Production PDF reader backed by the sibling `solstone-core-pdf` worker.
pub(crate) struct SystemFolderPdfReader;

impl FolderPdfReader for SystemFolderPdfReader {
    fn extract(&self, journal_root: &Path, bytes: &[u8]) -> Result<PdfPayload, PdfReadError> {
        let worker =
            solstone_core_import_sources::document::installed_pdf_worker(PDF_WORKER_TIMEOUT)
                .map_err(PdfReadError::Unavailable)?;
        let tmp_dir = journal_root.join(STATE_DIR).join(".tmp");
        create_directory_with_mode(&tmp_dir, 0o700).map_err(|e| {
            PdfReadError::Unavailable(format!("cannot create pdf staging dir: {e}"))
        })?;
        let seq = PDF_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_path = tmp_dir.join(format!("pdf_{}_{}.pdf", std::process::id(), seq));
        let _guard = TempFileGuard(tmp_path.clone());
        atomic_replace(&tmp_path, bytes, PRIVATE)
            .map_err(|e| PdfReadError::Unavailable(format!("cannot stage pdf: {e}")))?;
        let request = PdfWorkerRequest {
            command: PdfCommand::Extract,
            source: tmp_path,
            password: None,
            render: None,
            #[cfg(windows)]
            resources: solstone_core_import_sources::document::PdfWorkerResources::default(),
        };
        worker.execute(&request).map_err(map_worker_failure)
    }
}

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn map_worker_failure(failure: WorkerFailure) -> PdfReadError {
    match failure {
        WorkerFailure::Process { error, detail, .. } => {
            let msg = match detail {
                Some(d) if !d.is_empty() => format!("{error}: {d}"),
                _ => error,
            };
            PdfReadError::FileInvalid(format!("PDF could not be read ({msg})"))
        }
        WorkerFailure::TimedOut { .. } => {
            PdfReadError::FileInvalid("PDF extraction timed out".to_owned())
        }
        WorkerFailure::ProtocolViolation { detail } => {
            PdfReadError::Unavailable(format!("PDF worker protocol error: {detail}"))
        }
        WorkerFailure::Cancelled => {
            PdfReadError::Unavailable("PDF extraction cancelled".to_owned())
        }
    }
}

/// Checked envelope-level folder upload ready for per-file checking and landing.
#[derive(Debug)]
pub(crate) struct CheckedFolderUpload {
    pub(crate) meta: FolderMeta,
    pub(crate) activity_file_name: String,
    pub(crate) lines: Vec<FolderActivityLine>,
    pub(crate) parts: BTreeMap<String, Vec<u8>>,
}

/// Per-file refusal surfaced alongside landed items when a single file in a
/// window is broken or invalid.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RefusedFile {
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) part: Option<String>,
    pub(crate) reason_code: String,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct NoteState {
    path: String,
    text: String,
    content_hash: String,
    changed_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredRename {
    from: String,
    to: String,
    at_ms: i64,
}

/// Persisted folder metadata at `config/folders/<hash32>/folder.json`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FolderState {
    pub v: u32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub devices: BTreeMap<String, String>,
    #[serde(default)]
    renames: Vec<StoredRename>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<String>,
    #[serde(default)]
    pub event_count: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ReceivedRecord {
    day: String,
    segment: String,
    received_at: String,
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn folder_dir(journal: &Path, folder_id: &str) -> PathBuf {
    journal
        .join(STATE_DIR)
        .join(&sha256_hex(folder_id.as_bytes())[..32])
}

fn received_dir(journal: &Path, folder_id: &str) -> PathBuf {
    folder_dir(journal, folder_id).join("received")
}

fn received_path(journal: &Path, folder_id: &str, key_hash: &str) -> PathBuf {
    received_dir(journal, folder_id)
        .join(&key_hash[..2])
        .join(format!("{key_hash}.json"))
}

fn note_file(journal: &Path, folder_id: &str, path: &str) -> PathBuf {
    let hash = sha256_hex(path.as_bytes());
    folder_dir(journal, folder_id)
        .join("notes")
        .join(&hash[..2])
        .join(format!("{hash}.json"))
}

fn read_json_optional<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_json_private<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_ancestors(parent)?;
    }
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, PRIVATE).map_err(io::Error::other)?;
    Ok(())
}

fn create_private_ancestors(dir: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut cursor = Some(dir);
    while let Some(current) = cursor {
        if current.exists() {
            break;
        }
        missing.push(current.to_path_buf());
        cursor = current.parent();
    }
    for path in missing.into_iter().rev() {
        create_directory_with_mode(&path, 0o700).map_err(io::Error::other)?;
    }
    Ok(())
}

/// Fail-closed received lookup: anything other than a clean miss or valid
/// record returns an error so an unreadable ledger never re-admits erased data.
fn lookup_received(
    journal: &Path,
    folder_id: &str,
    key_hash: &str,
) -> io::Result<Option<ReceivedRecord>> {
    match std::fs::read(received_path(journal, folder_id, key_hash)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(received_dir(journal, folder_id)) {
                Ok(metadata) if metadata.is_dir() => Ok(None),
                Ok(_) => Err(io::Error::other(
                    "folder received record is not a directory",
                )),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn record_received(
    journal: &Path,
    folder_id: &str,
    key_hash: &str,
    record: &ReceivedRecord,
) -> io::Result<()> {
    write_json_private(&received_path(journal, folder_id, key_hash), record)
}

fn event_dedup_key(line: &FolderActivityLine) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"event\0");
    hasher.update(line.event.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(line.path.as_bytes());
    hasher.update(b"\0");
    if let Some(prev) = &line.previous_path {
        hasher.update(prev.as_bytes());
    }
    hasher.update(b"\0");
    hasher.update(line.at_ms().to_string().as_bytes());
    hasher.update(b"\0");
    if let Some(hash) = &line.content_hash {
        hasher.update(hash.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn once_content_key(kind_word: &str, content_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"once\0");
    hasher.update(kind_word.as_bytes());
    hasher.update(b"\0");
    hasher.update(content_hash.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn revision_content_key(kind_word: &str, path: &str, content_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rev\0");
    hasher.update(kind_word.as_bytes());
    hasher.update(b"\0");
    hasher.update(path.as_bytes());
    hasher.update(b"\0");
    hasher.update(content_hash.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn invalid_envelope(detail: impl Into<String>) -> (ReasonCode, String) {
    (ReasonCode::FolderInvalid, detail.into())
}

fn is_reserved_companion_name(name: &str) -> bool {
    names_folder_file(name)
        || names_obsidian_file(name)
        || names_calendar_file(name)
        || names_screenshot_file(name)
        || (name.starts_with("browser_") && name.ends_with(".jsonl"))
        || matches!(
            name,
            NOTE_TRANSCRIPT_FILE | DOCUMENT_TRANSCRIPT_FILE | FOLDER_TRANSCRIPT_FILE
        )
}

/// Validate envelope-level structure for a `"folder"` upload.
///
/// Per-file content checks (hash match, image decoding, PDF extraction, UTF-8
/// decoding) run later in `write_folder` so one broken file is refused by name
/// without blocking the rest of the window.
pub(crate) fn validate_folder_upload(
    source: &str,
    day: &str,
    segment: &str,
    meta: &Map<String, Value>,
    files: &[IncomingFile],
) -> Result<Option<CheckedFolderUpload>, (ReasonCode, String)> {
    if source != FOLDER_SOURCE {
        if files.iter().any(|file| names_folder_file(&file.submitted)) {
            return Err(invalid_envelope(
                "folder activity files are accepted only from the folder source",
            ));
        }
        return Ok(None);
    }
    let folder_meta =
        validate_folder_meta(meta).map_err(|error| invalid_envelope(error.to_string()))?;
    let activity_files: Vec<&IncomingFile> = files
        .iter()
        .filter(|file| names_folder_file(&file.submitted))
        .collect();
    let [activity] = activity_files.as_slice() else {
        return Err(invalid_envelope(
            "the folder source requires exactly one folder_<sha256>.jsonl activity file per upload",
        ));
    };
    let lines = validate_folder_file(&activity.submitted, &activity.bytes, day, segment)
        .map_err(|error| invalid_envelope(error.to_string()))?;

    let mut parts = BTreeMap::new();
    for file in files {
        if file.submitted == activity.submitted {
            continue;
        }
        if is_reserved_companion_name(&file.submitted) {
            return Err(invalid_envelope(format!(
                "companion file part '{}' uses a reserved name",
                file.submitted
            )));
        }
        parts.insert(file.submitted.clone(), file.bytes.clone());
    }

    let mut referenced = BTreeSet::new();
    for (idx, line) in lines.iter().enumerate() {
        if let Some(part) = &line.part {
            if !parts.contains_key(part) {
                return Err(invalid_envelope(format!(
                    "line {}: part '{}' is missing from multipart files",
                    idx + 1,
                    part
                )));
            }
            referenced.insert(part.clone());
        }
    }
    for part in parts.keys() {
        if !referenced.contains(part) {
            return Err(invalid_envelope(format!(
                "file part '{}' is not referenced by any activity line",
                part
            )));
        }
    }

    Ok(Some(CheckedFolderUpload {
        meta: folder_meta,
        activity_file_name: activity.submitted.clone(),
        lines,
        parts,
    }))
}

/// Resolved live facet inventory for validating owner-chosen folder facets and
/// serving `GET /app/devices/ingest/folder/facets`.
#[derive(Clone, Debug, Serialize)]
struct FacetItem {
    name: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(skip_serializing)]
    muted: bool,
}

fn read_journal_facets(journal: &Path) -> io::Result<Vec<FacetItem>> {
    let names = solstone_core_facets::list_facet_directories(journal)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut rows = Vec::new();
    for name in names {
        let Ok(Some(declaration)) = solstone_core_facets::observe_facet_declaration(journal, &name)
        else {
            continue;
        };
        let Some(obj) = declaration.value().as_object() else {
            continue;
        };
        if obj.contains_key("id")
            && !obj
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(solstone_core_facets::is_well_formed_facet_id)
        {
            continue;
        }
        let title = obj
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| name.clone());
        let color = obj.get("color").and_then(Value::as_str).map(str::to_owned);
        let muted = obj.get("muted").and_then(Value::as_bool).unwrap_or(false);
        rows.push(FacetItem {
            name,
            title,
            color,
            muted,
        });
    }
    rows.sort_by_key(|row| row.title.to_ascii_lowercase());
    Ok(rows)
}

fn resolve_declared_facet(journal: &Path, requested: Option<&str>) -> Option<String> {
    let slug = requested?.trim();
    if slug.is_empty() {
        return None;
    }
    let facets = read_journal_facets(journal).ok()?;
    if facets.iter().any(|f| f.name == slug) {
        return Some(slug.to_owned());
    }
    None
}

/// Note slot used by the per-folder Obsidian-style note diff engine.
struct NoteSlot {
    start_path: String,
    path: String,
    base: Option<String>,
    renamed_from: Option<String>,
    deleted: bool,
    vanished: bool,
    text: String,
    at_ms: i64,
}

struct PendingDocument {
    markdown: String,
}

struct PendingTranscript {
    rows: Vec<Value>,
}

struct PendingImage {
    is_screenshot: bool,
    ext: &'static str,
    hash: String,
    bytes: Vec<u8>,
}

/// Process and durably land one watched-folder 5-minute window upload.
///
/// The caller holds `hold_source_mutation(&state.journal_root, "folder")`, so
/// uploads for a folder apply strictly serially against its received ledger and
/// note state.
pub(crate) fn write_folder(
    state: &IngestState,
    cid: &str,
    day: String,
    segment: String,
    meta: Map<String, Value>,
    upload: CheckedFolderUpload,
) -> IngestCompletion {
    let retryable = |code: ReasonCode, detail: &str| {
        IngestCompletion::rejected(
            code,
            outcome_error("retryable", code, StatusCode::SERVICE_UNAVAILABLE, detail),
        )
    };
    let failed = |code: ReasonCode, detail: &str| {
        IngestCompletion::rejected(
            code,
            outcome_error("failed", code, StatusCode::INTERNAL_SERVER_ERROR, detail),
        )
    };

    if day > today(&state.journal_root, (state.now_ms)()) {
        return IngestCompletion::rejected(
            ReasonCode::FolderAhead,
            refusal(
                ReasonCode::FolderAhead,
                StatusCode::CONFLICT,
                "this window is later than the journal's today; send it again later",
            ),
        );
    }

    let folder_id = &upload.meta.id;
    let folder_state_path = folder_dir(&state.journal_root, folder_id).join("folder.json");
    let mut folder_state = match read_json_optional::<FolderState>(&folder_state_path) {
        Ok(Some(existing)) => existing,
        Ok(None) => FolderState {
            v: 1,
            id: folder_id.clone(),
            name: upload.meta.name.clone(),
            facet: upload.meta.facet.clone(),
            kinds: upload.meta.kinds.clone(),
            ..FolderState::default()
        },
        Err(error) => {
            log::warn!("folder state unreadable for {folder_id}: {error}");
            return retryable(
                ReasonCode::JournalReadFailed,
                "cannot read watched folder state; try again",
            );
        }
    };

    let resolved_facet = resolve_declared_facet(&state.journal_root, upload.meta.facet.as_deref());

    let mut lines = upload.lines;
    lines.sort_by_key(FolderActivityLine::at_ms);

    let mut note_overlay: HashMap<String, Option<NoteState>> = HashMap::new();
    let mut note_read: HashMap<String, Option<NoteState>> = HashMap::new();
    let mut note_slots: Vec<NoteSlot> = Vec::new();
    let mut note_slot_at: HashMap<String, usize> = HashMap::new();
    let mut new_renames: Vec<StoredRename> = Vec::new();

    let mut pending_docs: Vec<PendingDocument> = Vec::new();
    let mut pending_transcripts: Vec<PendingTranscript> = Vec::new();
    let mut pending_images: Vec<PendingImage> = Vec::new();
    let mut new_activity_events: Vec<StoredFolderEvent> = Vec::new();
    let mut pending_received: BTreeSet<String> = BTreeSet::new();
    let mut refused_files: Vec<RefusedFile> = Vec::new();
    let mut latest_observed_ms: Option<i64> = None;
    let mut latest_observed_rfc3339: Option<String> = None;

    for line in &lines {
        let obs_ms = line.observed_at_ms();
        if latest_observed_ms.is_none_or(|prev| obs_ms > prev) {
            latest_observed_ms = Some(obs_ms);
            latest_observed_rfc3339 =
                Some(zoned_rfc3339(&line.observed_at, Precision::AutoSi, false));
        }

        let ev_key = event_dedup_key(line);
        if pending_received.contains(&ev_key) {
            continue;
        }
        match lookup_received(&state.journal_root, folder_id, &ev_key) {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(error) => {
                log::warn!("folder received record unreadable: {error}");
                return retryable(
                    ReasonCode::JournalReadFailed,
                    "cannot read what this folder has received; try again",
                );
            }
        }

        let Some(part_name) = line.part.as_deref() else {
            // Activity-only event (unsupported file, deleted before send,
            // rename, remove, or kind turned off).
            if line.event == FolderEventKind::Removed
                && let Err(error) = apply_note_removed(
                    &state.journal_root,
                    folder_id,
                    &line.path,
                    line.at_ms(),
                    &mut note_overlay,
                    &mut note_read,
                    &mut note_slots,
                    &mut note_slot_at,
                )
            {
                log::warn!("folder note state unreadable: {error}");
                return retryable(
                    ReasonCode::JournalReadFailed,
                    "cannot read folder note state; try again",
                );
            }
            pending_received.insert(ev_key.clone());
            new_activity_events.push(stored_event(line, &day, &segment, ev_key, None, None));
            continue;
        };

        let bytes = &upload.parts[part_name];
        let actual_hash = sha256_hex(bytes);
        let expected_hash = line.content_hash.as_deref().unwrap_or_default();
        if actual_hash != expected_hash {
            refused_files.push(RefusedFile {
                path: line.path.clone(),
                part: Some(part_name.to_owned()),
                reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                detail: "content_hash does not match file bytes".to_owned(),
            });
            continue;
        }

        let rel_path = Path::new(&line.path);
        let basename = rel_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&line.path);
        let parent_dir = rel_path.parent().unwrap_or_else(|| Path::new(""));
        let verdict =
            triage::classify(bytes, basename, parent_dir, line.kind_hint.as_deref(), None);

        match verdict.kind {
            TriageKind::Notes => {
                if bytes.len() > MAX_NOTE_BYTES {
                    refused_files.push(RefusedFile {
                        path: line.path.clone(),
                        part: Some(part_name.to_owned()),
                        reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                        detail: "note exceeds 4 MiB limit".to_owned(),
                    });
                    continue;
                }
                if bytes.contains(&0) {
                    refused_files.push(RefusedFile {
                        path: line.path.clone(),
                        part: Some(part_name.to_owned()),
                        reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                        detail: "note contains binary NUL bytes".to_owned(),
                    });
                    continue;
                }
                let Some(decoded) = triage::decode_text(bytes) else {
                    refused_files.push(RefusedFile {
                        path: line.path.clone(),
                        part: Some(part_name.to_owned()),
                        reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                        detail: "note is not valid UTF-8 or UTF-16 text".to_owned(),
                    });
                    continue;
                };
                let applied = apply_note_change(
                    &state.journal_root,
                    folder_id,
                    line,
                    decoded.into_owned(),
                    actual_hash.clone(),
                    &mut note_overlay,
                    &mut note_read,
                    &mut note_slots,
                    &mut note_slot_at,
                    &mut new_renames,
                );
                match applied {
                    Ok(true) => {
                        pending_received.insert(ev_key.clone());
                        new_activity_events.push(stored_event(
                            line,
                            &day,
                            &segment,
                            ev_key,
                            Some("notes"),
                            Some(NOTE_TRANSCRIPT_FILE),
                        ));
                    }
                    Ok(false) => {
                        // Identical content_hash or older change: already held.
                        pending_received.insert(ev_key);
                    }
                    Err(error) => {
                        log::warn!("folder note state unreadable: {error}");
                        return retryable(
                            ReasonCode::JournalReadFailed,
                            "cannot read folder note state; try again",
                        );
                    }
                }
            }
            TriageKind::Documents => {
                let rev_key = revision_content_key("documents", &line.path, &actual_hash);
                if pending_received.contains(&rev_key) {
                    continue;
                }
                match lookup_received(&state.journal_root, folder_id, &rev_key) {
                    Ok(Some(_)) => continue,
                    Ok(None) => {}
                    Err(error) => {
                        log::warn!("folder received record unreadable: {error}");
                        return retryable(
                            ReasonCode::JournalReadFailed,
                            "cannot read what this folder has received; try again",
                        );
                    }
                }
                let ext = rel_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default();
                let is_pdf = ext == "pdf" || bytes.starts_with(b"%PDF-");
                let rendered = if is_pdf {
                    if !bytes.starts_with(b"%PDF-") {
                        refused_files.push(RefusedFile {
                            path: line.path.clone(),
                            part: Some(part_name.to_owned()),
                            reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                            detail: "PDF header is missing or malformed".to_owned(),
                        });
                        continue;
                    }
                    match state.pdf_reader.extract(&state.journal_root, bytes) {
                        Ok(payload) => match render_pdf_markdown(&line.path, &payload) {
                            Ok(md) => md,
                            Err(detail) => {
                                refused_files.push(RefusedFile {
                                    path: line.path.clone(),
                                    part: Some(part_name.to_owned()),
                                    reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                                    detail,
                                });
                                continue;
                            }
                        },
                        Err(PdfReadError::FileInvalid(detail)) => {
                            refused_files.push(RefusedFile {
                                path: line.path.clone(),
                                part: Some(part_name.to_owned()),
                                reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                                detail,
                            });
                            continue;
                        }
                        Err(PdfReadError::Unavailable(detail)) => {
                            log::warn!("folder PDF reader unavailable: {detail}");
                            return retryable(
                                ReasonCode::JournalReadFailed,
                                "PDF extraction is temporarily unavailable; try again",
                            );
                        }
                    }
                } else if matches!(ext.as_str(), "md" | "markdown" | "txt" | "log") {
                    if bytes.contains(&0) {
                        refused_files.push(RefusedFile {
                            path: line.path.clone(),
                            part: Some(part_name.to_owned()),
                            reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                            detail: "document contains binary NUL bytes".to_owned(),
                        });
                        continue;
                    }
                    match triage::decode_text(bytes) {
                        Some(decoded) if !decoded.trim().is_empty() => {
                            format!("# {}\n\n{}\n", line.path, decoded.trim_end())
                        }
                        _ => {
                            refused_files.push(RefusedFile {
                                path: line.path.clone(),
                                part: Some(part_name.to_owned()),
                                reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                                detail: "document text could not be decoded or was empty"
                                    .to_owned(),
                            });
                            continue;
                        }
                    }
                } else {
                    refused_files.push(RefusedFile {
                        path: line.path.clone(),
                        part: Some(part_name.to_owned()),
                        reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                        detail: format!("unsupported document format '.{ext}'; send metadata only"),
                    });
                    continue;
                };
                // If this path was previously tracked as a note and crossed the
                // document threshold, retire its note diff baseline.
                note_overlay.insert(line.path.clone(), None);
                pending_docs.push(PendingDocument { markdown: rendered });
                pending_received.insert(ev_key.clone());
                pending_received.insert(rev_key);
                new_activity_events.push(stored_event(
                    line,
                    &day,
                    &segment,
                    ev_key,
                    Some("documents"),
                    Some(DOCUMENT_TRANSCRIPT_FILE),
                ));
            }
            TriageKind::Transcripts => {
                let rev_key = revision_content_key("transcripts", &line.path, &actual_hash);
                if pending_received.contains(&rev_key) {
                    continue;
                }
                match lookup_received(&state.journal_root, folder_id, &rev_key) {
                    Ok(Some(_)) => continue,
                    Ok(None) => {}
                    Err(error) => {
                        log::warn!("folder received record unreadable: {error}");
                        return retryable(
                            ReasonCode::JournalReadFailed,
                            "cannot read what this folder has received; try again",
                        );
                    }
                }
                let Some(rows) = solstone_core_import::text::parse_folder_transcript_rows(
                    bytes,
                    basename,
                    &verdict,
                    line.at.timestamp(),
                    resolved_facet.as_deref(),
                ) else {
                    refused_files.push(RefusedFile {
                        path: line.path.clone(),
                        part: Some(part_name.to_owned()),
                        reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                        detail: "transcript could not be parsed or had no turns".to_owned(),
                    });
                    continue;
                };
                pending_transcripts.push(PendingTranscript { rows });
                pending_received.insert(ev_key.clone());
                pending_received.insert(rev_key);
                new_activity_events.push(stored_event(
                    line,
                    &day,
                    &segment,
                    ev_key,
                    Some("transcripts"),
                    Some(FOLDER_TRANSCRIPT_FILE),
                ));
            }
            TriageKind::Screenshots | TriageKind::Screen => {
                let once_key = once_content_key("screenshots", &actual_hash);
                if pending_received.contains(&once_key) {
                    continue;
                }
                match lookup_received(&state.journal_root, folder_id, &once_key) {
                    Ok(Some(_)) => continue,
                    Ok(None) => {}
                    Err(error) => {
                        log::warn!("folder received record unreadable: {error}");
                        return retryable(
                            ReasonCode::JournalReadFailed,
                            "cannot read what this folder has received; try again",
                        );
                    }
                }
                let ext = match check_folder_image(basename, bytes) {
                    Ok(ext) => ext,
                    Err(detail) => {
                        refused_files.push(RefusedFile {
                            path: line.path.clone(),
                            part: Some(part_name.to_owned()),
                            reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                            detail,
                        });
                        continue;
                    }
                };
                pending_images.push(PendingImage {
                    is_screenshot: true,
                    ext,
                    hash: actual_hash.clone(),
                    bytes: bytes.clone(),
                });
                pending_received.insert(ev_key.clone());
                pending_received.insert(once_key);
                new_activity_events.push(stored_event(
                    line,
                    &day,
                    &segment,
                    ev_key,
                    Some("screenshots"),
                    None,
                ));
            }
            TriageKind::Photos => {
                let once_key = once_content_key("photos", &actual_hash);
                if pending_received.contains(&once_key) {
                    continue;
                }
                match lookup_received(&state.journal_root, folder_id, &once_key) {
                    Ok(Some(_)) => continue,
                    Ok(None) => {}
                    Err(error) => {
                        log::warn!("folder received record unreadable: {error}");
                        return retryable(
                            ReasonCode::JournalReadFailed,
                            "cannot read what this folder has received; try again",
                        );
                    }
                }
                let ext = match check_folder_image(basename, bytes) {
                    Ok(ext) => ext,
                    Err(detail) => {
                        refused_files.push(RefusedFile {
                            path: line.path.clone(),
                            part: Some(part_name.to_owned()),
                            reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                            detail,
                        });
                        continue;
                    }
                };
                pending_images.push(PendingImage {
                    is_screenshot: false,
                    ext,
                    hash: actual_hash.clone(),
                    bytes: bytes.clone(),
                });
                pending_received.insert(ev_key.clone());
                pending_received.insert(once_key);
                new_activity_events.push(stored_event(
                    line,
                    &day,
                    &segment,
                    ev_key,
                    Some("photos"),
                    None,
                ));
            }
            other => {
                refused_files.push(RefusedFile {
                    path: line.path.clone(),
                    part: Some(part_name.to_owned()),
                    reason_code: ReasonCode::FolderFileInvalid.as_str().to_owned(),
                    detail: format!(
                        "file kind '{other:?}' is not supported for folder content; send metadata only"
                    ),
                });
            }
        }
    }

    // Build note transcript document (if any net note changes exist).
    let mut all_renames = folder_state.renames.clone();
    all_renames.extend(new_renames.iter().cloned());
    let mut note_entries: Vec<(i64, NoteEntry<Document>)> = Vec::new();
    for slot in &note_slots {
        let near = renames_near(&all_renames, slot.at_ms);
        if let Some(entry) = note_entry_for(slot, &near) {
            note_entries.push((slot.at_ms, entry));
        }
    }
    note_entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.path.cmp(&b.1.path)));
    let note_entries: Vec<NoteEntry<Document>> = note_entries.into_iter().map(|(_, e)| e).collect();
    let note_document =
        (!note_entries.is_empty()).then(|| render_transcript_document(&note_entries));

    let has_content = note_document.is_some()
        || !pending_docs.is_empty()
        || !pending_transcripts.is_empty()
        || !pending_images.is_empty();

    if new_activity_events.is_empty() && !has_content {
        if !refused_files.is_empty() {
            let detail = refused_files[0].detail.clone();
            let body = json!({
                "error": "Ingest request refused",
                "reason_code": ReasonCode::FolderFileInvalid.as_str(),
                "detail": detail,
                "refused_files": refused_files,
            });
            return IngestCompletion::rejected(
                ReasonCode::FolderFileInvalid,
                (StatusCode::BAD_REQUEST, Json(body)).into_response(),
            );
        }
        // Every item was already received before (including items since erased).
        let body = json!({
            "status": "duplicate",
            "existing_segment": segment,
            "message": "Nothing new in this window",
            "file_descriptors": [],
            "meta": meta,
        });
        return IngestCompletion::accepted(day, segment, Json(body).into_response());
    }

    // Bind this folder's dedicated stream (`folder_<12 hex>`).
    let stream_source = folder_stream_source(folder_id);
    let hints = StreamHints {
        kind: Some(StreamKind::Observed),
        host: None,
        platform: None,
    };
    let bound = match bind_ingest_stream(
        &state.journal_root,
        &day,
        &segment,
        cid,
        &stream_source,
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

    let seg_dir = match SegmentDir::resolve(&state.journal_root, &day, &segment, &bound.stream) {
        Ok(dir) => dir,
        Err(_) => {
            return failed(
                ReasonCode::JournalWriteFailed,
                "cannot resolve folder segment directory",
            );
        }
    };
    match reclaim_for_arrival(seg_dir.path()) {
        Ok(solstone_core_segment::Arrival::Removing) => {
            return failed(ReasonCode::SegmentRemoved, "this segment is being removed");
        }
        Ok(_) => {}
        Err(_) => {
            return failed(ReasonCode::JournalReadFailed, "cannot read segment state");
        }
    }
    let talents_dir = seg_dir.path().join("talents");
    if create_private_ancestors(&talents_dir).is_err() {
        return failed(
            ReasonCode::JournalWriteFailed,
            "cannot create folder segment directory",
        );
    }

    // Land content files into the 5-minute segment (merging cleanly when an
    // earlier upload already populated this window).
    let mut written_content_files: Vec<String> = Vec::new();
    let mut descriptors: Vec<FileDescriptor> = Vec::new();
    let mut total_written_bytes: u64 = 0;

    if let Some(doc) = &note_document {
        let note_path = seg_dir.path().join(NOTE_TRANSCRIPT_FILE);
        let merged_bytes = match std::fs::read(&note_path) {
            Ok(existing) if !existing.is_empty() => {
                let mut combined = existing;
                if !combined.ends_with(b"\n") {
                    combined.push(b'\n');
                }
                combined.push(b'\n');
                combined.extend_from_slice(doc.as_bytes());
                combined
            }
            _ => doc.as_bytes().to_vec(),
        };
        if atomic_replace(&note_path, &merged_bytes, PRIVATE).is_err() {
            return failed(
                ReasonCode::JournalWriteFailed,
                "cannot write folder note transcript",
            );
        }
        if merged_bytes.as_slice() == doc.as_bytes()
            && let Err(error) = crate::obsidian_display::save(seg_dir.path(), doc)
        {
            log::warn!("folder note display receipt could not be saved: {error}");
        }
        written_content_files.push(NOTE_TRANSCRIPT_FILE.to_owned());
        total_written_bytes += merged_bytes.len() as u64;
        descriptors.push(written_descriptor(
            NOTE_TRANSCRIPT_FILE,
            NOTE_TRANSCRIPT_FILE,
            &merged_bytes,
        ));
    }

    if !pending_docs.is_empty() {
        let doc_path = seg_dir.path().join(DOCUMENT_TRANSCRIPT_FILE);
        let mut combined = match std::fs::read_to_string(&doc_path) {
            Ok(existing) if !existing.trim().is_empty() => existing,
            _ => String::new(),
        };
        for doc in &pending_docs {
            if !combined.is_empty() {
                if !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str("\n---\n\n");
            }
            combined.push_str(&doc.markdown);
            if !combined.ends_with('\n') {
                combined.push('\n');
            }
        }
        if atomic_replace(&doc_path, combined.as_bytes(), PRIVATE).is_err() {
            return failed(
                ReasonCode::JournalWriteFailed,
                "cannot write folder document transcript",
            );
        }
        written_content_files.push(DOCUMENT_TRANSCRIPT_FILE.to_owned());
        total_written_bytes += combined.len() as u64;
        descriptors.push(written_descriptor(
            DOCUMENT_TRANSCRIPT_FILE,
            DOCUMENT_TRANSCRIPT_FILE,
            combined.as_bytes(),
        ));
    }

    if !pending_transcripts.is_empty() {
        let tr_path = seg_dir.path().join(FOLDER_TRANSCRIPT_FILE);
        let existing_present = tr_path.is_file();
        let mut combined: Vec<u8> = std::fs::read(&tr_path).unwrap_or_default();
        for (idx, tr) in pending_transcripts.iter().enumerate() {
            let skip_header = existing_present || idx > 0;
            for (row_idx, row) in tr.rows.iter().enumerate() {
                if skip_header && row_idx == 0 {
                    continue;
                }
                if let Ok(line) = serde_json::to_string(row) {
                    combined.extend_from_slice(line.as_bytes());
                    combined.push(b'\n');
                }
            }
        }
        if atomic_replace(&tr_path, &combined, PRIVATE).is_err() {
            return failed(
                ReasonCode::JournalWriteFailed,
                "cannot write folder transcript",
            );
        }
        written_content_files.push(FOLDER_TRANSCRIPT_FILE.to_owned());
        total_written_bytes += combined.len() as u64;
        descriptors.push(written_descriptor(
            FOLDER_TRANSCRIPT_FILE,
            FOLDER_TRANSCRIPT_FILE,
            &combined,
        ));
    }

    let mut primary_screenshot_taken = has_primary_screenshot(seg_dir.path());
    for img in &pending_images {
        let filename = if img.is_screenshot {
            if !primary_screenshot_taken {
                primary_screenshot_taken = true;
                format!("{SCREENSHOT_STEM}.{}", img.ext)
            } else {
                format!("screenshot_{}_screen.{}", &img.hash[..12], img.ext)
            }
        } else {
            format!("photo_{}.{}", &img.hash[..12], img.ext)
        };
        let target = seg_dir.path().join(&filename);
        if atomic_replace(&target, &img.bytes, PRIVATE).is_err() {
            return failed(
                ReasonCode::JournalWriteFailed,
                "cannot write folder image content",
            );
        }
        // Attach landed filename to matching activity event.
        if let Some(ev) = new_activity_events.iter_mut().find(|ev| {
            ev.content_hash.as_deref() == Some(img.hash.as_str()) && ev.landed_file.is_none()
        }) {
            ev.landed_file = Some(filename.clone());
        }
        written_content_files.push(filename.clone());
        total_written_bytes += img.bytes.len() as u64;
        descriptors.push(written_descriptor(&filename, &filename, &img.bytes));
    }

    // Merge and persist `talents/folder_activity.json` and deterministic
    // `talents/screen_text.md` (preserving any existing OCR blocks).
    let activity_json_path = talents_dir.join(FOLDER_ACTIVITY_FILE);
    let mut stored_activity = match read_json_optional::<StoredFolderActivity>(&activity_json_path)
    {
        Ok(Some(existing)) => existing,
        Ok(None) => StoredFolderActivity {
            v: 1,
            folder_id: folder_id.clone(),
            folder_name: upload.meta.name.clone(),
            owner_facet: resolved_facet.clone(),
            events: Vec::new(),
        },
        Err(error) => {
            log::warn!("folder_activity.json unreadable: {error}");
            return failed(
                ReasonCode::JournalReadFailed,
                "cannot read existing folder activity record",
            );
        }
    };
    stored_activity.folder_name = upload.meta.name.clone();
    if let Some(facet) = &resolved_facet {
        stored_activity.owner_facet = Some(facet.clone());
    }
    let existing_keys: BTreeSet<String> = stored_activity
        .events
        .iter()
        .map(|ev| ev.key.clone())
        .collect();
    for ev in &new_activity_events {
        if !existing_keys.contains(&ev.key) {
            stored_activity.events.push(ev.clone());
        }
    }
    if write_json_private(&activity_json_path, &stored_activity).is_err() {
        return failed(
            ReasonCode::JournalWriteFailed,
            "cannot write folder activity record",
        );
    }

    let screen_text_path = talents_dir.join("screen_text.md");
    let existing_ocr = std::fs::read_to_string(&screen_text_path)
        .ok()
        .map(|existing| extract_non_folder_screen_text(&existing))
        .unwrap_or_default();
    let rendered_screen_text =
        merge_folder_activity_with_screen_text(&stored_activity, &existing_ocr);
    if atomic_replace(&screen_text_path, rendered_screen_text.as_bytes(), PRIVATE).is_err() {
        return failed(
            ReasonCode::JournalWriteFailed,
            "cannot write folder screen_text record",
        );
    }
    if descriptors.is_empty() {
        total_written_bytes += rendered_screen_text.len() as u64;
        descriptors.push(written_descriptor(
            &upload.activity_file_name,
            "screen_text.md",
            rendered_screen_text.as_bytes(),
        ));
    }

    // Apply owner-chosen facet to `talents/facets.json` (unioning with any
    // facets already present so content processing never loses its facets and
    // facet-limited connections immediately see the folder segment).
    if let Some(slug) = &resolved_facet {
        let facets_path = talents_dir.join("facets.json");
        let mut rows: Vec<Value> = std::fs::read(&facets_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if !rows
            .iter()
            .any(|row| row.get("facet").and_then(Value::as_str) == Some(slug.as_str()))
        {
            rows.push(json!({"facet": slug, "level": "high"}));
            if write_json_private(&facets_path, &rows).is_err() {
                return failed(
                    ReasonCode::JournalWriteFailed,
                    "cannot write folder facet assignment",
                );
            }
        }
    }

    // Append custody receipt (`DeviceIngestEvent`) with `stream_source` matching
    // the bound `StreamRecord.source` so `GET /app/devices/ingest/segments/{day}`
    // verifies cleanly.
    let receipt = DeviceIngestEvent {
        record_type: "device_ingest".to_owned(),
        record_version: 1,
        outcome: "accepted".to_owned(),
        protocol_version: 3,
        cid: cid.to_owned(),
        source: stream_source.clone(),
        stream: bound.stream.clone(),
        day: day.clone(),
        segment: segment.clone(),
        files: descriptors.clone(),
        meta: meta.clone(),
        extra: Map::new(),
    };
    if append_durable_event(seg_dir.path(), &DurableEvent::DeviceIngest(receipt)).is_err() {
        return failed(
            ReasonCode::EventAppendFailed,
            "cannot append folder ingest event",
        );
    }

    let advance_ok = advance_bound_stream(
        &bound.stream,
        &day,
        &segment,
        &seg_dir,
        hints.clone(),
        cid,
        &stream_source,
    )
    .or_else(|_| {
        retry_bound_stream(
            &bound.stream,
            &day,
            &segment,
            &seg_dir,
            hints,
            cid,
            &stream_source,
        )
    });
    if advance_ok.is_err() {
        return failed(
            ReasonCode::StreamAdvanceFailed,
            "cannot advance folder stream",
        );
    }

    // Always index `talents/screen_text.md` immediately so activity records are
    // searchable without waiting for any talent or model pass.
    let screen_text_rel = format!("{day}/{}/{segment}/talents/screen_text.md", bound.stream);
    if let Err(error) =
        solstone_core_indexer_todo::enqueue_path(&state.journal_root, Path::new(&screen_text_rel))
    {
        log::warn!("folder screen_text indexer enqueue failed: {error}");
    }
    for file in &written_content_files {
        let rel = format!("{day}/{}/{segment}/{file}", bound.stream);
        if solstone_core_format::content::resolve_spec(&rel).is_some()
            && let Err(error) =
                solstone_core_indexer_todo::enqueue_path(&state.journal_root, Path::new(&rel))
        {
            log::warn!("folder content indexer enqueue failed for {rel}: {error}");
        }
    }

    // Only dirty the day and notify Callosum when content files actually landed.
    // Activity-only folder segments schedule zero talents and zero model passes.
    if !written_content_files.is_empty() {
        if solstone_core_ingest_resolve::bump_stream_marker(&state.journal_root, &day).is_err() {
            return failed(
                ReasonCode::StreamMarkerBumpFailed,
                "cannot advance stream marker",
            );
        }
        let notice = IngestNotice {
            cid,
            source: &stream_source,
            day: &day,
            stream: &bound.stream,
            segment: &segment,
            files: &written_content_files,
            meta: &meta,
        };
        if let Err(error) = state.notifier.notify(&notice) {
            log::warn!("folder ingest notification failed: {error}");
            return failed(ReasonCode::NotifyFailed, "cannot notify ingest listeners");
        }
    }

    // Commit per-folder received ledger, note overlay, and folder state AFTER
    // durable segment writes.
    let now_rfc3339 = activity_timestamp((state.now_ms)());
    let received_record = ReceivedRecord {
        day: day.clone(),
        segment: segment.clone(),
        received_at: now_rfc3339.clone(),
    };
    for key in &pending_received {
        if let Err(error) = record_received(&state.journal_root, folder_id, key, &received_record) {
            log::warn!("folder received record not saved: {error}");
            return failed(
                ReasonCode::JournalWriteFailed,
                "cannot record what this folder has received",
            );
        }
    }
    for (path, note_state) in &note_overlay {
        let target = note_file(&state.journal_root, folder_id, path);
        let res = match note_state {
            Some(ns) => write_json_private(&target, ns),
            None => match std::fs::remove_file(&target) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        };
        if let Err(error) = res {
            log::warn!("folder note state not saved: {error}");
            return failed(
                ReasonCode::JournalWriteFailed,
                "cannot save folder note state",
            );
        }
    }
    let sync_stamp = latest_observed_rfc3339.unwrap_or(now_rfc3339);
    folder_state.v = 1;
    folder_state.id = folder_id.clone();
    folder_state.name = upload.meta.name.clone();
    folder_state.facet = upload.meta.facet.clone();
    folder_state.kinds = upload.meta.kinds.clone();
    folder_state
        .devices
        .insert(cid.to_owned(), sync_stamp.clone());
    folder_state.last_synced_at = Some(sync_stamp);
    folder_state.event_count = folder_state
        .event_count
        .saturating_add(new_activity_events.len() as u64);
    folder_state.renames.extend(new_renames);
    if let Some(latest) = folder_state.renames.iter().map(|r| r.at_ms).max() {
        folder_state
            .renames
            .retain(|r| latest - r.at_ms <= RENAME_MEMORY_MS);
    }
    if let Err(error) = write_json_private(&folder_state_path, &folder_state) {
        log::warn!("folder state not saved: {error}");
        return failed(ReasonCode::JournalWriteFailed, "cannot save folder state");
    }

    let written_names: Vec<String> = descriptors.iter().map(|d| d.written.clone()).collect();
    let mut body = json!({
        "status": "ok",
        "segment": segment,
        "files": written_names,
        "bytes": total_written_bytes,
        "file_descriptors": descriptors,
        "meta": meta,
    });
    if !refused_files.is_empty() {
        body["refused_files"] = serde_json::to_value(&refused_files).unwrap_or_default();
    }
    IngestCompletion::accepted(day, segment, Json(body).into_response())
}

fn written_descriptor(submitted: &str, written: &str, bytes: &[u8]) -> FileDescriptor {
    let mut extra = Map::new();
    extra.insert(
        "disposition".to_owned(),
        Value::String("written".to_owned()),
    );
    FileDescriptor {
        submitted: submitted.to_owned(),
        written: written.to_owned(),
        size: bytes.len() as u64,
        sha256: sha256_hex(bytes),
        extra,
    }
}

fn stored_event(
    line: &FolderActivityLine,
    day: &str,
    segment: &str,
    key: String,
    kind: Option<&str>,
    landed_file: Option<&str>,
) -> StoredFolderEvent {
    StoredFolderEvent {
        key,
        event: line.event,
        path: line.path.clone(),
        previous_path: line.previous_path.clone(),
        at: zoned_rfc3339(&line.at, Precision::AutoSi, false),
        at_ms: line.at_ms(),
        offset_ms: event_offset_ms(day, segment, &line.at),
        file_type: line.file_type.clone(),
        size: line.size,
        content_hash: line.content_hash.clone(),
        origin_url: line.origin_url().map(str::to_owned),
        downloaded_by: line.downloaded_by().map(str::to_owned),
        source_app: line.source_app().map(str::to_owned),
        kind: kind.map(str::to_owned),
        landed_file: landed_file.map(str::to_owned),
    }
}

fn has_primary_screenshot(seg_dir: &Path) -> bool {
    solstone_core_format::screenshots::SCREENSHOT_EXTENSIONS
        .iter()
        .any(|ext| seg_dir.join(format!("{SCREENSHOT_STEM}.{ext}")).is_file())
}

fn check_folder_image(name: &str, bytes: &[u8]) -> Result<&'static str, String> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "image could not be read".to_owned())?;
    let format = reader
        .format()
        .ok_or_else(|| "unsupported or unrecognized image format".to_owned())?;
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::WebP => "webp",
        _ => {
            return Err(
                "unsupported image format; convert to png, jpg, jpeg or webp on the device"
                    .to_owned(),
            );
        }
    };
    if let Some((_, declared_ext)) = name.rsplit_once('.') {
        let lower = declared_ext.to_ascii_lowercase();
        let expected = match lower.as_str() {
            "png" => Some(ImageFormat::Png),
            "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
            "webp" => Some(ImageFormat::WebP),
            _ => None,
        };
        if let Some(exp) = expected
            && exp != format
        {
            return Err("image bytes do not match its extension".to_owned());
        }
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| "image header could not be decoded".to_owned())?;
    if width == 0 || height == 0 {
        return Err("image has zero dimensions".to_owned());
    }
    if u64::from(width) * u64::from(height) > MAX_SCREENSHOT_PIXELS {
        return Err(format!(
            "image is {width}x{height}, over {MAX_SCREENSHOT_PIXELS} pixels"
        ));
    }
    Ok(ext)
}

fn render_pdf_markdown(path: &str, payload: &PdfPayload) -> Result<String, String> {
    if payload.encrypted {
        return Err("PDF is encrypted and could not be read".to_owned());
    }
    if payload.page_count == 0 {
        return Err("PDF has no pages".to_owned());
    }
    let title = payload
        .metadata
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(path);
    let mut out = format!(
        "# {title}\n\n**Type:** Document\n**Pages:** {}\n**Source:** {path}\n\n---\n\n",
        payload.page_count
    );
    let mut pages = payload.pages.clone();
    pages.sort_by_key(|p| p.index);
    for (idx, page) in pages.iter().enumerate() {
        if idx > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&format!("## Page {}\n\n", page.index));
        match &page.text {
            Some(text) if !text.trim().is_empty() => {
                out.push_str(text.trim_end());
                out.push('\n');
            }
            _ => {
                out.push_str("[No selectable text on this page]\n");
            }
        }
    }
    Ok(out)
}

fn load_note(
    journal: &Path,
    folder_id: &str,
    overlay: &HashMap<String, Option<NoteState>>,
    read: &mut HashMap<String, Option<NoteState>>,
    path: &str,
) -> io::Result<Option<NoteState>> {
    if let Some(held) = overlay.get(path) {
        return Ok(held.clone());
    }
    if let Some(held) = read.get(path) {
        return Ok(held.clone());
    }
    let held = read_json_optional::<NoteState>(&note_file(journal, folder_id, path))?;
    read.insert(path.to_owned(), held.clone());
    Ok(held)
}

#[allow(clippy::too_many_arguments)]
fn apply_note_removed(
    journal: &Path,
    folder_id: &str,
    path: &str,
    at_ms: i64,
    overlay: &mut HashMap<String, Option<NoteState>>,
    read: &mut HashMap<String, Option<NoteState>>,
    slots: &mut Vec<NoteSlot>,
    slot_at: &mut HashMap<String, usize>,
) -> io::Result<()> {
    if let Some(index) = slot_at.remove(path) {
        overlay.insert(path.to_owned(), None);
        let slot = &mut slots[index];
        slot.deleted = true;
        slot.vanished = slot.base.is_none();
        return Ok(());
    }
    let Some(current) = load_note(journal, folder_id, overlay, read, path)? else {
        return Ok(());
    };
    slot_at.insert(path.to_owned(), slots.len());
    slots.push(NoteSlot {
        start_path: path.to_owned(),
        path: path.to_owned(),
        base: Some(current.text),
        renamed_from: None,
        deleted: true,
        vanished: false,
        text: String::new(),
        at_ms,
    });
    overlay.insert(path.to_owned(), None);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_note_change(
    journal: &Path,
    folder_id: &str,
    line: &FolderActivityLine,
    text: String,
    hash: String,
    overlay: &mut HashMap<String, Option<NoteState>>,
    read: &mut HashMap<String, Option<NoteState>>,
    slots: &mut Vec<NoteSlot>,
    slot_at: &mut HashMap<String, usize>,
    new_renames: &mut Vec<StoredRename>,
) -> io::Result<bool> {
    let at_ms = line.at_ms();
    let new_state = NoteState {
        path: line.path.clone(),
        text: text.clone(),
        content_hash: hash.clone(),
        changed_at_ms: at_ms,
    };
    if line.event == FolderEventKind::Renamed
        && let Some(prev) = line.previous_path.as_deref()
    {
        if let Some(index) = slot_at.remove(prev) {
            let slot = &mut slots[index];
            slot.path = line.path.clone();
            if slot.base.is_some() {
                slot.renamed_from.get_or_insert_with(|| prev.to_owned());
            }
            slot.text = text;
            slot_at.insert(line.path.clone(), index);
            overlay.insert(prev.to_owned(), None);
            overlay.insert(line.path.clone(), Some(new_state));
            new_renames.push(StoredRename {
                from: prev.to_owned(),
                to: line.path.clone(),
                at_ms,
            });
            return Ok(true);
        }
        if let Some(held) = load_note(journal, folder_id, overlay, read, prev)? {
            if at_ms < held.changed_at_ms {
                return Ok(false);
            }
            slot_at.insert(line.path.clone(), slots.len());
            slots.push(NoteSlot {
                start_path: prev.to_owned(),
                path: line.path.clone(),
                base: Some(held.text),
                renamed_from: Some(prev.to_owned()),
                deleted: false,
                vanished: false,
                text,
                at_ms,
            });
            overlay.insert(prev.to_owned(), None);
            overlay.insert(line.path.clone(), Some(new_state));
            new_renames.push(StoredRename {
                from: prev.to_owned(),
                to: line.path.clone(),
                at_ms,
            });
            return Ok(true);
        }
    }

    if let Some(&index) = slot_at.get(&line.path) {
        slots[index].text = text;
        overlay.insert(line.path.clone(), Some(new_state));
        return Ok(true);
    }
    let current = load_note(journal, folder_id, overlay, read, &line.path)?;
    match &current {
        Some(held) if held.content_hash == hash => return Ok(false),
        Some(held) if at_ms < held.changed_at_ms => return Ok(false),
        _ => {}
    }
    slot_at.insert(line.path.clone(), slots.len());
    slots.push(NoteSlot {
        start_path: line.path.clone(),
        path: line.path.clone(),
        base: current.map(|held| held.text),
        renamed_from: None,
        deleted: false,
        vanished: false,
        text,
        at_ms,
    });
    overlay.insert(line.path.clone(), Some(new_state));
    Ok(true)
}

fn renames_near(renames: &[StoredRename], at_ms: i64) -> Vec<KnownRename> {
    renames
        .iter()
        .filter(|rename| (at_ms - rename.at_ms).abs() <= RENAME_MEMORY_MS)
        .map(|rename| KnownRename {
            from: rename.from.clone(),
            to: rename.to.clone(),
        })
        .collect()
}

fn note_entry_for(slot: &NoteSlot, renames: &[KnownRename]) -> Option<NoteEntry<Document>> {
    if slot.deleted {
        if slot.vanished {
            return None;
        }
        return Some(NoteEntry {
            path: slot.start_path.clone(),
            status: Some(note_status(Message::NoteRemoved)),
            body: Document::default(),
        });
    }
    let Some(base) = &slot.base else {
        return (!slot.text.trim().is_empty()).then(|| NoteEntry {
            path: slot.path.clone(),
            status: None,
            body: Document::plain(slot.text.clone()),
        });
    };
    if let Some(from) = &slot.renamed_from {
        return Some(NoteEntry {
            path: slot.path.clone(),
            status: Some(note_status(Message::NoteRenamed { path: from.clone() })),
            body: render_edit_document(base, &slot.text).unwrap_or_default(),
        });
    }
    if only_link_rewrites(base, &slot.text, renames) {
        return None;
    }
    render_edit_document(base, &slot.text).map(|body| NoteEntry {
        path: slot.path.clone(),
        status: Some(note_status(Message::NoteEdited)),
        body,
    })
}

fn note_status(message: Message) -> Document {
    let mut document = Document::default();
    document.push_message(message);
    document
}

/// Published supported-file list for `GET /app/devices/ingest/folder/supported`.
#[must_use]
pub fn supported_payload() -> Value {
    json!({
        "version": 1,
        "ruleset_version": triage::RULESET_VERSION,
        "window_seconds": FOLDER_WINDOW_SECONDS,
        "max_file_bytes": solstone_core_ingest_contract::MAX_PART_BYTES,
        "max_note_bytes": MAX_NOTE_BYTES,
        "max_image_pixels": MAX_SCREENSHOT_PIXELS,
        "document_revision_cooldown_seconds": DOCUMENT_REVISION_COOLDOWN_SECONDS,
        "kinds": {
            "notes": {
                "extensions": ["md", "markdown", "txt", "log"],
                "mode": "first_sight_and_net_change"
            },
            "documents": {
                "extensions": ["pdf", "md", "markdown", "txt", "log"],
                "mode": "each_revision",
                "thresholds": {
                    "structured_prose_min_words": 1500,
                    "structured_prose_min_heading_levels": 2,
                    "long_prose_min_words": 5000
                }
            },
            "transcripts": {
                "extensions": ["vtt", "srt", "md", "markdown", "txt", "log", "json", "csv", "tsv", "html"],
                "mode": "each_revision"
            },
            "screenshots": {
                "extensions": ["png", "jpg", "jpeg", "webp"],
                "convert_to_png_on_device": ["heic", "heif", "tif", "tiff", "gif", "bmp"],
                "mode": "once"
            },
            "photos": {
                "extensions": ["png", "jpg", "jpeg", "webp"],
                "convert_to_png_on_device": ["heic", "heif", "tif", "tiff", "gif", "bmp"],
                "mode": "once"
            },
            "other": {
                "extensions": [],
                "mode": "metadata_only"
            }
        },
        "activity_only": {
            "model_passes": 0
        }
    })
}

/// `GET /app/devices/ingest/folder/supported`
pub(crate) async fn folder_supported(
    Extension(basis): Extension<AccessBasis>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, status, detail)) = validate_access(&basis) {
        return refusal(code, status, detail);
    }
    if let Err((code, status, detail)) = validate_protocol(&headers) {
        return refusal(code, status, detail);
    }
    Json(supported_payload()).into_response()
}

/// `GET /app/devices/ingest/folder/facets`
pub(crate) async fn folder_facets(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
) -> Response {
    if let Err((code, status, detail)) = validate_access(&basis) {
        return refusal(code, status, detail);
    }
    if let Err((code, status, detail)) = validate_protocol(&headers) {
        return refusal(code, status, detail);
    }
    spawn_blocking_response(
        OwnerReadRole::DeviceIngestSegments,
        move || match read_journal_facets(&state.journal_root) {
            Ok(facets) => {
                let enabled: Vec<FacetItem> = facets.into_iter().filter(|f| !f.muted).collect();
                Json(json!({ "facets": enabled })).into_response()
            }
            Err(error) => {
                log::warn!("folder facets list unreadable: {error}");
                refusal(
                    ReasonCode::JournalReadFailed,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cannot read facets",
                )
            }
        },
    )
    .await
}

#[derive(Debug, Deserialize)]
struct EstimateRequest {
    #[serde(default)]
    folder_id: Option<String>,
    items: Vec<EstimateItem>,
}

#[derive(Debug, Deserialize)]
struct EstimateItem {
    #[serde(default)]
    path: Option<String>,
    at: String,
    #[serde(default)]
    file_type: Option<String>,
    #[serde(default)]
    kind_hint: Option<String>,
    #[serde(default)]
    content_hash: Option<String>,
    #[serde(default)]
    has_content: bool,
}

/// `POST /app/devices/ingest/folder/estimate`
///
/// Compute an honest backfill preview estimate in solstone units (`items`,
/// `segments`, `days`, `thoughts`, never money). Activity-only windows cost
/// zero thoughts; content windows count `sense`/`sense_static` plus vision
/// passes for screenshots and photos.
pub(crate) async fn folder_estimate(
    Extension(basis): Extension<AccessBasis>,
    State(state): State<IngestState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err((code, status, detail)) = validate_access(&basis) {
        return refusal(code, status, detail);
    }
    if let Err((code, status, detail)) = validate_protocol(&headers) {
        return refusal(code, status, detail);
    }
    let request: EstimateRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(_) => {
            return refusal(
                ReasonCode::FolderInvalid,
                StatusCode::BAD_REQUEST,
                "estimate body must be valid JSON with an items array",
            );
        }
    };
    if request.items.len() > MAX_ESTIMATE_ITEMS {
        return refusal(
            ReasonCode::FolderInvalid,
            StatusCode::BAD_REQUEST,
            format!("estimate accepts at most {MAX_ESTIMATE_ITEMS} items"),
        );
    }
    if let Some(id) = &request.folder_id
        && !is_valid_folder_id(id)
    {
        return refusal(
            ReasonCode::FolderInvalid,
            StatusCode::BAD_REQUEST,
            "folder_id is invalid",
        );
    }
    spawn_blocking_response(
        OwnerReadRole::DeviceIngestSegments,
        move || match compute_estimate(&state.journal_root, request) {
            Ok(value) => Json(value).into_response(),
            Err((code, status, detail)) => refusal(code, status, detail),
        },
    )
    .await
}

fn compute_estimate(
    journal: &Path,
    request: EstimateRequest,
) -> Result<Value, (ReasonCode, StatusCode, String)> {
    let mut items = 0usize;
    let mut content_items = 0usize;
    let mut activity_only_items = 0usize;
    let mut already_in_journal = 0usize;
    let mut all_segments: BTreeSet<(String, String)> = BTreeSet::new();
    let mut content_segments: BTreeSet<(String, String)> = BTreeSet::new();
    let mut days: BTreeSet<String> = BTreeSet::new();
    let mut vision_passes: usize = 0;

    let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::from([
        ("notes", 0),
        ("documents", 0),
        ("transcripts", 0),
        ("screenshots", 0),
        ("photos", 0),
        ("activity_only", 0),
    ]);

    for (idx, item) in request.items.iter().enumerate() {
        let at = solstone_core_time::parse_rfc3339(&item.at).map_err(|_| {
            (
                ReasonCode::FolderInvalid,
                StatusCode::BAD_REQUEST,
                format!("items[{idx}].at must be RFC 3339 with an offset"),
            )
        })?;
        let kind_bucket = classify_estimate_kind(item);

        // If folder_id and content_hash were provided, check whether this item
        // is already recorded in the folder's received ledger.
        if let (Some(folder_id), Some(hash)) =
            (request.folder_id.as_deref(), item.content_hash.as_deref())
        {
            let check_key = match kind_bucket {
                "screenshots" | "photos" => Some(once_content_key(kind_bucket, hash)),
                "documents" | "transcripts" => item
                    .path
                    .as_deref()
                    .map(|p| revision_content_key(kind_bucket, p, hash)),
                _ => None,
            };
            if let Some(key) = check_key {
                match lookup_received(journal, folder_id, &key) {
                    Ok(Some(_)) => {
                        already_in_journal += 1;
                        continue;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        return Err((
                            ReasonCode::JournalReadFailed,
                            StatusCode::SERVICE_UNAVAILABLE,
                            "cannot read what this folder has received; try again".to_owned(),
                        ));
                    }
                }
            }
        }

        let local = at.datetime();
        let day = solstone_core_time::day_key(local.date());
        let secs =
            (local.hour() as u32) * 3600 + (local.minute() as u32) * 60 + (local.second() as u32);
        let slot = (secs / FOLDER_WINDOW_SECONDS) * FOLDER_WINDOW_SECONDS;
        let seg = format!(
            "{:02}{:02}{:02}_{FOLDER_WINDOW_SECONDS}",
            slot / 3600,
            (slot % 3600) / 60,
            slot % 60
        );

        items += 1;
        days.insert(day.clone());
        all_segments.insert((day.clone(), seg.clone()));
        *by_kind.entry(kind_bucket).or_insert(0) += 1;

        if kind_bucket == "activity_only" {
            activity_only_items += 1;
        } else {
            content_items += 1;
            content_segments.insert((day, seg));
            if matches!(kind_bucket, "screenshots" | "photos") {
                vision_passes += 1;
            }
        }
    }

    let total_segments = all_segments.len();
    let c_segments = content_segments.len();
    let activity_only_segments = total_segments.saturating_sub(c_segments);
    // ~1.1 thoughts per content segment (`sense`/`sense_static` + daily rollup)
    // plus one vision pass per screenshot or photo; zero for activity-only.
    let sense_thoughts = if c_segments == 0 {
        0
    } else {
        (c_segments * 11 + 5) / 10
    };
    let thoughts = sense_thoughts + vision_passes;

    let backfill_paused = solstone_core_journal_config::read_journal_config(journal)
        .ok()
        .and_then(|read| {
            read.config
                .as_ref()
                .and_then(|cfg| cfg.get("processing"))
                .and_then(|p| p.get("backfill_paused"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);

    Ok(json!({
        "items": items,
        "content_items": content_items,
        "activity_only_items": activity_only_items,
        "already_in_journal": already_in_journal,
        "segments": total_segments,
        "content_segments": c_segments,
        "activity_only_segments": activity_only_segments,
        "days": days.len(),
        "thoughts": thoughts,
        "counts_by_kind": by_kind,
        "backfill_paused": backfill_paused,
    }))
}

fn classify_estimate_kind(item: &EstimateItem) -> &'static str {
    if !item.has_content {
        return "activity_only";
    }
    if let Some(hint) = item.kind_hint.as_deref() {
        match hint {
            "notes" => return "notes",
            "documents" => return "documents",
            "transcripts" => return "transcripts",
            "screenshots" | "screen_text" => return "screenshots",
            "photos" => return "photos",
            _ => {}
        }
    }
    let ext = item
        .file_type
        .as_deref()
        .or_else(|| {
            item.path
                .as_deref()
                .and_then(|p| Path::new(p).extension().and_then(|e| e.to_str()))
        })
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "documents",
        "md" | "markdown" | "txt" | "log" => "notes",
        "png" | "jpg" | "jpeg" | "webp" | "heic" | "heif" | "tif" | "tiff" | "gif" | "bmp" => {
            "photos"
        }
        "vtt" | "srt" | "json" | "csv" | "tsv" | "html" => "transcripts",
        _ => "activity_only",
    }
}

/// Synced folders grouped by device `cid` → folder `name` → `last_synced_at`,
/// used by `known_sources` so `/app/network/api/sources` reflects watched
/// folders alongside Obsidian vaults.
pub(crate) fn synced_folders(
    journal: &Path,
) -> Result<BTreeMap<String, BTreeMap<String, String>>, String> {
    let root = journal.join(STATE_DIR);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut folders = BTreeMap::<String, BTreeMap<String, String>>::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        let Some(held) = read_json_optional::<FolderState>(&entry.path().join("folder.json"))
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        for (device, last_synced_at) in &held.devices {
            if last_synced_at.parse::<jiff::Timestamp>().is_ok() {
                folders
                    .entry(device.clone())
                    .or_default()
                    .insert(held.name.clone(), last_synced_at.clone());
            }
        }
    }
    Ok(folders)
}

/// List all persisted watched folders on this journal, sorted by folder name.
pub fn list_synced_folders(journal: &Path) -> io::Result<Vec<FolderState>> {
    let root = journal.join(STATE_DIR);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut rows = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        if let Some(held) = read_json_optional::<FolderState>(&entry.path().join("folder.json"))? {
            rows.push(held);
        }
    }
    rows.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(rows)
}

#[cfg(test)]
#[path = "folder_tests.rs"]
mod tests;
