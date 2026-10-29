// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Json, Multipart, State},
    http::StatusCode,
    response::Response,
};
use chrono::Utc;
use chrono_tz::Tz;
use serde_json::{Map, Value, json};
use solstone_core_import::{
    ImportError, ImportMetadata, ManifestMatch, SourceHash, allocate_import_id,
    find_manifest_by_hash_where, hash_source, read_import_metadata, relocate_import,
    validate_timestamp, write_import_metadata,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, atomic_replace, contained_path, create_directory_with_mode, install_file,
};
use solstone_core_processing_record::MediaKind;
use tempfile::NamedTempFile;

use crate::{
    AppState,
    callosum::{BusError, TaskWatch, request_required},
    http::{error, import_not_found, json as json_response},
    mobile_import_contract::{
        ImportErrorKind, ImportReasonCode, SavedSource, inprocess_start_response,
        queued_start_response,
    },
    multipart, save_stream,
};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_millis() as i64
}

fn import_timestamp(zone: Tz) -> String {
    Utc::now()
        .with_timezone(&zone)
        .format("%Y%m%d_%H%M%S")
        .to_string()
}

fn deterministic_timestamp(path: &Path, filename: &str, zone: Tz) -> Option<String> {
    solstone_core_import_sources::image::detected_creation_stamp(path, Some(filename), zone)
        .map(|ts| ts.as_str().to_owned())
}

/// The upload's timestamp and how it was found. Saving consults no model: a file with no
/// deterministic timestamp takes the upload time, so its no-match reason names only the
/// deterministic sources and its recorded diagnostics never claim a model was called.
fn timestamp_for_upload(
    path: &Path,
    filename: &str,
    zone: Tz,
) -> (String, &'static str, Option<&'static str>) {
    // A PDF's worker chooses its dates after inspecting the document. A filename alone
    // cannot promise an override before it has been compared with creation metadata.
    if Path::new(filename)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return (import_timestamp(zone), "document_dates", None);
    }
    if let Some(timestamp) = deterministic_timestamp(path, filename, zone) {
        return (timestamp, "deterministic", None);
    }
    (
        import_timestamp(zone),
        "upload_fallback",
        Some("no_deterministic_match"),
    )
}

/// A trimmed, non-empty string, or nothing. Any other JSON value is absent: rendering a
/// stored `null` as text turned a quick import's missing hint into `--source null`.
fn clean_optional(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// The stored importer source hint. Earlier builds stored an absent hint as the literal
/// string "null", which names no importer source, so it reads as absent too.
/// The import card that brings in a Strava download.
const STRAVA_SOURCE: &str = "strava";

/// How long a Strava upload may wait to be started before it is removed.
const ABANDONED_STRAVA_UPLOAD: Duration = Duration::from_secs(60 * 60);

/// Remove Strava downloads uploaded on the Strava card and never imported.
///
/// The card promises the download isn't kept. A run removes its upload when it
/// ends; an upload whose import was never started (the owner left the card) is
/// removed here once it has waited an hour. Best effort: a failure leaves it for
/// the next start.
pub(crate) fn sweep_abandoned_strava_uploads(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root.join("imports")) else {
        return;
    };
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if id.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(metadata) = read_import_metadata(root, &id) else {
            continue;
        };
        if source_hint(&metadata).as_deref() != Some(STRAVA_SOURCE) {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= ABANDONED_STRAVA_UPLOAD);
        if old_enough {
            if solstone_core_import::project_import_result(root, &id).status
                == solstone_core_import::ProjectionStatus::Pending
            {
                let _ = solstone_core_import::remove_import_records(root, &id);
            } else {
                discard_terminal_strava_download(root, &id);
            }
        }
    }
}

/// Preflight can fail before the source producer runs its own cleanup. Keep the
/// terminal record, but never retain its uploaded Strava download or snapshots.
fn discard_terminal_strava_download(root: &Path, id: &str) {
    use solstone_core_import::ProjectionStatus;
    if matches!(
        solstone_core_import::project_import_result(root, id).status,
        ProjectionStatus::Pending | ProjectionStatus::Running
    ) {
        return;
    }
    if read_import_metadata(root, id)
        .is_ok_and(|metadata| source_hint(&metadata).as_deref() == Some(STRAVA_SOURCE))
        && let Err(error) = solstone_core_import::snapshot::discard_download(root, id)
    {
        log::warn!("could not remove the Strava download for {id}: {error}");
    }
}

pub(crate) fn source_hint(metadata: &ImportMetadata) -> Option<String> {
    clean_optional(metadata.get("source_hint")).filter(|hint| hint != "null")
}

fn text_value(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn client_bag(value: Option<&Value>) -> Value {
    value
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// The source a saved file is recorded under. A video file is recorded as audio because that
/// is how it imports: the importer keeps its audio track and transcribes it. Recording it as
/// text hid its imported content, which the content list finds through this field.
fn source_for(filename: &str, content_type: Option<&str>) -> &'static str {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let source = if solstone_core_import_sources::image::is_image_extension(&extension) {
        SavedSource::Image
    } else if matches!(extension.as_str(), "pdf" | "doc" | "docx") {
        SavedSource::Document
    } else if content_type
        .is_some_and(|value| value.starts_with("audio/") || value.starts_with("video/"))
        || matches!(
            solstone_core_processing_record::media_kind(&extension),
            Some(MediaKind::Audio | MediaKind::Video)
        )
    {
        SavedSource::Audio
    } else {
        SavedSource::Text
    };
    source.as_wire()
}

fn safe_filename(filename: &str) -> Option<String> {
    Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && *value != "." && *value != "..")
        .map(ToOwned::to_owned)
}

fn failure(
    code: ImportReasonCode,
    message: &str,
    detail: impl Into<String>,
    kind: Option<ImportErrorKind>,
) -> Response {
    error(code.status(), message, code.as_wire(), detail.into(), kind)
}

fn metadata_failed(detail: impl Into<String>) -> Response {
    failure(
        ImportReasonCode::ImportMetadataFailed,
        "that import metadata couldn't be updated.",
        detail,
        None,
    )
}

fn missing(detail: impl Into<String>) -> Response {
    failure(
        ImportReasonCode::MissingRequiredField,
        "a required field is missing.",
        detail,
        None,
    )
}

pub(crate) fn invalid_state_body(
    detail: impl Into<String>,
    kind: Option<ImportErrorKind>,
) -> Value {
    crate::http::error_body(
        "that action isn't available in the current state.",
        ImportReasonCode::InvalidOperationForState.as_wire(),
        detail.into(),
        kind,
    )
}

fn invalid_state(detail: impl Into<String>, kind: Option<ImportErrorKind>) -> Response {
    failure(
        ImportReasonCode::InvalidOperationForState,
        "that action isn't available in the current state.",
        detail,
        kind,
    )
}

pub(crate) fn resolve_import_id(root: &Path, data: &Value) -> Option<String> {
    let path = text_value(data, "path");
    let timestamp = text_value(data, "timestamp");
    if path.is_empty() || timestamp.is_empty() {
        return None;
    }
    if let Some(id) = data
        .get("import_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(id.to_owned());
    }
    let imports = root.join("imports");
    let file_path = Path::new(&path);
    let is_local_path = !file_path.starts_with(&imports);
    if is_local_path {
        Some(timestamp)
    } else {
        file_path
            .parent()
            .and_then(|value| value.file_name())
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned)
            .or(Some(timestamp))
    }
}

pub(crate) type TaskIdHook = dyn Fn(&TaskIdOp) -> Result<(), ImportError> + Send + Sync;

#[derive(Debug, Clone)]
pub(crate) enum TaskIdOp {
    Record {
        new_id: String,
    },
    Restore {
        recorded_id: String,
        previous: Option<String>,
    },
}

fn exchange_task_id(
    root: &Path,
    import_id: &str,
    op: &TaskIdOp,
    hook: Option<&TaskIdHook>,
) -> Result<(), ImportError> {
    if let Some(hook) = hook {
        hook(op)?;
    }
    match op {
        TaskIdOp::Record { new_id } => {
            solstone_core_import::record_queued_task(root, import_id, new_id).map(|_| ())
        }
        TaskIdOp::Restore {
            recorded_id,
            previous,
        } => {
            let _guard = solstone_core_import::hold_import_lock(root, import_id)?;
            let current = solstone_core_import::read_import_metadata(root, import_id).ok();
            let current_task_id = current
                .as_ref()
                .and_then(|m| m.get("task_id"))
                .and_then(Value::as_str);
            if current_task_id == Some(recorded_id.as_str()) {
                let mut results = serde_json::Map::new();
                results.insert(
                    "task_id".to_owned(),
                    match previous {
                        Some(prev) => Value::String(prev.clone()),
                        None => Value::Null,
                    },
                );
                solstone_core_import::record_import_results_unlocked(root, import_id, results)?;
            }
            Ok(())
        }
    }
}

fn manifest_exists(root: &Path, hash: &SourceHash) -> bool {
    find_manifest_by_hash_where(root, hash, |found| !is_failed_original_import(root, found))
        .ok()
        .and_then(|scan| scan.found)
        .is_some()
}

/// A failed image or document import does not make its bytes "already imported".
///
/// Its manifest is written before publication, so a failure leaves it behind.
/// A retry claims the segment that already holds the same original and
/// finishes publishing it, so it adds no second copy. The import is read by its
/// directory, because a started import can be relocated without its manifest's
/// `import_id` changing.
fn is_failed_original_import(root: &Path, found: &ManifestMatch) -> bool {
    if !matches!(
        found.manifest.get("source_type").and_then(Value::as_str),
        Some("image" | "document")
    ) {
        return false;
    }
    let Some(directory) = found
        .path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    solstone_core_import::project_import_result(root, directory).status
        == solstone_core_import::ProjectionStatus::Failed
}

fn import_is_running_or_successful(
    root: &Path,
    timestamp: &str,
    _metadata: &ImportMetadata,
) -> bool {
    let proj = solstone_core_import::project_import_result(root, timestamp);
    matches!(
        proj.status,
        solstone_core_import::ProjectionStatus::Running
            | solstone_core_import::ProjectionStatus::Success
    )
}

fn recorded_timestamp(metadata: &ImportMetadata, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| validate_timestamp(value).is_ok())
        .map(ToOwned::to_owned)
}

/// Directory id the start request must address. A historical row stored only
/// `user_timestamp`, which was that directory.
fn recorded_directory_id(metadata: &ImportMetadata) -> Value {
    recorded_timestamp(metadata, "import_id")
        .or_else(|| recorded_timestamp(metadata, "user_timestamp"))
        .map_or(Value::String(String::new()), Value::String)
}

fn recorded_source_timestamp(metadata: &ImportMetadata) -> Value {
    recorded_timestamp(metadata, "source_timestamp")
        .or_else(|| recorded_timestamp(metadata, "user_timestamp"))
        .map_or(Value::String(String::new()), Value::String)
}

/// An explicit choice is independent of the collision-safe staging directory.
pub(crate) fn owner_timestamp_choice(data: &Value, metadata: &ImportMetadata) -> Option<String> {
    let timestamp = data.get("timestamp")?.as_str()?;
    let source = recorded_source_timestamp(metadata);
    (data
        .get("timestamp_override")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || source
            .as_str()
            .filter(|source| !source.is_empty())
            .is_some_and(|source| source != timestamp))
    .then(|| timestamp.to_owned())
}

fn resolved_summary(mut response: Value, path: &Path, filename: Option<&str>, zone: Tz) -> Value {
    response["resolved_timestamp"] = solstone_core_import_sources::image::detected_creation_stamp(
        path,
        filename.or_else(|| path.file_name().and_then(|name| name.to_str())),
        zone,
    )
    .map_or(Value::Null, |stamp| json!(stamp.as_str()));
    response
}

fn claim_import_directory(root: &Path, semantic: &str) -> Result<String, String> {
    let source = validate_timestamp(semantic).map_err(|error| error.to_string())?;
    let allocated = allocate_import_id(root, &source).map_err(|error| error.to_string())?;
    Ok(allocated.import_id.as_str().to_owned())
}

pub(crate) fn summary(metadata: &ImportMetadata) -> Value {
    json!({
        "schema_version": 1,
        "status": "staged",
        "replay": false,
        "path": metadata.get("file_path").cloned().unwrap_or(Value::String(String::new())),
        "timestamp": recorded_directory_id(metadata),
        "source_timestamp": recorded_source_timestamp(metadata),
        "client_item_id": metadata.get("client_item_id").cloned().unwrap_or(Value::String(String::new())),
        "source": metadata.get("source").cloned().unwrap_or_else(|| json!("text")),
        "setting": metadata.get("setting").cloned().unwrap_or(Value::Null),
        "recommended_action": "start",
        "metadata": {
            "original_filename": metadata.get("original_filename").cloned().unwrap_or(Value::Null),
            "mime_type": metadata.get("mime_type").cloned().unwrap_or(Value::Null),
            "imported_via": metadata.get("imported_via").cloned().unwrap_or(Value::Null),
            "observer_handle": metadata.get("observer_handle").cloned().unwrap_or(Value::Null),
            "source_hint": metadata.get("source_hint").cloned().unwrap_or(Value::Null),
            "client": metadata.get("client").cloned().unwrap_or_else(|| json!({})),
        },
        "diagnostics": {
            "timestamp_detection_method": metadata.get("timestamp_detection_method").cloned().unwrap_or_else(|| json!("upload_fallback")),
            "timestamp_detection_model_called": metadata.get("timestamp_detection_model_called").cloned().unwrap_or_else(|| json!(false)),
            "timestamp_detection_no_match_reason": metadata.get("timestamp_detection_no_match_reason").cloned().unwrap_or(Value::Null),
            "source_inference": metadata.get("source_inference").cloned().unwrap_or_else(|| json!("default")),
        },
    })
}

pub(crate) fn replay_summary(metadata: &ImportMetadata) -> Value {
    let mut result = summary(metadata);
    result["replay"] = json!(true);
    result["recommended_action"] = json!("start");
    result
}

fn staged_by_client_item(root: &Path, client_item_id: &str) -> Option<ImportMetadata> {
    std::fs::read_dir(root.join("imports"))
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .find_map(|timestamp| {
            let metadata = read_import_metadata(root, &timestamp).ok()?;
            (metadata.get("client_item_id").and_then(Value::as_str) == Some(client_item_id))
                .then_some(metadata)
        })
}

fn staged_by_source_hash(root: &Path, source_hash: &SourceHash) -> Option<ImportMetadata> {
    std::fs::read_dir(root.join("imports"))
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .find_map(|timestamp| {
            let metadata = read_import_metadata(root, &timestamp).ok()?;
            (metadata.get("source_hash").and_then(Value::as_str) == Some(source_hash.as_str()))
                .then_some(metadata)
        })
}

fn stage_bytes(
    root: &Path,
    timestamp: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<PathBuf, ImportError> {
    let target = contained_import_file(root, timestamp, filename)?;
    let directory = target.parent().expect("import target parent").to_owned();
    create_directory_with_mode(&directory, 0o700).map_err(|error| ImportError::PathResolution {
        path: directory.clone(),
        message: error.to_string(),
    })?;
    atomic_replace(&target, bytes, AtomicWriteOptions { mode: Some(0o600) }).map_err(|error| {
        ImportError::PromotionFailed {
            path: target.clone(),
            message: error.to_string(),
        }
    })?;
    Ok(target)
}

pub(crate) fn contained_import_file(
    root: &Path,
    timestamp: &str,
    filename: &str,
) -> Result<PathBuf, ImportError> {
    let relative = format!("imports/{timestamp}/{filename}");
    contained_path(root, &relative).map_err(|error| ImportError::PathResolution {
        path: root.join(relative),
        message: error.to_string(),
    })
}

pub(crate) const SAVE_TMP_DIR: &str = ".save-tmp";

fn save_tmp_dir(root: &Path) -> PathBuf {
    root.join("imports").join(SAVE_TMP_DIR)
}

fn ensure_save_tmp(root: &Path) -> Result<PathBuf, String> {
    let directory = save_tmp_dir(root);
    create_directory_with_mode(&directory, 0o700).map_err(|error| error.to_string())?;
    Ok(directory)
}

fn payload_too_large(detail: impl Into<String>) -> Response {
    failure(
        ImportReasonCode::MultipartPartTooLarge,
        "that file is too large to bring in.",
        detail,
        None,
    )
}

fn stream_error_response(error: save_stream::SaveStreamError) -> Response {
    match error {
        save_stream::SaveStreamError::CeilingExceeded => {
            payload_too_large("multipart file exceeds 4 GiB")
        }
        save_stream::SaveStreamError::LengthLimited => {
            payload_too_large("multipart file exceeds request limit")
        }
        save_stream::SaveStreamError::Read => failure(
            ImportReasonCode::IngestNoFiles,
            "there are no files to bring in.",
            "cannot read multipart part",
            None,
        ),
        save_stream::SaveStreamError::Io(error) => {
            metadata_failed(format!("Failed to stage temporary file: {error}"))
        }
    }
}

struct UploadedFile {
    filename: Option<String>,
    content_type: Option<String>,
    counted: save_stream::CountedTemp,
}

struct StagedMetadata<'a> {
    timestamp: &'a str,
    original_filename: &'a str,
    file_path: String,
    source_hash: SourceHash,
    source: &'a str,
    mime_type: Option<String>,
    client_item_id: String,
    data: &'a Value,
    is_local_path: bool,
    method: &'a str,
    zone: Tz,
}

fn staged_metadata(input: StagedMetadata<'_>) -> ImportMetadata {
    let StagedMetadata {
        timestamp,
        original_filename,
        file_path,
        source_hash,
        source,
        mime_type,
        client_item_id,
        data,
        is_local_path,
        method,
        zone,
    } = input;
    let upload_timestamp = now_ms();
    let imported_via = text_value(data, "imported_via");
    let imported_via = if imported_via.is_empty() {
        "web_dashboard".to_owned()
    } else {
        imported_via
    };
    Map::from_iter([
        ("original_filename".to_owned(), json!(original_filename)),
        ("upload_timestamp".to_owned(), json!(upload_timestamp)),
        (
            "upload_datetime".to_owned(),
            json!(Utc::now().with_timezone(&zone).naive_local().to_string()),
        ),
        ("user_timestamp".to_owned(), json!(timestamp)),
        ("timestamp_detection_method".to_owned(), json!(method)),
        ("timestamp_detection_model_called".to_owned(), json!(false)),
        (
            "timestamp_detection_no_match_reason".to_owned(),
            Value::Null,
        ),
        ("source_inference".to_owned(), json!("default")),
        ("file_size".to_owned(), Value::Null),
        (
            "mime_type".to_owned(),
            mime_type.map_or(Value::Null, Value::String),
        ),
        (
            "setting".to_owned(),
            clean_optional(data.get("setting")).map_or(Value::Null, Value::String),
        ),
        ("file_path".to_owned(), json!(file_path)),
        ("is_local_path".to_owned(), json!(is_local_path)),
        ("imported_via".to_owned(), json!(imported_via)),
        ("link_id".to_owned(), Value::Null),
        (
            "observer_handle".to_owned(),
            clean_optional(data.get("observer_handle")).map_or(Value::Null, Value::String),
        ),
        ("client_item_id".to_owned(), json!(client_item_id)),
        ("source_hash".to_owned(), json!(source_hash.into_inner())),
        ("source".to_owned(), json!(source)),
        (
            "source_hint".to_owned(),
            clean_optional(data.get("source_hint")).map_or(Value::Null, Value::String),
        ),
        ("client".to_owned(), client_bag(data.get("client"))),
    ])
}

/// Register an already copied native folder using the maintained staging record.
/// The native worker retains its original preparation through this call and
/// temporary-copy cleanup. An error preserves the admitted folder and its id.
#[cfg(unix)]
pub fn register_admitted_folder(
    root: &solstone_core_journal_io::JournalRoot,
    folder: &solstone_core_import::AdmittedFolder,
    source: &str,
    original_filename: &str,
    client_item_id: &str,
    source_timestamp: &solstone_core_import::Timestamp,
    zone: Tz,
) -> Result<Value, solstone_core_import::FolderAdmitError> {
    use solstone_core_import::FolderAdmitError;
    // These are the supported picked-folder routes. Body setup remains held.
    if !matches!(source, "obsidian" | "gemini" | "document" | "audio_archive")
        || client_item_id.is_empty()
    {
        return Err(FolderAdmitError::Denied);
    }
    let path = root
        .canonical_path()
        .join("imports")
        .join(folder.import_id.as_str())
        .join("tree");
    let data = json!({"source_hint":source,"imported_via":"mobile"});
    let mut metadata = staged_metadata(StagedMetadata {
        timestamp: source_timestamp.as_str(),
        original_filename,
        file_path: path.to_str().ok_or(FolderAdmitError::Denied)?.to_owned(),
        source_hash: SourceHash::new(folder.legacy_hash.clone()),
        source,
        mime_type: None,
        client_item_id: client_item_id.to_owned(),
        data: &data,
        is_local_path: false,
        method: "path_fallback",
        zone,
    });
    metadata.insert("import_id".into(), json!(folder.import_id.as_str()));
    metadata.insert("source_timestamp".into(), json!(source_timestamp.as_str()));
    metadata.insert("folder_snapshot".into(), json!(folder.snapshot));
    solstone_core_import::saved_start::initialize_saved_start(
        &mut metadata,
        folder.import_id.as_str(),
    )
    .map_err(|_| FolderAdmitError::Denied)?;
    folder.write_metadata_bound(root, &metadata)?;
    Ok(summary(&metadata))
}

pub(crate) async fn save(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut fields = BTreeMap::new();
    let mut upload = None;
    let mut part_count = 0;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) if save_stream::is_length_limit(&error) => {
                return payload_too_large("multipart file exceeds request limit");
            }
            Err(_) => {
                return failure(
                    ImportReasonCode::IngestNoFiles,
                    "there are no files to bring in.",
                    "invalid multipart body",
                    None,
                );
            }
        };
        if part_count == multipart::MAX_PARTS {
            return failure(
                ImportReasonCode::IngestNoFiles,
                "there are no files to bring in.",
                "too many multipart parts",
                None,
            );
        }
        part_count += 1;
        if field.headers().len() > multipart::MAX_HEADERS {
            return failure(
                ImportReasonCode::IngestNoFiles,
                "there are no files to bring in.",
                "too many multipart headers",
                None,
            );
        }
        let filename = field.file_name().map(ToOwned::to_owned);
        if filename
            .as_ref()
            .is_some_and(|value| value.len() > multipart::MAX_FILENAME_BYTES)
        {
            return failure(
                ImportReasonCode::IngestNoFiles,
                "there are no files to bring in.",
                "multipart filename is too long",
                None,
            );
        }
        let name = field.name().unwrap_or_default().to_owned();
        let content_type = field.content_type().map(ToOwned::to_owned);
        if name == "file" {
            let directory = match ensure_save_tmp(&state.root) {
                Ok(directory) => directory,
                Err(error) => return metadata_failed(error),
            };
            match save_stream::stream_field(field, &directory, multipart::MAX_SAVE_FILE_BYTES).await
            {
                Ok(counted) => {
                    upload = Some(UploadedFile {
                        filename,
                        content_type,
                        counted,
                    });
                }
                Err(error) => return stream_error_response(error),
            }
        } else {
            match multipart::bounded(field).await {
                Ok(bytes) => {
                    if let Ok(value) = String::from_utf8(bytes) {
                        fields.insert(name, value);
                    }
                }
                Err(detail) => {
                    return failure(
                        ImportReasonCode::IngestNoFiles,
                        "there are no files to bring in.",
                        detail,
                        None,
                    );
                }
            }
        }
    }
    let data = Value::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    );
    let client_item_id = text_value(&data, "client_item_id");
    if client_item_id.is_empty() {
        return missing("Missing client_item_id");
    }
    let text = text_value(&data, "text");
    enum Incoming {
        File {
            filename: String,
            original_filename: String,
            mime_type: Option<String>,
            counted: save_stream::CountedTemp,
        },
        Paste {
            bytes: Vec<u8>,
            temporary: NamedTempFile,
        },
    }
    let incoming = match upload {
        Some(UploadedFile {
            filename: Some(original),
            content_type,
            counted,
        }) => {
            let Some(filename) = safe_filename(&original) else {
                return failure(
                    ImportReasonCode::IngestNoFiles,
                    "there are no files to bring in.",
                    "No input",
                    None,
                );
            };
            Incoming::File {
                filename,
                original_filename: original,
                mime_type: content_type,
                counted,
            }
        }
        _ if !text.is_empty() => {
            let directory = match ensure_save_tmp(&state.root) {
                Ok(directory) => directory,
                Err(error) => return metadata_failed(error),
            };
            let mut temporary = match NamedTempFile::new_in(directory) {
                Ok(temporary) => temporary,
                Err(error) => {
                    return metadata_failed(format!("Failed to stage temporary file: {error}"));
                }
            };
            if let Err(error) = temporary
                .write_all(text.as_bytes())
                .and_then(|()| temporary.flush())
            {
                return metadata_failed(format!("Failed to stage temporary file: {error}"));
            }
            Incoming::Paste {
                bytes: text.into_bytes(),
                temporary,
            }
        }
        _ => {
            return failure(
                ImportReasonCode::IngestNoFiles,
                "there are no files to bring in.",
                "No input",
                None,
            );
        }
    };
    let is_strava = match &incoming {
        Incoming::File {
            original_filename,
            counted,
            ..
        } => solstone_core_import_sources::strava::looks_like_strava_download(
            counted.path(),
            Some(original_filename),
        ),
        Incoming::Paste { bytes, .. } => {
            solstone_core_import_sources::strava::looks_like_strava_csv_bytes(bytes)
        }
    };
    // The Strava card is the one door that takes a Strava download: its importer
    // reads only the workout list, and the run removes the upload when it ends.
    let strava_card = text_value(&data, "source_hint").trim() == STRAVA_SOURCE;
    if is_strava && !strava_card {
        return failure(
            ImportReasonCode::StravaDownload,
            "this looks like a Strava download, so your journal didn't keep it. use the Strava card so only your workouts are read.",
            "this looks like a Strava download, so your journal didn't keep it. use the Strava card so only your workouts are read.",
            None,
        );
    }
    let source_path = match &incoming {
        Incoming::File { counted, .. } => counted.path().to_path_buf(),
        Incoming::Paste { temporary, .. } => temporary.path().to_path_buf(),
    };
    let source_hash = match hash_source(&source_path) {
        Ok(source_hash) => source_hash,
        Err(error) => return metadata_failed(format!("Failed to stage temporary file: {error}")),
    };
    if let Some(existing) = staged_by_client_item(&state.root, &client_item_id) {
        if existing.get("source_hash").and_then(Value::as_str) == Some(source_hash.as_str()) {
            return json_response(
                StatusCode::OK,
                resolved_summary(
                    replay_summary(&existing),
                    &source_path,
                    match &incoming {
                        Incoming::File {
                            original_filename, ..
                        } => Some(original_filename.as_str()),
                        Incoming::Paste { .. } => None,
                    },
                    solstone_core_journal_config::owner_zone(&state.root),
                ),
            );
        }
        return invalid_state(
            "client_item_id already staged for different content; use a new client_item_id or re-fetch the existing item",
            Some(ImportErrorKind::ClientItemConflict),
        );
    }
    // Importing the same Strava download again is how a deleted import is brought
    // back, and it re-places only what is missing, so it is never refused as a repeat.
    if !strava_card && manifest_exists(&state.root, &source_hash) {
        return invalid_state(
            "content already imported",
            Some(ImportErrorKind::AlreadyImported),
        );
    }
    if let Some(existing) = staged_by_source_hash(&state.root, &source_hash) {
        return json_response(
            StatusCode::OK,
            resolved_summary(
                summary(&existing),
                &source_path,
                match &incoming {
                    Incoming::File {
                        original_filename, ..
                    } => Some(original_filename.as_str()),
                    Incoming::Paste { .. } => None,
                },
                solstone_core_journal_config::owner_zone(&state.root),
            ),
        );
    }
    let original_for_timestamp = match &incoming {
        Incoming::File {
            original_filename, ..
        } => original_filename.as_str(),
        Incoming::Paste { .. } => "paste.txt",
    };
    let (semantic, method, no_match_reason) = timestamp_for_upload(
        &source_path,
        original_for_timestamp,
        solstone_core_journal_config::owner_zone(&state.root),
    );
    let import_id = match claim_import_directory(&state.root, &semantic) {
        Ok(import_id) => import_id,
        Err(error) => return metadata_failed(error),
    };
    let (file_path, original_filename, mime_type, file_size) = match incoming {
        Incoming::File {
            filename,
            original_filename,
            mime_type,
            counted,
        } => {
            let target = match contained_import_file(&state.root, &import_id, &filename) {
                Ok(target) => target,
                Err(error) => return metadata_failed(error.to_string()),
            };
            let directory = target.parent().expect("import target parent").to_owned();
            if let Err(error) = create_directory_with_mode(&directory, 0o700) {
                return metadata_failed(error.to_string());
            }
            let file_size = counted.bytes();
            let kept = match counted.keep() {
                Ok(kept) => kept,
                Err(error) => {
                    return metadata_failed(format!("Failed to stage temporary file: {error}"));
                }
            };
            if let Err(error) =
                install_file(kept, &target, AtomicWriteOptions { mode: Some(0o600) })
            {
                return metadata_failed(error.to_string());
            }
            (target, original_filename, mime_type, file_size)
        }
        Incoming::Paste { bytes, temporary } => {
            let file_path = match stage_bytes(&state.root, &import_id, "paste.txt", &bytes) {
                Ok(path) => path,
                Err(error) => return metadata_failed(error.to_string()),
            };
            drop(temporary);
            (
                file_path,
                "paste.txt".to_owned(),
                Some("text/plain".to_owned()),
                bytes.len() as u64,
            )
        }
    };
    let mut metadata = staged_metadata(StagedMetadata {
        zone: solstone_core_journal_config::owner_zone(&state.root),
        timestamp: &semantic,
        original_filename: &original_filename,
        file_path: file_path.display().to_string(),
        source_hash,
        source: source_for(&original_filename, mime_type.as_deref()),
        mime_type,
        client_item_id,
        data: &data,
        is_local_path: false,
        method,
    });
    metadata.insert("import_id".to_owned(), json!(import_id));
    metadata.insert("source_timestamp".to_owned(), json!(semantic));
    metadata.insert("file_size".to_owned(), json!(file_size));
    metadata.insert(
        "timestamp_detection_no_match_reason".to_owned(),
        no_match_reason.map_or(Value::Null, |reason| json!(reason)),
    );
    if let Err(error) =
        solstone_core_import::saved_start::initialize_saved_start(&mut metadata, &import_id)
    {
        return metadata_failed(error.to_string());
    }
    if let Err(error) = contained_import_file(&state.root, &import_id, "import.json") {
        return metadata_failed(error.to_string());
    }
    if let Err(error) = write_import_metadata(&state.root, &import_id, &metadata) {
        return metadata_failed(format!("Failed to write metadata: {error}"));
    }
    json_response(StatusCode::OK, summary(&metadata))
}

pub(crate) async fn save_path(State(state): State<AppState>, Json(data): Json<Value>) -> Response {
    let client_item_id = text_value(&data, "client_item_id");
    if client_item_id.is_empty() {
        return missing("Missing client_item_id");
    }
    let local_path = text_value(&data, "path");
    if local_path.is_empty() {
        return missing("Missing path");
    }
    let local = Path::new(&local_path);
    if !local.exists() {
        return error(
            StatusCode::NOT_FOUND,
            "that file isn't available.",
            "file_not_found",
            format!("Path not found: {local_path}"),
            None,
        );
    }
    if solstone_core_import_sources::strava::looks_like_strava_download(local, None) {
        return failure(
            ImportReasonCode::StravaDownload,
            "this looks like a Strava download, so your journal didn't keep it. use the Strava card so only your workouts are read.",
            "this looks like a Strava download, so your journal didn't keep it. use the Strava card so only your workouts are read.",
            None,
        );
    }
    let source_hash = match hash_source(local) {
        Ok(hash) => hash,
        Err(error) => return metadata_failed(error.to_string()),
    };
    if let Some(existing) = staged_by_client_item(&state.root, &client_item_id) {
        if existing.get("source_hash").and_then(Value::as_str) == Some(source_hash.as_str()) {
            return json_response(
                StatusCode::OK,
                resolved_summary(
                    replay_summary(&existing),
                    local,
                    None,
                    solstone_core_journal_config::owner_zone(&state.root),
                ),
            );
        }
        return invalid_state(
            "client_item_id already staged for different content; use a new client_item_id or re-fetch the existing item",
            Some(ImportErrorKind::ClientItemConflict),
        );
    }
    if manifest_exists(&state.root, &source_hash) {
        return invalid_state(
            "content already imported",
            Some(ImportErrorKind::AlreadyImported),
        );
    }
    if let Some(existing) = staged_by_source_hash(&state.root, &source_hash) {
        return json_response(
            StatusCode::OK,
            resolved_summary(
                summary(&existing),
                local,
                None,
                solstone_core_journal_config::owner_zone(&state.root),
            ),
        );
    }
    let zone = solstone_core_journal_config::owner_zone(&state.root);
    let original_filename = local
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_owned();
    let (semantic, method, no_match_reason) = timestamp_for_upload(local, &original_filename, zone);
    let method = if method == "upload_fallback" {
        "path_fallback"
    } else {
        method
    };
    let import_id = match claim_import_directory(&state.root, &semantic) {
        Ok(import_id) => import_id,
        Err(error) => return metadata_failed(error),
    };
    let mut metadata = staged_metadata(StagedMetadata {
        zone,
        timestamp: &semantic,
        original_filename: &original_filename,
        file_path: local_path,
        source_hash,
        source: source_for(&original_filename, None),
        mime_type: None,
        client_item_id,
        data: &data,
        is_local_path: true,
        method,
    });
    metadata.insert("import_id".to_owned(), json!(import_id));
    metadata.insert("source_timestamp".to_owned(), json!(semantic));
    metadata.insert(
        "timestamp_detection_no_match_reason".to_owned(),
        no_match_reason.map_or(Value::Null, |reason| json!(reason)),
    );
    if let Err(error) =
        solstone_core_import::saved_start::initialize_saved_start(&mut metadata, &import_id)
    {
        return metadata_failed(error.to_string());
    }
    if let Err(error) = contained_import_file(&state.root, &import_id, "import.json") {
        return metadata_failed(error.to_string());
    }
    if let Err(error) = write_import_metadata(&state.root, &import_id, &metadata) {
        return metadata_failed(format!("Failed to write metadata: {error}"));
    }
    json_response(StatusCode::OK, summary(&metadata))
}

pub(crate) async fn meta(State(state): State<AppState>, Json(data): Json<Value>) -> Response {
    let raw_path = text_value(&data, "path");
    if raw_path.is_empty() {
        return missing("Missing import path");
    }
    let Some(timestamp) = data.get("import_id").and_then(Value::as_str).or_else(|| {
        Path::new(&raw_path)
            .parent()
            .and_then(|path| path.file_name())
            .and_then(|value| value.to_str())
    }) else {
        return import_not_found("Import metadata not found");
    };
    if contained_import_file(&state.root, timestamp, "import.json").is_err() {
        return import_not_found("Import metadata not found");
    }
    let mut metadata = match read_import_metadata(&state.root, timestamp) {
        Ok(metadata) => metadata,
        Err(_) => return import_not_found("Import metadata not found"),
    };
    if metadata.get("file_path").and_then(Value::as_str) != Some(raw_path.as_str()) {
        return import_not_found("Import metadata not found");
    }
    if import_is_running_or_successful(&state.root, timestamp, &metadata) {
        return invalid_state("import already started or processed", None);
    }
    if source_hint(&metadata).as_deref() != Some(STRAVA_SOURCE)
        && metadata
            .get("source_hash")
            .and_then(Value::as_str)
            .is_some_and(|hash| manifest_exists(&state.root, &SourceHash::new(hash.to_owned())))
    {
        return invalid_state("content already imported", None);
    }
    let mut changed = Map::new();
    for key in [
        "setting",
        "original_filename",
        "mime_type",
        "source_hint",
        "observer_handle",
        "imported_via",
        "client",
    ] {
        if !data.get(key).is_some() {
            continue;
        }
        let value = if matches!(key, "setting" | "source_hint" | "observer_handle") {
            clean_optional(data.get(key)).map_or(Value::Null, Value::String)
        } else if key == "client" {
            client_bag(data.get(key))
        } else {
            data.get(key).cloned().unwrap()
        };
        if metadata.get(key) != Some(&value) {
            metadata.insert(key.to_owned(), value.clone());
            changed.insert(key.to_owned(), value);
        }
    }
    if !changed.is_empty()
        && let Err(error) = write_import_metadata(&state.root, timestamp, &metadata)
    {
        return metadata_failed(format!("Failed to update metadata: {error}"));
    }
    json_response(
        StatusCode::OK,
        json!({"status":"ok","path":raw_path,"timestamp":timestamp,"updated":changed}),
    )
}

fn relocation_error(error: ImportError, old_timestamp: &str, timestamp: &str) -> Response {
    match error {
        ImportError::SourceMissing { .. } => {
            import_not_found(&format!("Import directory not found for {old_timestamp}"))
        }
        ImportError::DestinationExists { .. } => failure(
            ImportReasonCode::ImportConflict,
            "that import couldn't be started because it already exists.",
            format!("Import already exists for timestamp {timestamp}"),
            None,
        ),
        error => metadata_failed(format!("Failed to rename import directory: {error}")),
    }
}

fn queue_error() -> Response {
    failure(
        ImportReasonCode::ImportQueueUnreachable,
        "your journal's background service isn't running. start it, then try again.",
        "your journal's background service isn't running. start it, then try again.",
        None,
    )
}

pub(crate) fn command(
    path: &str,
    timestamp: &str,
    metadata: &ImportMetadata,
    force: bool,
) -> Vec<String> {
    if crate::audio_archive::selected(metadata) {
        return crate::audio_archive::command(path, metadata);
    }
    // This request path is an intentionally unvalidated trust boundary: iOS echoes metadata.file_path.
    let mut cmd = vec![
        "solstone".to_owned(),
        "journal".to_owned(),
        "importer".to_owned(),
        path.to_owned(),
    ];
    let method = metadata
        .get("timestamp_detection_method")
        .and_then(Value::as_str);
    if metadata.contains_key("owner_timestamp") {
        cmd.push(timestamp.to_owned());
    } else if matches!(method, Some("deterministic") | Some("document_dates")) {
        cmd.extend([
            "--auto".to_owned(),
            "--preview-stamp".to_owned(),
            timestamp.to_owned(),
        ]);
    } else if matches!(method, Some("upload_fallback") | Some("path_fallback")) {
        cmd.extend([
            "--fallback-timestamp".to_owned(),
            timestamp.to_owned(),
            "--preview-stamp".to_owned(),
            timestamp.to_owned(),
        ]);
    } else {
        cmd.push(timestamp.to_owned());
    }
    if let Some(setting) = metadata
        .get("setting")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        cmd.extend(["--setting".to_owned(), setting.to_owned()]);
    }
    if let Some(source_hint) = source_hint(metadata) {
        cmd.extend(["--source".to_owned(), source_hint]);
    }
    if force {
        cmd.push("--force".to_owned());
    }
    if let Some(digest) = metadata
        .get("preview")
        .and_then(|value| value.get("digest"))
        .and_then(Value::as_str)
    {
        cmd.extend(["--snapshot-digest".to_owned(), digest.to_owned()]);
        if let Some(token) = metadata["preview"]["token"].as_str() {
            cmd.extend(["--preview-token".to_owned(), token.to_owned()]);
        }
    }
    cmd
}

pub(crate) type SendSeam = std::sync::Arc<
    std::sync::Mutex<Box<dyn FnMut(&Path, &str, &[String]) -> Result<(), BusError> + Send>>,
>;
pub(crate) type WriteSeam = std::sync::Arc<
    std::sync::Mutex<
        Box<dyn FnMut(&Path, &str, &ImportMetadata) -> Result<PathBuf, ImportError> + Send>,
    >,
>;
pub(crate) type ProduceSeam = std::sync::Arc<
    std::sync::Mutex<
        Box<
            dyn FnMut(PathBuf, PathBuf, String, solstone_core_import::RegistrySource, bool, u64)
                + Send,
        >,
    >,
>;
pub(crate) type FollowSeam = std::sync::Arc<std::sync::Mutex<Box<dyn FnMut(&str, &str) + Send>>>;
pub(crate) type ConsumeSeam = std::sync::Arc<
    std::sync::Mutex<Box<dyn FnMut(&Path, &str, bool) -> Result<(), ImportError> + Send>>,
>;

#[derive(Clone, Default)]
pub(crate) struct StartSeams {
    pub send: Option<SendSeam>,
    pub write: Option<WriteSeam>,
    pub produce: Option<ProduceSeam>,
    pub follow: Option<FollowSeam>,
    pub task_id_hook: Option<std::sync::Arc<TaskIdHook>>,
    pub consume: Option<ConsumeSeam>,
}

impl StartSeams {
    pub const fn production() -> Self {
        Self {
            send: None,
            write: None,
            produce: None,
            follow: None,
            task_id_hook: None,
            consume: None,
        }
    }
}

pub(crate) async fn run_start(
    root: &Path,
    data: Value,
    plan: crate::preview::PreviewPlan<'_>,
    seams: StartSeams,
) -> Response {
    let embedded = match &plan {
        crate::preview::PreviewPlan::Embedded(slot) => Some(Arc::clone(slot)),
        _ => None,
    };
    let resolved_id = resolve_import_id(root, &data);
    let guard = if let Some(id) = resolved_id.as_deref() {
        Some(crate::start_lock::acquire(root, id).await)
    } else {
        None
    };

    let first_only = match data.get("first_start_only") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return invalid_state("invalid first start permission", None),
    };
    if first_only {
        let eligible = resolved_id.as_deref().is_some_and(|id| {
            !embedded.as_ref().is_some_and(|slot| slot.retains(id))
                && read_import_metadata(root, id).is_ok_and(|metadata| {
                    solstone_core_import::saved_start::saved_start_state(&metadata, id)
                        == solstone_core_import::saved_start::SavedStartState::Eligible
                })
        });
        if !eligible || data.get("force").and_then(Value::as_bool) == Some(true) {
            return invalid_state("saved import first start is not available", None);
        }
    }

    let force = data.get("force").and_then(Value::as_bool).unwrap_or(false);
    if let Some(id) = resolved_id.as_deref()
        && embedded.as_ref().is_some_and(|slot| slot.retains(id))
    {
        return replay_body(root, id);
    }
    if !force && let Some(id) = resolved_id.as_deref() {
        if let Ok(metadata) = read_import_metadata(root, id) {
            match crate::audio_archive::completed_run(root, &data, &metadata) {
                Ok(Some(run_id)) => return replay_body(root, &run_id),
                Ok(None) => (),
                Err(error) => {
                    return crate::preview::preview_error_response(
                        StatusCode::BAD_REQUEST,
                        "import_preview_failed",
                        &error,
                    );
                }
            }
        }
        let projection = solstone_core_import::project_import_result(root, id);
        let should_replay = match projection.status {
            solstone_core_import::ProjectionStatus::Running
            | solstone_core_import::ProjectionStatus::Success => true,
            solstone_core_import::ProjectionStatus::Unconfirmed => projection
                .raw_publication
                .as_ref()
                .and_then(|pub_val| {
                    serde_json::from_value::<solstone_core_import::PublicationRecord>(
                        pub_val.clone(),
                    )
                    .ok()
                })
                .is_some_and(|rec| rec.status == solstone_core_import::PublicationStatus::Success),
            _ => false,
        };
        if should_replay {
            return replay_body(root, id);
        }
    }

    let (validated, pending_preview) = match &embedded {
        Some(slot) => {
            match crate::preview::prepare_embedded_start(root, data, Arc::clone(slot)).await {
                Ok(prepared) => prepared,
                Err(err) => {
                    return crate::preview::preview_error_response(
                        ImportReasonCode::ImportPreviewRequired.status(),
                        ImportReasonCode::ImportPreviewRequired.as_wire(),
                        &err,
                    );
                }
            }
        }
        None => match crate::preview::validate_start(root, data, plan).await {
            Ok(data) => (data, None),
            Err(err) => {
                return crate::preview::preview_error_response(
                    ImportReasonCode::ImportPreviewRequired.status(),
                    ImportReasonCode::ImportPreviewRequired.as_wire(),
                    &err,
                );
            }
        },
    };

    let Some(import_id) = resolved_id.as_deref() else {
        return invalid_state("saved import first start is not available", None);
    };
    let consumed = if let Some(consume) = &seams.consume {
        (*consume.lock().unwrap())(root, import_id, first_only)
    } else {
        solstone_core_import::saved_start::consume_saved_start(root, import_id, first_only)
    };
    if let Err(error) = consumed {
        if matches!(error, ImportError::InvalidAttemptState { .. }) {
            return invalid_state("saved import first start is not available", None);
        }
        return metadata_failed(error.to_string());
    }

    let watch = if embedded.is_some() || seams.send.is_some() {
        None
    } else {
        TaskWatch::subscribe(root).await
    };

    let root_path = root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let watch = RefCell::new(watch);
        let root = &root_path;
        let data = &validated;
        let hook = seams.task_id_hook.clone();
        start_with(
            root,
            data,
            |r, task_id, cmd| {
                if let Some(send_fn) = seams.send.as_ref() {
                    let mut lock = send_fn.lock().unwrap();
                    (*lock)(r, task_id, cmd)
                } else {
                    match watch.borrow().as_ref() {
                        Some(w) => w.request(task_id, cmd),
                        None => request_required(r, task_id, cmd),
                    }
                }
            },
            |r, timestamp, meta| {
                if let Some(write_fn) = seams.write.as_ref() {
                    let mut lock = write_fn.lock().unwrap();
                    (*lock)(r, timestamp, meta)
                } else {
                    write_import_metadata(r, timestamp, meta)
                }
            },
            |r, source, timestamp, source_type, force, generation| {
                if let Some(produce_fn) = seams.produce.as_ref() {
                    let mut lock = produce_fn.lock().unwrap();
                    (*lock)(r, source, timestamp, source_type, force, generation)
                } else {
                    spawn_inprocess_import(r, source, timestamp, source_type, force, generation)
                }
            },
            |task_id, timestamp| {
                if let Some(follow_fn) = seams.follow.as_ref() {
                    let mut lock = follow_fn.lock().unwrap();
                    (*lock)(task_id, timestamp)
                } else if let Some(w) = watch.borrow_mut().take() {
                    settle_on_exit(w, root.to_path_buf(), task_id, timestamp);
                }
            },
            Some(StartDispatch {
                task_id_hook: hook,
                embedded,
                pending_preview,
            }),
        )
    })
    .await
    .unwrap_or_else(|error| metadata_failed(format!("could not start the import: {error}")))
}

pub(crate) async fn start(State(state): State<AppState>, Json(data): Json<Value>) -> Response {
    if let Some(slot) = state.route.clone() {
        return run_start(
            &state.root,
            data,
            crate::preview::PreviewPlan::Embedded(slot),
            StartSeams::production(),
        )
        .await;
    }
    let program = match crate::preview::planner() {
        Ok(program) => program,
        Err(err) => {
            return crate::preview::preview_error_response(
                ImportReasonCode::ImportPreviewRequired.status(),
                ImportReasonCode::ImportPreviewRequired.as_wire(),
                &err,
            );
        }
    };
    run_start(
        &state.root,
        data,
        crate::preview::PreviewPlan::Program(&program),
        StartSeams::production(),
    )
    .await
}

/// When the queued importer exits, settle a record it left running, then tell the page how it
/// ended. Without the settle the owner sees "running" for the full wall-clock bound after an
/// importer that failed at once. Without the event a queued import, which sends no live events
/// of its own, reads "preparing…" until the stall timer and "processing…" on its detail page
/// until a reload, however quickly it finished.
fn settle_on_exit(watch: TaskWatch, root: PathBuf, task_id: &str, timestamp: &str) {
    let task_id = task_id.to_owned();
    let timestamp = timestamp.to_owned();
    tokio::spawn(async move {
        let bound = Duration::from_millis(solstone_core_import::RUNNING_ATTEMPT_BOUND_MS);
        let Ok(Some(exit_code)) = tokio::time::timeout(bound, watch.exit_code(&task_id)).await
        else {
            return;
        };
        let _ = tokio::task::spawn_blocking(move || {
            let _ = solstone_core_import::settle_exited_import(
                &root,
                &timestamp,
                exit_code,
                u64::try_from(now_ms()).unwrap_or_default(),
            );
            announce_exit(&root, &timestamp);
        })
        .await;
    });
}

enum ExitEvent {
    Completed(Box<solstone_core_import::ImporterCompleted>),
    Error(solstone_core_import::ImporterError),
}

/// Send the terminal event an exited queued import is owed, whether the importer recorded its
/// own outcome or the settle recorded one for it.
fn announce_exit(root: &Path, timestamp: &str) {
    discard_terminal_strava_download(root, timestamp);
    let projection = solstone_core_import::project_import_result(root, timestamp);
    let emitter = solstone_core_import::EventEmitter::new(root, None);
    match exit_event(timestamp, &projection) {
        Some(ExitEvent::Completed(value)) => {
            solstone_core_import::emit_importer_completed(&emitter, &value);
        }
        Some(ExitEvent::Error(value)) => {
            solstone_core_import::emit_importer_error(&emitter, &value);
        }
        None => {}
    }
}

/// The page answers any terminal event with a read of the durable record, so the event names
/// the import and its attempt and carries the record's own counts; it does not re-derive them.
/// A record that still reads running or pending is owed nothing yet.
fn exit_event(
    import_id: &str,
    projection: &solstone_core_import::ImportProjection,
) -> Option<ExitEvent> {
    use solstone_core_import::ProjectionStatus;
    let duration_ms = projection.duration_ms.unwrap_or_default();
    match projection.status {
        ProjectionStatus::Running | ProjectionStatus::Pending => None,
        ProjectionStatus::Success => Some(ExitEvent::Completed(Box::new(
            solstone_core_import::ImporterCompleted {
                import_id: import_id.to_owned(),
                stage: "complete".to_owned(),
                duration_ms,
                total_files_created: projection.total_files_created.unwrap_or_default(),
                output_files: vec![],
                metadata_file: String::new(),
                stages_run: vec![],
                segments: vec![],
                stream: String::new(),
                source_type: Some(projection.source_type.clone()),
                source_display: Some(projection.source_display.clone()),
                entries_written: projection.entries_written.unwrap_or_default(),
                entities_seeded: projection.entities_seeded.unwrap_or_default(),
                date_range: projection.date_range.clone(),
                generation: projection.generation,
                attempt_id: projection.attempt_id.clone(),
                errors: vec![],
            },
        ))),
        ProjectionStatus::Failed
        | ProjectionStatus::Unavailable
        | ProjectionStatus::Unconfirmed => {
            Some(ExitEvent::Error(solstone_core_import::ImporterError {
                import_id: import_id.to_owned(),
                stage: projection
                    .error_stage
                    .clone()
                    .unwrap_or_else(|| "execution".to_owned()),
                error: projection
                    .error
                    .clone()
                    .unwrap_or_else(|| solstone_core_import::IMPORT_FAILED_REASON.to_owned()),
                duration_ms,
                partial_outputs: vec![],
                generation: projection.generation,
                attempt_id: projection.attempt_id.clone(),
            }))
        }
    }
}

fn replay_body(root: &Path, id: &str) -> Response {
    let projection = solstone_core_import::project_import_result(root, id);
    let effective_task_id = projection
        .task_id
        .as_deref()
        .filter(|task_id| !task_id.is_empty())
        .unwrap_or(id);
    let mut body = if let Some(attempt) = &projection.attempt {
        inprocess_start_response(
            effective_task_id,
            id,
            attempt.generation,
            &attempt.attempt_id,
        )
    } else {
        queued_start_response(effective_task_id, id)
    };
    body["replay"] = json!(true);
    json_response(StatusCode::OK, body)
}

struct StartDispatch {
    task_id_hook: Option<std::sync::Arc<TaskIdHook>>,
    embedded: Option<Arc<crate::ImportRouteSlot>>,
    pending_preview: Option<crate::preview::PendingPreview>,
}

/// Owner-date effect planned before any rename or metadata write.
struct OwnerDatePlan {
    relocate: bool,
    /// File-path write, recorded before `owner_timestamp` is added.
    relocated: Option<ImportMetadata>,
    owner_write: bool,
    record_id: String,
    command_path: String,
    metadata: ImportMetadata,
}

fn plan_owner_date(
    root: &Path,
    data: &Value,
    path: &str,
    timestamp: &str,
    original_timestamp: &str,
    mut metadata: ImportMetadata,
) -> Result<OwnerDatePlan, Box<Response>> {
    let file_path = Path::new(path);
    let is_local_path = !file_path.starts_with(root.join("imports"));
    let mut command_path = path.to_owned();
    let owner_choice = owner_timestamp_choice(data, &metadata);
    let mut record_id = original_timestamp.to_owned();
    let relocate = owner_choice.is_some()
        && original_timestamp != timestamp
        && !root
            .join("imports")
            .join(timestamp)
            .try_exists()
            .unwrap_or(true);
    let mut relocated = None;
    if relocate {
        if let Err(error) = contained_import_file(root, timestamp, "import.json") {
            return Err(Box::new(metadata_failed(error.to_string())));
        }
        let filename = file_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !is_local_path {
            command_path = match contained_import_file(root, timestamp, filename) {
                Ok(planned) => planned.display().to_string(),
                Err(error) => return Err(Box::new(metadata_failed(error.to_string()))),
            };
        }
        metadata.insert("file_path".to_owned(), json!(command_path));
        metadata.insert("source_timestamp".to_owned(), json!(timestamp));
        record_id = timestamp.to_owned();
        metadata.insert("import_id".to_owned(), json!(record_id.as_str()));
        metadata.insert("user_timestamp".to_owned(), json!(record_id.as_str()));
        relocated = Some(metadata.clone());
    }
    let owner_write = owner_choice.is_some();
    if let Some(owner) = owner_choice {
        metadata.insert("owner_timestamp".to_owned(), json!(owner));
        metadata.insert("source_timestamp".to_owned(), json!(timestamp));
    }
    if let Some(relative) = data.get("_preview_source").and_then(Value::as_str) {
        command_path = match contained_import_file(root, &record_id, relative) {
            Ok(planned) => planned.display().to_string(),
            Err(error) => return Err(Box::new(metadata_failed(error.to_string()))),
        };
    }
    Ok(OwnerDatePlan {
        relocate,
        relocated,
        owner_write,
        record_id,
        command_path,
        metadata,
    })
}

fn apply_owner_date<W>(
    root: &Path,
    plan: &OwnerDatePlan,
    original_timestamp: &str,
    timestamp: &str,
    write: &mut W,
) -> Result<(), Box<Response>>
where
    W: FnMut(&Path, &str, &ImportMetadata) -> Result<PathBuf, ImportError>,
{
    if plan.relocate {
        if let Err(error) = relocate_import(root, original_timestamp, timestamp) {
            return Err(Box::new(relocation_error(
                error,
                original_timestamp,
                timestamp,
            )));
        }
        let relocated = plan
            .relocated
            .as_ref()
            .expect("a relocation records the file-path metadata");
        if let Err(error) = write(root, &plan.record_id, relocated) {
            return Err(Box::new(metadata_failed(format!(
                "Failed to update file path in metadata: {error}"
            ))));
        }
    }
    if plan.owner_write
        && let Err(error) = write(root, &plan.record_id, &plan.metadata)
    {
        return Err(Box::new(metadata_failed(error.to_string())));
    }
    Ok(())
}

/// A preview token names a record already on disk. No token means there is nothing to verify.
fn unverified_preview(
    root: &Path,
    data: &Value,
    command_path: &str,
    timestamp: &str,
    force: bool,
    metadata: &ImportMetadata,
) -> Option<Response> {
    let token = data.get("preview_token").and_then(Value::as_str)?;
    solstone_core_import::snapshot::verify_preview(
        root,
        Path::new(command_path),
        token,
        timestamp,
        force,
        metadata.get("setting").and_then(Value::as_str),
        source_hint(metadata).as_deref(),
    )
    .err()
    .map(|error| invalid_state(&error, None))
}

fn start_with<S, W, P, F>(
    root: &Path,
    data: &Value,
    mut send: S,
    mut write: W,
    mut produce: P,
    mut follow: F,
    dispatch: Option<StartDispatch>,
) -> Response
where
    S: FnMut(&Path, &str, &[String]) -> Result<(), BusError>,
    W: FnMut(&Path, &str, &ImportMetadata) -> Result<PathBuf, ImportError>,
    P: FnMut(PathBuf, PathBuf, String, solstone_core_import::RegistrySource, bool, u64),
    F: FnMut(&str, &str),
{
    let Some(path) = data
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return missing("missing params");
    };
    let Some(timestamp) = data
        .get("timestamp")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return missing("missing params");
    };
    let force = data.get("force").and_then(Value::as_bool).unwrap_or(false);
    let original_timestamp = match resolve_import_id(root, data) {
        Some(id) => id,
        None => return missing("missing params"),
    };
    if contained_import_file(root, &original_timestamp, "import.json").is_err() {
        return import_not_found(&format!(
            "Import metadata not found for {original_timestamp}"
        ));
    }
    let metadata = match read_import_metadata(root, &original_timestamp) {
        Ok(metadata) => metadata,
        Err(_) => {
            return import_not_found(&format!(
                "Import metadata not found for {original_timestamp}"
            ));
        }
    };
    if source_hint(&metadata).as_deref() != Some(STRAVA_SOURCE)
        && metadata
            .get("source_hash")
            .and_then(Value::as_str)
            .is_some_and(|hash| manifest_exists(root, &SourceHash::new(hash.to_owned())))
    {
        return invalid_state(
            "content already imported; will not start",
            Some(ImportErrorKind::AlreadyImported),
        );
    }
    // A save of the same bytes replays the staged row, so a source that records no manifest
    // (generic text) would otherwise import a second time. Only `force` re-imports on purpose.
    match solstone_core_import::project_import_result(root, &original_timestamp).status {
        solstone_core_import::ProjectionStatus::Running => {
            return invalid_state("import already started or processed", None);
        }
        solstone_core_import::ProjectionStatus::Success if !force => {
            return invalid_state("content already imported; will not start", None);
        }
        _ => {}
    }
    let (task_id_hook, embedded, pending_preview) = match dispatch {
        Some(dispatch) => (
            dispatch.task_id_hook,
            dispatch.embedded,
            dispatch.pending_preview,
        ),
        None => (None, None, None),
    };
    let mut plan = match plan_owner_date(root, data, path, timestamp, &original_timestamp, metadata)
    {
        Ok(plan) => plan,
        Err(response) => return *response,
    };
    if let Some(slot) = embedded {
        let mut command_metadata = plan.metadata.clone();
        if let Some(pending) = pending_preview.as_ref() {
            command_metadata.insert("preview".to_owned(), pending.record.clone());
        }
        if crate::audio_archive::selected(&command_metadata)
            && let Err(error) = crate::audio_archive::resume_hint(root, &mut command_metadata)
        {
            return crate::preview::preview_error_response(
                StatusCode::BAD_REQUEST,
                "import_preview_failed",
                &error,
            );
        }
        let cmd = command(&plan.command_path, timestamp, &command_metadata, force);
        // Admit the planned command before relocating or writing the owner date.
        let captured = match slot.capture(cmd, crate::embedded_route::source_time(timestamp)) {
            Ok(captured) => captured,
            Err(error) => {
                return crate::preview::preview_error_response(
                    ImportReasonCode::ImportPreviewRequired.status(),
                    ImportReasonCode::ImportPreviewRequired.as_wire(),
                    &crate::preview::PreviewError::PlanFailed(error),
                );
            }
        };
        // Commit a deferred preview on the original id before any date write.
        if let Some(pending) = pending_preview {
            let record = pending.record.clone();
            if let Err(error) = crate::preview::commit_pending_preview(root, pending) {
                drop(captured);
                return crate::preview::preview_error_response(
                    ImportReasonCode::ImportPreviewRequired.status(),
                    ImportReasonCode::ImportPreviewRequired.as_wire(),
                    &error,
                );
            }
            plan.metadata.insert("preview".to_owned(), record.clone());
            if let Some(relocated) = plan.relocated.as_mut() {
                relocated.insert("preview".to_owned(), record);
            }
        }
        if let Err(response) =
            apply_owner_date(root, &plan, &original_timestamp, timestamp, &mut write)
        {
            drop(captured);
            return *response;
        }
        if let Some(response) = unverified_preview(
            root,
            data,
            &plan.command_path,
            timestamp,
            force,
            &plan.metadata,
        ) {
            drop(captured);
            return response;
        }
        let task_id = now_ms().to_string();
        let settle_id = plan.record_id.clone();
        let slot_for_done = Arc::clone(&slot);
        slot.remember(&plan.record_id);
        if let Err(error) = slot.run_retained(captured, move |_result| {
            slot_for_done.forget(&settle_id);
        }) {
            slot.forget(&plan.record_id);
            return crate::preview::preview_error_response(
                ImportReasonCode::ImportPreviewRequired.status(),
                ImportReasonCode::ImportPreviewRequired.as_wire(),
                &crate::preview::PreviewError::PlanFailed(error),
            );
        }
        return json_response(
            StatusCode::OK,
            queued_start_response(&task_id, &plan.record_id),
        );
    }
    if let Err(response) = apply_owner_date(root, &plan, &original_timestamp, timestamp, &mut write)
    {
        return *response;
    }
    // A deferred preview exists only in memory. Verify it after it is written.
    if pending_preview.is_none()
        && let Some(response) = unverified_preview(
            root,
            data,
            &plan.command_path,
            timestamp,
            force,
            &plan.metadata,
        )
    {
        return response;
    }
    let inprocess_source = detect_inprocess_source(&plan.metadata, &plan.command_path);
    let task_id = now_ms().to_string();
    if let Some(source) = inprocess_source {
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let source_hint_str = source_hint(&plan.metadata);
        let facts = match solstone_core_import::admit_running_attempt(
            root,
            &plan.record_id,
            started_at_ms,
            source_hint_str.as_deref(),
        ) {
            Ok(facts) => facts,
            Err(error) => {
                return metadata_failed(format!("Failed to record running attempt: {error}"));
            }
        };
        produce(
            root.to_path_buf(),
            PathBuf::from(&plan.command_path),
            plan.record_id.clone(),
            source,
            force,
            facts.generation,
        );
        return json_response(
            StatusCode::OK,
            inprocess_start_response(
                &task_id,
                &plan.record_id,
                facts.generation,
                &facts.attempt_id,
            ),
        );
    }

    let cmd = command(&plan.command_path, timestamp, &plan.metadata, force);
    let previous_task_id = plan
        .metadata
        .get("task_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned);

    let hook_ref = task_id_hook.as_deref();
    if let Err(error) = exchange_task_id(
        root,
        &plan.record_id,
        &TaskIdOp::Record {
            new_id: task_id.clone(),
        },
        hook_ref,
    ) {
        return metadata_failed(format!("Failed to record queued task: {error}"));
    }

    if send(root, &task_id, &cmd).is_err() {
        let _ = exchange_task_id(
            root,
            &plan.record_id,
            &TaskIdOp::Restore {
                recorded_id: task_id,
                previous: previous_task_id,
            },
            hook_ref,
        );
        return queue_error();
    }

    plan.metadata.insert("task_id".to_owned(), json!(task_id));
    let hinted = source_hint(&plan.metadata).map_or(Value::Null, Value::String);
    plan.metadata.insert("source_hint".to_owned(), hinted);
    let _ = write(root, &plan.record_id, &plan.metadata);
    follow(&task_id, &plan.record_id);
    json_response(
        StatusCode::OK,
        queued_start_response(&task_id, &plan.record_id),
    )
}

pub(crate) fn detect_inprocess_source(
    metadata: &ImportMetadata,
    command_path: &str,
) -> Option<solstone_core_import::RegistrySource> {
    let source_hint = source_hint(metadata);
    let source = clean_optional(metadata.get("source"));
    let extension = Path::new(command_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if source_hint.as_deref() == Some("image")
        || source.as_deref() == Some("image")
        || solstone_core_import_sources::image::is_image_extension(&extension)
    {
        Some(solstone_core_import::RegistrySource::Image)
    } else if source_hint.as_deref() == Some("document")
        || source.as_deref() == Some("document")
        || extension == "pdf"
    {
        Some(solstone_core_import::RegistrySource::Document)
    } else {
        None
    }
}

#[cfg(not(windows))]
fn pdf_worker_sibling() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let parent = current
        .parent()
        .ok_or_else(|| "current executable has no parent".to_owned())?;
    let path = parent.join(format!("solstone-core-pdf{}", std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("missing sibling executable {}", path.display()))
    }
}

fn spawn_inprocess_import(
    root: PathBuf,
    source_path: PathBuf,
    import_id: String,
    source: solstone_core_import::RegistrySource,
    force: bool,
    generation: u64,
) {
    let (producer_root, producer_id) = (root.clone(), import_id.clone());
    run_to_end(root, import_id, move || {
        run_inprocess_import(
            &producer_root,
            &source_path,
            &producer_id,
            source,
            force,
            generation,
        );
    });
}

/// Run an admitted in-process attempt on its own thread; the attempt ends with the thread. A
/// producer that recorded its end is left alone. One that panicked, or returned without
/// recording an end, is settled here rather than reading running until the web restarts.
fn run_to_end(
    root: PathBuf,
    import_id: String,
    run: impl FnOnce() + Send + 'static,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
        let _ = solstone_core_import::settle_exited_import(
            &root,
            &import_id,
            i32::from(run.is_err()),
            u64::try_from(now_ms()).unwrap_or_default(),
        );
        solstone_core_import::release_attempt(&root, &import_id);
    })
}

#[allow(clippy::too_many_arguments)]
fn inprocess_producer_request<'a>(
    root: &'a Path,
    source_path: &'a Path,
    import_id: &'a str,
    source: solstone_core_import::RegistrySource,
    force: bool,
    generation: u64,
    owner_timestamp: Option<&'a str>,
    fallback_timestamp: Option<&'a str>,
) -> solstone_core_import_sources::NativeProducerRequest<'a> {
    solstone_core_import_sources::NativeProducerRequest {
        journal_root: root,
        source_path,
        import_id,
        source,
        revision: None,
        password: None,
        owner_timestamp,
        fallback_timestamp,
        force,
        expected_generation: Some(generation),
        heartbeat_interval: Some(Duration::from_secs(60)),
    }
}

fn run_inprocess_import(
    root: &Path,
    source_path: &Path,
    import_id: &str,
    source: solstone_core_import::RegistrySource,
    force: bool,
    generation: u64,
) {
    let metadata = match read_import_metadata(root, import_id) {
        Ok(metadata) => metadata,
        Err(error) => {
            log::warn!("import {import_id}: {error}");
            return;
        }
    };
    if let Some(token) = metadata
        .get("preview")
        .and_then(|value| value.get("token"))
        .and_then(Value::as_str)
        && let Err(error) = solstone_core_import::snapshot::verify_preview(
            root,
            source_path,
            token,
            metadata
                .get("source_timestamp")
                .and_then(Value::as_str)
                .unwrap_or(import_id),
            force,
            metadata.get("setting").and_then(Value::as_str),
            source_hint(&metadata).as_deref(),
        )
    {
        log::warn!("import {import_id}: {error}");
        if let Err(record_error) = solstone_core_import::record_unconfirmed_attempt(
            root,
            import_id,
            generation,
            now_ms() as u64,
            Some("the source or settings changed; preview again before importing".to_owned()),
        ) {
            log::warn!("import {import_id}: could not record preview refusal: {record_error}");
        }
        return;
    }
    let wire = solstone_core_import_sources::image::SystemWireClient;
    let publication = solstone_core_import::NativePublicationOperations;
    let owner_timestamp = metadata.get("owner_timestamp").and_then(Value::as_str);
    let fallback_timestamp = if owner_timestamp.is_none()
        && matches!(
            metadata
                .get("timestamp_detection_method")
                .and_then(Value::as_str),
            Some("upload_fallback") | Some("path_fallback")
        ) {
        metadata.get("source_timestamp").and_then(Value::as_str)
    } else {
        None
    };
    let req = inprocess_producer_request(
        root,
        source_path,
        import_id,
        source,
        force,
        generation,
        owner_timestamp,
        fallback_timestamp,
    );

    let res = match source {
        solstone_core_import::RegistrySource::Document => {
            let timeout = Duration::from_secs(90);
            #[cfg(not(windows))]
            let worker_path = pdf_worker_sibling().unwrap_or_else(|reason| {
                log::warn!("document import {import_id}: PDF worker unavailable: {reason}");
                PathBuf::new()
            });
            #[cfg(not(windows))]
            let worker =
                solstone_core_import_sources::document::SystemPdfWorker::new(worker_path, timeout);
            // Windows runs the PDF worker only from the verified signed package, under a
            // bounded Job. A refusal (no package, failed verification) records the attempt
            // as failed below rather than leaving it running.
            #[cfg(windows)]
            let windows_worker =
                solstone_core_import_sources::document::WindowsPdfWorker::from_verified_package(
                    timeout,
                );
            #[cfg(windows)]
            let worker = match windows_worker {
                Ok(w) => w,
                Err(reason) => {
                    // The owner sees the generic failure; the reason goes to the journal's log.
                    log::warn!("document import {import_id}: PDF worker unavailable: {reason}");
                    let finished_at_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let _ = solstone_core_import::record_unconfirmed_attempt(
                        root,
                        import_id,
                        generation,
                        finished_at_ms,
                        Some(solstone_core_import::IMPORT_FAILED_REASON.to_owned()),
                    );
                    let emitter = solstone_core_import::events::EventEmitter::new(root, None);
                    solstone_core_import::events::emit_importer_error(
                        &emitter,
                        &solstone_core_import::events::ImporterError {
                            import_id: import_id.to_owned(),
                            stage: "execution".to_owned(),
                            error: solstone_core_import::IMPORT_FAILED_REASON.to_owned(),
                            duration_ms: 0,
                            partial_outputs: vec![],
                            generation: Some(generation),
                            attempt_id: Some(format!("{import_id}:{generation}")),
                        },
                    );
                    return;
                }
            };

            let model = solstone_core_generate::OneShotClient::sibling()
                .ok()
                .map(solstone_core_import_sources::document::SystemDocumentModelClient::new);
            if let Some(ref model) = model {
                let res = solstone_core_import_sources::run_native_producer(
                    req,
                    &wire,
                    &worker,
                    model,
                    &publication,
                );
                model.finish();
                res
            } else {
                solstone_core_import_sources::run_native_producer(
                    req,
                    &wire,
                    &worker,
                    &solstone_core_import_sources::NullDocumentModelClient,
                    &publication,
                )
            }
        }
        _ => {
            let null_worker = solstone_core_import_sources::NullPdfWorker;
            solstone_core_import_sources::run_native_producer(
                req,
                &wire,
                &null_worker,
                &solstone_core_import_sources::NullDocumentModelClient,
                &publication,
            )
        }
    };

    if let Err(_err) = res {
        let emitter = solstone_core_import::events::EventEmitter::new(root, None);
        solstone_core_import::events::emit_importer_error(
            &emitter,
            &solstone_core_import::events::ImporterError {
                import_id: import_id.to_owned(),
                stage: "execution".to_owned(),
                error: "import failed".to_owned(),
                duration_ms: 0,
                partial_outputs: vec![],
                generation: Some(generation),
                attempt_id: Some(format!("{import_id}:{generation}")),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{SendSeam, TaskIdHook, inprocess_producer_request};
    use chrono_tz::Tz;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::{cell::RefCell, fs, rc::Rc, time::Duration};

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        response::Response,
    };
    use serde_json::{Map, Value, json};
    use sha2::{Digest, Sha256};
    #[cfg(feature = "full-tests")]
    use solstone_core_import::{AttemptState, admit_running_attempt, get_attempt_facts};
    use solstone_core_import::{
        ImportError, ImportMetadata, ManifestWriteRequest, SourceHash, hash_source,
        read_import_metadata, relocate_import, write_import_metadata, write_manifest,
    };
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::{
        BusError, ExitEvent, SAVE_TMP_DIR, StartSeams, TaskIdOp, command, exit_event,
        manifest_exists, run_start, source_for, start_with,
    };
    use crate::multipart;

    /// A video file imports as audio, so it is recorded as audio whether or not the browser
    /// sent a type; so is an audio file saved from a local path, which carries no type at all.
    #[tokio::test]
    async fn choosing_a_date_preserves_row_identity_and_an_occupied_target() {
        for occupied in [false, true] {
            let root = TempDir::new().unwrap();
            let old = "20261011_235959";
            let chosen = "20261012_000000";
            let source = root.path().join(format!("imports/{old}/notes.txt"));
            let mut row = metadata(source.display().to_string(), "hash");
            row.insert("import_id".into(), json!(old));
            row.insert("source_timestamp".into(), json!(old));
            staged(root.path(), old, Value::Object(row));
            fs::write(&source, "source notes").unwrap();
            if occupied {
                fs::create_dir_all(root.path().join(format!("imports/{chosen}"))).unwrap();
                fs::write(
                    root.path().join(format!("imports/{chosen}/owner.txt")),
                    "keep",
                )
                .unwrap();
            }
            let response = start_with(
                root.path(),
                &json!({"path":source,"import_id":old,
                "timestamp":chosen,"timestamp_override":true}),
                |_, _, cmd| {
                    assert_eq!(cmd[4], chosen);
                    Ok(())
                },
                write_import_metadata,
                |_, _, _, _, _, _| panic!("text queues through supervisor"),
                |_, _| {},
                None,
            );
            assert_eq!(response.status(), StatusCode::OK);
            let id = if occupied { old } else { chosen };
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let started: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(started["import_id"], id);
            let stored = read_import_metadata(root.path(), id).unwrap();
            assert_eq!(super::summary(&stored)["timestamp"], id);
            assert_eq!(stored["source_timestamp"], chosen);
            assert_eq!(stored["owner_timestamp"], chosen);
            if occupied {
                assert_eq!(
                    fs::read_to_string(root.path().join(format!("imports/{chosen}/owner.txt")))
                        .unwrap(),
                    "keep"
                );
            }
        }
    }

    #[test]
    fn an_explicit_date_equal_to_a_staging_id_still_overrides_the_source() {
        let metadata = json!({"source_timestamp":"20261011_235959", "timestamp_detection_method":"deterministic"});
        let metadata = metadata.as_object().unwrap();
        let data = json!({"timestamp":"20261012_000000", "timestamp_override":true});
        let choice = super::owner_timestamp_choice(&data, metadata).unwrap();
        let mut chosen = metadata.clone();
        chosen.insert("owner_timestamp".into(), json!(choice));
        let cmd = super::command("photo.jpg", &choice, &chosen, false);
        assert_eq!(cmd[4], "20261012_000000");
        assert!(!cmd.iter().any(|arg| arg == "--auto"));
    }

    #[test]
    fn a_video_or_audio_file_is_recorded_as_audio() {
        for (filename, content_type) in [
            ("clip.mp4", Some("video/mp4")),
            ("clip.mp4", None),
            ("clip.MOV", Some("video/quicktime")),
            ("clip.webm", Some("video/webm")),
            ("memo.wav", Some("audio/wav")),
            ("memo.wav", None),
            ("memo.m4a", Some("application/octet-stream")),
        ] {
            assert_eq!(
                source_for(filename, content_type),
                "audio",
                "{filename} ({content_type:?})"
            );
        }
        assert_eq!(source_for("notes.txt", Some("text/plain")), "text");
        assert_eq!(source_for("scan.pdf", Some("application/pdf")), "document");
        assert_eq!(source_for("photo.png", Some("image/png")), "image");
    }

    fn save_tmp_entries(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let directory = root.join("imports").join(SAVE_TMP_DIR);
        if !directory.exists() {
            return Vec::new();
        }
        fs::read_dir(directory)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect()
    }

    fn assert_save_tmp_clean(root: &std::path::Path) {
        assert_eq!(save_tmp_entries(root), Vec::<std::path::PathBuf>::new());
    }

    fn multipart_save(body: String, boundary: &str) -> Request<Body> {
        Request::post("/app/import/api/save")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap()
    }

    fn staged(root: &std::path::Path, timestamp: &str, metadata: Value) {
        write_import_metadata(root, timestamp, &metadata.as_object().unwrap().clone()).unwrap();
    }

    fn metadata(file_path: String, source_hash: &str) -> ImportMetadata {
        Map::from_iter([
            ("file_path".to_owned(), json!(file_path)),
            ("source_hash".to_owned(), json!(source_hash)),
            ("source".to_owned(), json!("wrong-source")),
            ("source_hint".to_owned(), json!("right-source")),
            ("setting".to_owned(), json!("notes")),
        ])
    }

    #[test]
    fn summary_omits_legacy_facet_and_keeps_setting() {
        let mut legacy = metadata("/client/file.txt".to_owned(), "hash");
        legacy.insert("facet".to_owned(), json!("work"));

        let result = super::summary(&legacy);
        assert!(result.get("facet").is_none());
        assert_eq!(result["setting"], json!("notes"));
    }

    async fn response_json(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    fn files_below(directory: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(directory).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files_below(&path, files);
            } else {
                files.push(path);
            }
        }
    }

    #[tokio::test]
    async fn criterion_1_save_path_preserves_owner_material_and_records_a_pointer() {
        let root = TempDir::new().unwrap();
        let owner = root.path().join("owner.txt");
        fs::write(&owner, b"owner bytes").unwrap();
        let before = hash_source(&owner).unwrap();
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"pointer","path":owner}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (_, body) = response_json(response).await;
        let timestamp = body["timestamp"].as_str().unwrap();
        let stored = read_import_metadata(root.path(), timestamp).unwrap();
        assert_eq!(stored["is_local_path"], true);
        assert_eq!(stored["file_path"], owner.display().to_string());
        assert_eq!(stored["source_hash"], before.as_str());
        assert_eq!(
            solstone_core_import::saved_start::saved_start_state(&stored, timestamp),
            solstone_core_import::saved_start::SavedStartState::Eligible
        );
        assert_eq!(hash_source(&owner).unwrap(), before);
        let mut import_files = Vec::new();
        files_below(&root.path().join("imports"), &mut import_files);
        import_files.sort();
        let expected_json = root
            .path()
            .join("imports")
            .join(timestamp)
            .join("import.json");
        assert!(import_files.contains(&expected_json));
        assert!(import_files.len() <= 2);
        for file in &import_files {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            assert!(
                name == "import.json" || name == ".lock.lock" || name == ".lock",
                "unexpected file staged in imports: {}",
                file.display()
            );
        }
        assert_eq!(fs::read(&owner).unwrap(), b"owner bytes");
    }

    #[tokio::test]
    async fn save_path_leaves_an_occupied_clock_and_records_source_time() {
        let root = TempDir::new().unwrap();
        let owner = root.path().join("owner.txt");
        fs::write(&owner, b"owner bytes").unwrap();
        let zone = solstone_core_journal_config::owner_zone(root.path());
        let occupied_name = super::import_timestamp(zone);
        fs::create_dir_all(root.path().join("imports")).unwrap();
        let occupied = root.path().join("imports").join(&occupied_name);
        fs::write(&occupied, b"keep").unwrap();
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"pointer","path":owner}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(fs::read(&occupied).unwrap(), b"keep");
        let timestamp = body["timestamp"].as_str().unwrap();
        let source = body["source_timestamp"].as_str().unwrap();
        assert!(root.path().join("imports").join(timestamp).is_dir());
        if source == occupied_name {
            assert_ne!(timestamp, occupied_name);
        }
        let stored = read_import_metadata(root.path(), timestamp).unwrap();
        assert_eq!(stored["source_timestamp"], source);
        assert_eq!(stored["import_id"], timestamp);
    }

    #[test]
    fn criterion_23_source_hash_is_native_for_files_and_directory_listings() {
        let root = TempDir::new().unwrap();
        let file = root.path().join("source.txt");
        fs::write(&file, b"file-content").unwrap();
        assert_eq!(
            hash_source(&file).unwrap().as_str(),
            format!("{:x}", Sha256::digest(b"file-content"))
        );
        let directory = root.path().join("vault");
        fs::create_dir_all(directory.join("nested")).unwrap();
        fs::write(directory.join("z.txt"), b"z").unwrap();
        fs::write(directory.join("nested/a.txt"), b"abc").unwrap();
        assert_eq!(
            hash_source(&directory).unwrap().as_str(),
            format!("{:x}", Sha256::digest(b"nested/a.txt:3\nz.txt:1"))
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[tokio::test]
    async fn criterion_2_save_stages_upload_bytes_without_a_surviving_temp_file() {
        let root = TempDir::new().unwrap();
        let boundary = "boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nupload\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nhello upload\r\n--{boundary}--\r\n"
        );
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(body, boundary))
            .await
            .unwrap();
        let (_, body) = response_json(response).await;
        let path = std::path::PathBuf::from(body["path"].as_str().unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"hello upload");
        assert_save_tmp_clean(root.path());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[tokio::test]
    async fn save_replays_same_second_same_name_without_replacing_staged_bytes() {
        let root = TempDir::new().unwrap();
        let boundary = "same-second";
        let upload = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nreplay-id\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nowner-safe bytes\r\n--{boundary}--\r\n"
        );
        let mut responses = Vec::new();
        for _ in 0..2 {
            let response = crate::routes(root.path().to_path_buf())
                .oneshot(multipart_save(upload.clone(), boundary))
                .await
                .unwrap();
            responses.push(response_json(response).await.1);
        }
        let path = std::path::PathBuf::from(responses[0]["path"].as_str().unwrap());
        assert_eq!(fs::read(path).unwrap(), b"owner-safe bytes");
        assert_eq!(responses[1]["replay"], true);
        assert_eq!(responses[1]["path"], responses[0]["path"]);
        assert_save_tmp_clean(root.path());
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn upload_body(boundary: &str, client_item_id: &str, filename: &str, bytes: &str) -> String {
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\n{client_item_id}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n{bytes}\r\n--{boundary}--\r\n"
        )
    }

    #[cfg(all(test, feature = "full-tests"))]
    async fn upload(
        root: &std::path::Path,
        client_item_id: &str,
        filename: &str,
        bytes: &str,
    ) -> Value {
        let boundary = "stamp";
        let response = crate::routes(root.to_path_buf())
            .oneshot(multipart_save(
                upload_body(boundary, client_item_id, filename, bytes),
                boundary,
            ))
            .await
            .unwrap();
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[tokio::test]
    async fn same_stamp_uploads_keep_distinct_bytes_and_the_source_time() {
        let root = TempDir::new().unwrap();
        let filename = "note_20260801_120000.txt";
        let stamp = "20260801_120000";
        let first = upload(root.path(), "alpha-item", filename, "alpha-bytes").await;
        let second = upload(root.path(), "bravo-item", filename, "bravo-bytes").await;
        assert_eq!(first["timestamp"], stamp);
        assert_eq!(second["timestamp"], "20260801_120001");
        assert_eq!(first["source_timestamp"], stamp);
        assert_eq!(second["source_timestamp"], stamp);
        let first_path = std::path::PathBuf::from(first["path"].as_str().unwrap());
        let second_path = std::path::PathBuf::from(second["path"].as_str().unwrap());
        assert_eq!(fs::read(&first_path).unwrap(), b"alpha-bytes");
        assert_eq!(fs::read(&second_path).unwrap(), b"bravo-bytes");

        let replay = upload(root.path(), "alpha-item", filename, "alpha-bytes").await;
        assert_eq!(replay["replay"], true);
        assert_eq!(replay["path"], first["path"]);
        assert_eq!(fs::read(&first_path).unwrap(), b"alpha-bytes");

        let same_bytes = upload(root.path(), "echo-item", filename, "alpha-bytes").await;
        assert_eq!(same_bytes["path"], first["path"]);
        assert_eq!(same_bytes["timestamp"], stamp);

        let admitted = admit_running_attempt(root.path(), stamp, 1, Some("text")).unwrap();
        assert_eq!(admitted.generation, 1);
        let third = upload(root.path(), "charlie-item", filename, "charlie-bytes").await;
        assert_eq!(third["timestamp"], "20260801_120002");
        assert_eq!(third["source_timestamp"], stamp);
        assert_eq!(fs::read(&first_path).unwrap(), b"alpha-bytes");
        let first_meta = read_import_metadata(root.path(), stamp).unwrap();
        let facts = get_attempt_facts(&first_meta).unwrap();
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.state, AttemptState::Running);

        let captured = Rc::new(RefCell::new(Vec::new()));
        let capture = Rc::clone(&captured);
        let started = start_with(
            root.path(),
            &json!({"path": second_path.display().to_string(), "timestamp": stamp, "import_id": "20260801_120001"}),
            move |_, _, cmd| {
                *capture.borrow_mut() = cmd.to_vec();
                Ok(())
            },
            write_import_metadata,
            |_, _, _, _, _, _| {},
            |_, _| {},
            None,
        );
        assert_eq!(started.status(), StatusCode::OK);
        assert_eq!(captured.borrow()[4], "--auto");
        assert_eq!(captured.borrow()[6], stamp);
        let second_meta = read_import_metadata(root.path(), "20260801_120001").unwrap();
        assert_eq!(second_meta["source_timestamp"], stamp);
        assert!(!second_meta.contains_key("owner_timestamp"));
        assert_eq!(fs::read(&first_path).unwrap(), b"alpha-bytes");
        assert_eq!(fs::read(&second_path).unwrap(), b"bravo-bytes");
        assert!(!root.path().join("imports/20260801_120003").exists());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[tokio::test]
    async fn new_save_permission_survives_dedupe_but_never_resets_or_upgrades_legacy() {
        use solstone_core_import::saved_start::{
            SAVED_START_KEY, SavedStartState, consume_saved_start, saved_start_state,
        };
        let root = TempDir::new().unwrap();
        let saved = upload(root.path(), "saved-item", "note.txt", "saved bytes").await;
        let id = saved["timestamp"].as_str().unwrap();
        let metadata = read_import_metadata(root.path(), id).unwrap();
        assert_eq!(saved_start_state(&metadata, id), SavedStartState::Eligible);
        let duplicate = upload(root.path(), "saved-item", "note.txt", "saved bytes").await;
        assert_eq!(duplicate["path"], saved["path"]);
        assert_eq!(
            saved_start_state(&read_import_metadata(root.path(), id).unwrap(), id),
            SavedStartState::Eligible
        );
        consume_saved_start(root.path(), id, true).unwrap();
        for client in ["saved-item", "different-client"] {
            let duplicate = upload(root.path(), client, "note.txt", "saved bytes").await;
            assert_eq!(duplicate["path"], saved["path"]);
            assert_eq!(
                saved_start_state(&read_import_metadata(root.path(), id).unwrap(), id),
                SavedStartState::Consumed
            );
        }

        let legacy_root = TempDir::new().unwrap();
        let legacy = upload(legacy_root.path(), "old-item", "old.txt", "old bytes").await;
        let legacy_id = legacy["timestamp"].as_str().unwrap();
        let mut old = read_import_metadata(legacy_root.path(), legacy_id).unwrap();
        old.remove(SAVED_START_KEY);
        fs::write(
            legacy_root
                .path()
                .join("imports")
                .join(legacy_id)
                .join("import.json"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        for client in ["old-item", "another-client"] {
            let duplicate = upload(legacy_root.path(), client, "old.txt", "old bytes").await;
            assert_eq!(duplicate["path"], legacy["path"]);
            assert_eq!(
                saved_start_state(
                    &read_import_metadata(legacy_root.path(), legacy_id).unwrap(),
                    legacy_id
                ),
                SavedStartState::Unknown
            );
        }
        assert!(
            !read_import_metadata(legacy_root.path(), legacy_id)
                .unwrap()
                .contains_key(SAVED_START_KEY)
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[tokio::test]
    async fn save_pastes_text_field_without_a_file_part() {
        let root = TempDir::new().unwrap();
        let boundary = "paste";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\npaste-id\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"text\"\r\n\r\npasted notes\r\n--{boundary}--\r\n"
        );
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(body, boundary))
            .await
            .unwrap();
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::OK);
        let path = std::path::PathBuf::from(body["path"].as_str().unwrap());
        assert_eq!(path.file_name().unwrap(), "paste.txt");
        assert_eq!(fs::read(&path).unwrap(), b"pasted notes");
        assert_save_tmp_clean(root.path());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[tokio::test]
    async fn save_conflict_and_already_imported_leave_no_temp() {
        let root = TempDir::new().unwrap();
        let boundary = "first";
        let first = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nkeep-id\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nfirst bytes\r\n--{boundary}--\r\n"
        );
        let first = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(first, boundary))
            .await
            .unwrap();
        let (_, first) = response_json(first).await;
        let first_path = std::path::PathBuf::from(first["path"].as_str().unwrap());
        let conflict = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nkeep-id\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"other.txt\"\r\nContent-Type: text/plain\r\n\r\nsecond bytes\r\n--{boundary}--\r\n"
        );
        let conflict = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(conflict, boundary))
            .await
            .unwrap();
        let (status, conflict) = response_json(conflict).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(conflict["reason_code"], "invalid_operation_for_state");
        assert_eq!(fs::read(&first_path).unwrap(), b"first bytes");
        assert_save_tmp_clean(root.path());

        let hash = read_import_metadata(root.path(), first["timestamp"].as_str().unwrap()).unwrap()
            ["source_hash"]
            .as_str()
            .unwrap()
            .to_owned();
        write_manifest(&ManifestWriteRequest {
            journal_root: root.path(),
            import_id: "complete",
            source_type: "text",
            source_hash: &SourceHash::new(hash),
            entry_count: 1,
            days_affected: &[],
            files_created: &[],
            imported_via: "test",
            link_id: None,
            observer_handle: None,
            raw_retention: None,
        })
        .unwrap();
        let imported = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nnew-id\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nfirst bytes\r\n--{boundary}--\r\n"
        );
        let imported = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(imported, boundary))
            .await
            .unwrap();
        let (_, imported) = response_json(imported).await;
        assert_eq!(imported["reason_code"], "invalid_operation_for_state");
        assert_save_tmp_clean(root.path());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[tokio::test]
    async fn save_already_staged_same_hash_leaves_no_temp() {
        let root = TempDir::new().unwrap();
        let boundary = "staged";
        let first = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nfirst-id\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nshared bytes\r\n--{boundary}--\r\n"
        );
        let first = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(first, boundary))
            .await
            .unwrap();
        let (_, first) = response_json(first).await;
        let again = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nsecond-id\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nshared bytes\r\n--{boundary}--\r\n"
        );
        let again = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(again, boundary))
            .await
            .unwrap();
        let (_, again) = response_json(again).await;
        assert_eq!(again["path"], first["path"]);
        assert_eq!(again["replay"], false);
        assert_save_tmp_clean(root.path());
    }

    #[tokio::test]
    async fn save_refuses_non_file_part_over_64_mib() {
        let root = TempDir::new().unwrap();
        let boundary = "huge";
        let oversized = "x".repeat(multipart::MAX_PART_BYTES + 1);
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\n{oversized}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n--{boundary}--\r\n"
        );
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(body, boundary))
            .await
            .unwrap();
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["detail"], "multipart part exceeds 64 MiB");
        assert_save_tmp_clean(root.path());
    }

    #[test]
    fn upload_timestamp_prefers_a_valid_deterministic_filename_timestamp() {
        let (timestamp, method, reason) = super::timestamp_for_upload(
            std::path::Path::new("notes.txt"),
            "notes_20260801_120000.txt",
            Tz::UTC,
        );
        assert_eq!(timestamp, "20260801_120000");
        assert_eq!(method, "deterministic");
        assert_eq!(reason, None);
    }

    #[test]
    fn deterministic_filename_timestamp_wins_when_payload_is_only_on_disk() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("payload.bin");
        fs::write(&path, b"not an image and not exif").unwrap();
        let (timestamp, method, reason) =
            super::timestamp_for_upload(&path, "notes_20260801_120000.txt", Tz::UTC);
        assert_eq!(timestamp, "20260801_120000");
        assert_eq!(method, "deterministic");
        assert_eq!(reason, None);
    }

    fn make_test_jpeg_app1(dt: &str, md: &str) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8];
        data.extend_from_slice(&[0xFF, 0xE1]);

        let mut tiff = Vec::new();
        tiff.extend_from_slice(&[b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00]);

        let exif_ifd_offset = 26u32;
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x8769u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&exif_ifd_offset.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());

        let dt_bytes = format!("{dt}\0").into_bytes();
        let mod_bytes = format!("{md}\0").into_bytes();
        let num_entries = 2u16;
        let ifd_len = 2 + (num_entries as usize * 12) + 4;
        let mut string_data_offset = (26 + ifd_len) as u32;

        tiff.extend_from_slice(&num_entries.to_le_bytes());

        // Entry 1: Tag 0x0132 (ModifyDate)
        tiff.extend_from_slice(&0x0132u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&(mod_bytes.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&string_data_offset.to_le_bytes());
        string_data_offset += mod_bytes.len() as u32;

        // Entry 2: Tag 0x9003 (DateTimeOriginal)
        tiff.extend_from_slice(&0x9003u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&(dt_bytes.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&string_data_offset.to_le_bytes());

        tiff.extend_from_slice(&0u32.to_le_bytes());

        tiff.extend_from_slice(&mod_bytes);
        tiff.extend_from_slice(&dt_bytes);

        let app1_len = (2 + 6 + tiff.len()) as u16;
        data.extend_from_slice(&app1_len.to_be_bytes());
        data.extend_from_slice(b"Exif\0\0");
        data.extend_from_slice(&tiff);
        data.extend_from_slice(&[0xFF, 0xD9]);
        data
    }

    fn make_test_mp4_mvhd(creation_secs_since_1904: u32, brand: &[u8; 4]) -> Vec<u8> {
        let mut data = Vec::new();
        // ftyp box
        data.extend_from_slice(&16u32.to_be_bytes());
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(brand);
        data.extend_from_slice(&0u32.to_be_bytes());

        // mvhd box
        let mut mvhd = Vec::new();
        mvhd.extend_from_slice(&28u32.to_be_bytes());
        mvhd.extend_from_slice(b"mvhd");
        mvhd.push(0); // version 0
        mvhd.extend_from_slice(&[0, 0, 0]); // flags
        mvhd.extend_from_slice(&creation_secs_since_1904.to_be_bytes());
        mvhd.extend_from_slice(&999999u32.to_be_bytes()); // mod time
        mvhd.extend_from_slice(&1000u32.to_be_bytes()); // timescale
        mvhd.extend_from_slice(&0u32.to_be_bytes()); // duration

        // moov box
        let moov_len = (8 + mvhd.len()) as u32;
        data.extend_from_slice(&moov_len.to_be_bytes());
        data.extend_from_slice(b"moov");
        data.extend_from_slice(&mvhd);
        data
    }

    #[test]
    fn creation_stamp_parity() {
        let root = TempDir::new().unwrap();
        let old_mtime = std::time::UNIX_EPOCH + Duration::from_secs(1_704_067_200);

        let jpeg_path = root.path().join("photo.jpg");
        fs::write(
            &jpeg_path,
            make_test_jpeg_app1("2026:05:10 14:30:00", "1999:01:01 00:00:00"),
        )
        .unwrap();

        let m4a_path = root.path().join("audio.m4a");
        fs::write(&m4a_path, make_test_mp4_mvhd(3_861_259_200, b"M4A ")).unwrap();

        let mp4_path = root.path().join("video.mp4");
        fs::write(&mp4_path, make_test_mp4_mvhd(3_861_259_200, b"mp42")).unwrap();

        let txt_path = root.path().join("20260704_090000.txt");
        fs::write(&txt_path, b"some text content").unwrap();

        let wav_path = root
            .path()
            .join("limitless_pendant_2026-01-15T03-30-00_to_2026-01-15T04-00-00.wav");
        fs::write(&wav_path, b"RIFF....WAVEfmt ").unwrap();

        let nodate_path = root.path().join("nodate.bin");
        fs::write(&nodate_path, b"unrecognized random bytes").unwrap();

        for p in [
            &jpeg_path,
            &m4a_path,
            &mp4_path,
            &txt_path,
            &wav_path,
            &nodate_path,
        ] {
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old_mtime))
                .unwrap();
        }

        let cases = [
            (&jpeg_path, "photo.jpg", Tz::UTC, "20260510_143000"),
            (&m4a_path, "audio.m4a", Tz::UTC, "20260510_120000"),
            (&mp4_path, "video.mp4", Tz::UTC, "20260510_120000"),
            (&txt_path, "20260704_090000.txt", Tz::UTC, "20260704_090000"),
            (
                &wav_path,
                "limitless_pendant_2026-01-15T03-30-00_to_2026-01-15T04-00-00.wav",
                Tz::America__New_York,
                "20260114_223000",
            ),
        ];

        for (path, filename, zone, expected_stamp) in cases {
            let (upload_stamp, method, reason) = super::timestamp_for_upload(path, filename, zone);
            let detected = solstone_core_import_sources::image::detected_creation_stamp(
                path,
                Some(filename),
                zone,
            );
            assert_eq!(
                detected.as_ref().map(|t| t.as_str()),
                Some(upload_stamp.as_str()),
                "mismatch for {filename}"
            );
            assert_eq!(
                upload_stamp, expected_stamp,
                "stamp mismatch for {filename}"
            );
            assert_eq!(method, "deterministic", "method mismatch for {filename}");
            assert_eq!(reason, None, "reason mismatch for {filename}");
        }

        let (upload_stamp, method, reason) =
            super::timestamp_for_upload(&nodate_path, "nodate.bin", Tz::UTC);
        assert_eq!(method, "upload_fallback");
        assert_eq!(reason, Some("no_deterministic_match"));
        assert!(!upload_stamp.starts_with("20240101"));
        let detected_none = solstone_core_import_sources::image::detected_creation_stamp(
            &nodate_path,
            Some("nodate.bin"),
            Tz::UTC,
        );
        assert_eq!(detected_none, None);
    }

    /// Saving never consults a model, so an upload that falls back to its upload time must
    /// not record a model attempt or a model no-match, in its response or in import.json.
    #[cfg(all(test, feature = "full-tests"))]
    #[tokio::test]
    async fn upload_fallback_records_no_model_attempt() {
        let root = TempDir::new().unwrap();
        let boundary = "fallback";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nfallback-id\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\nContent-Type: text/plain\r\n\r\nno timestamp here\r\n--{boundary}--\r\n"
        );
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(body, boundary))
            .await
            .unwrap();
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::OK);
        let diagnostics = &body["diagnostics"];
        assert_eq!(diagnostics["timestamp_detection_method"], "upload_fallback");
        assert_eq!(diagnostics["timestamp_detection_model_called"], false);
        assert_eq!(
            diagnostics["timestamp_detection_no_match_reason"],
            "no_deterministic_match"
        );

        let import_id = body["timestamp"].as_str().unwrap();
        let stored = read_import_metadata(root.path(), import_id).unwrap();
        assert_eq!(stored["timestamp_detection_model_called"], false);
        assert_eq!(
            stored["timestamp_detection_no_match_reason"],
            "no_deterministic_match"
        );
    }

    #[test]
    fn criterion_22_pointer_start_never_renames_owner_material() {
        let root = TempDir::new().unwrap();
        let owner = root.path().join("owner.txt");
        fs::write(&owner, b"only owner copy").unwrap();
        let mut pointer = metadata(owner.display().to_string(), "hash");
        pointer.insert("user_timestamp".to_owned(), json!("old-ts"));
        staged(root.path(), "new-ts", Value::Object(pointer));
        let response = start_with(
            root.path(),
            &json!({"path":owner,"timestamp":"new-ts"}),
            |_, _, _| Ok(()),
            write_import_metadata,
            |_, _, _, _, _, _| {},
            |_, _| {},
            None,
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(fs::read(&owner).unwrap(), b"only owner copy");
        assert!(!root.path().join("imports/old-ts").exists());
        assert_eq!(
            read_import_metadata(root.path(), "new-ts").unwrap()["file_path"],
            owner.display().to_string()
        );
    }

    // Owner-only directory modes are Unix permission bits.
    #[cfg(unix)]
    #[test]
    fn criterion_28_relocation_repairs_import_chain_privacy() {
        let root = TempDir::new().unwrap();
        let old = root.path().join("imports/old");
        fs::create_dir_all(&old).unwrap();
        fs::set_permissions(
            root.path().join("imports"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(&old, fs::Permissions::from_mode(0o755)).unwrap();
        relocate_import(root.path(), "old", "new").unwrap();
        assert_eq!(
            fs::metadata(root.path().join("imports"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join("imports/new"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn criterion_21_start_command_keeps_reference_order_and_request_path() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("imports/ts/item.txt");
        staged(
            root.path(),
            "ts",
            Value::Object(metadata(path.display().to_string(), "hash")),
        );
        let captured = Rc::new(RefCell::new(Vec::new()));
        let capture = Rc::clone(&captured);
        let response = start_with(
            root.path(),
            &json!({"path":"/client/echoed/path.txt","timestamp":"ts","force":true}),
            move |_, _, cmd| {
                *capture.borrow_mut() = cmd.to_vec();
                Ok(())
            },
            write_import_metadata,
            |_, _, _, _, _, _| {},
            |_, _| {},
            None,
        );
        assert_eq!(response.status(), StatusCode::OK);
        // The queued start records its task id durably: without it the row has no clock
        // and no way to time out, and stays pending for good.
        let stored = solstone_core_import::read_import_metadata(root.path(), "ts").unwrap();
        assert!(
            stored
                .get("task_id")
                .is_some_and(|task_id| !task_id.is_null()),
            "{stored:?}"
        );
        assert_eq!(
            *captured.borrow(),
            vec![
                "solstone",
                "journal",
                "importer",
                "/client/echoed/path.txt",
                "ts",
                "--setting",
                "notes",
                "--source",
                "right-source",
                "--force"
            ]
        );
        let mut whitespace = metadata(path.display().to_string(), "hash");
        whitespace.insert("source_hint".to_owned(), json!("  \t "));
        assert!(!command("path", "ts", &whitespace, false).contains(&"--source".to_owned()));
    }

    #[tokio::test]
    async fn a_quick_import_with_no_source_queues_without_one_and_settles_when_it_exits() {
        let root = TempDir::new().unwrap();
        let notes = root.path().join("notes.md");
        fs::write(&notes, b"# meeting notes\n").unwrap();
        // The quick drop-and-validate flow names no source; only the guided flow does.
        let response = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"quick","path":notes}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, saved) = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        let timestamp = saved["timestamp"].as_str().unwrap().to_owned();

        let captured = Rc::new(RefCell::new(Vec::new()));
        let capture = Rc::clone(&captured);
        let followed = Rc::new(RefCell::new(None));
        let follow = Rc::clone(&followed);
        let response = start_with(
            root.path(),
            &json!({"path":notes,"timestamp":timestamp}),
            move |_, _, cmd| {
                *capture.borrow_mut() = cmd.to_vec();
                Ok(())
            },
            write_import_metadata,
            |_, _, _, _, _, _| panic!("a text file is not produced in process"),
            move |task_id, timestamp| {
                *follow.borrow_mut() = Some((task_id.to_owned(), timestamp.to_owned()));
            },
            None,
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            *captured.borrow(),
            vec![
                "solstone".to_owned(),
                "journal".to_owned(),
                "importer".to_owned(),
                notes.display().to_string(),
                "--fallback-timestamp".to_owned(),
                timestamp.clone(),
                "--preview-stamp".to_owned(),
                timestamp.clone(),
            ]
        );
        let stored = read_import_metadata(root.path(), &timestamp).unwrap();
        assert_eq!(stored["source_hint"], Value::Null);
        let (task_id, followed_timestamp) = followed.borrow_mut().take().unwrap();
        assert_eq!(stored["task_id"], json!(task_id));
        assert_eq!(followed_timestamp, timestamp);
        let running = solstone_core_import::project_import_result(root.path(), &timestamp);
        assert_eq!(
            running.status,
            solstone_core_import::ProjectionStatus::Running
        );
        assert!(exit_event(&timestamp, &running).is_none());

        // The supervisor reports the importer's non-zero exit: the row is failed now, not
        // after the hour-long running bound, and the page is told so.
        assert!(
            solstone_core_import::settle_exited_import(
                root.path(),
                &timestamp,
                1,
                super::now_ms() as u64
            )
            .unwrap()
        );
        let settled = solstone_core_import::project_import_result(root.path(), &timestamp);
        assert_eq!(
            settled.status,
            solstone_core_import::ProjectionStatus::Failed
        );
        let Some(ExitEvent::Error(error)) = exit_event(&timestamp, &settled) else {
            panic!("a failed import is announced as an importer error");
        };
        assert_eq!(error.import_id, timestamp);
        assert_eq!(error.generation, Some(1));
        assert_eq!(error.attempt_id, Some(format!("{timestamp}:1")));
    }

    #[test]
    fn an_exited_import_that_recorded_its_own_success_is_announced_completed() {
        let root = TempDir::new().unwrap();
        recorded_import(root.path(), "done", "text", Outcome::Succeeded);
        let projection = solstone_core_import::project_import_result(root.path(), "done");
        let Some(ExitEvent::Completed(completed)) = exit_event("done", &projection) else {
            panic!("a recorded success is announced as completed");
        };
        assert_eq!(completed.import_id, "done");
        assert_eq!(completed.generation, projection.generation);
        assert!(completed.generation.is_some());
    }

    enum Outcome {
        Succeeded,
        Failed,
        Unconfirmed,
    }

    fn recorded_import(root: &std::path::Path, dir: &str, source_type: &str, outcome: Outcome) {
        staged(
            root,
            dir,
            Value::Object(metadata(
                root.join(format!("imports/{dir}/photo.png"))
                    .display()
                    .to_string(),
                "same-bytes",
            )),
        );
        write_manifest(&ManifestWriteRequest {
            journal_root: root,
            import_id: dir,
            source_type,
            source_hash: &SourceHash::new("same-bytes".to_owned()),
            entry_count: 1,
            days_affected: &[],
            files_created: &[],
            imported_via: "native",
            link_id: None,
            observer_handle: None,
            raw_retention: None,
        })
        .unwrap();
        let now = super::now_ms() as u64;
        let attempt = solstone_core_import::admit_running_attempt(root, dir, now, None).unwrap();
        let (reason, expected) = match outcome {
            Outcome::Succeeded => {
                solstone_core_import::record_completed_attempt(
                    root,
                    dir,
                    attempt.generation,
                    now + 1,
                    None,
                    None,
                )
                .unwrap();
                assert_eq!(
                    solstone_core_import::project_import_result(root, dir).status,
                    solstone_core_import::ProjectionStatus::Success
                );
                return;
            }
            Outcome::Failed => (
                solstone_core_import::IMPORT_FAILED_REASON,
                solstone_core_import::ProjectionStatus::Failed,
            ),
            Outcome::Unconfirmed => (
                solstone_core_import::IMPORT_UNCONFIRMED_REASON,
                solstone_core_import::ProjectionStatus::Unconfirmed,
            ),
        };
        solstone_core_import::record_unconfirmed_attempt(
            root,
            dir,
            attempt.generation,
            now + 1,
            Some(reason.to_owned()),
        )
        .unwrap();
        assert_eq!(
            solstone_core_import::project_import_result(root, dir).status,
            expected
        );
    }

    #[test]
    fn a_failed_original_import_does_not_hold_its_bytes() {
        let hash = SourceHash::new("same-bytes".to_owned());

        for source_type in ["image", "document"] {
            let root = TempDir::new().unwrap();
            recorded_import(root.path(), "failed", source_type, Outcome::Failed);
            assert!(!manifest_exists(root.path(), &hash), "{source_type}");

            for outcome in [Outcome::Unconfirmed, Outcome::Succeeded] {
                let root = TempDir::new().unwrap();
                recorded_import(root.path(), "held", source_type, outcome);
                assert!(manifest_exists(root.path(), &hash), "{source_type}");
            }
        }

        let root = TempDir::new().unwrap();
        recorded_import(root.path(), "failed", "text", Outcome::Failed);
        assert!(manifest_exists(root.path(), &hash));
    }

    #[test]
    fn a_successful_import_holds_its_bytes_beside_a_failed_one_in_either_order() {
        let hash = SourceHash::new("same-bytes".to_owned());
        for source_type in ["image", "document"] {
            for (failed, succeeded) in [("a", "b"), ("b", "a")] {
                let root = TempDir::new().unwrap();
                recorded_import(root.path(), failed, source_type, Outcome::Failed);
                recorded_import(root.path(), succeeded, source_type, Outcome::Succeeded);
                assert!(manifest_exists(root.path(), &hash), "{source_type}");
            }
        }
    }

    #[test]
    fn a_finished_or_running_import_is_not_started_again_without_force() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("imports/ts/notes.md");
        staged(
            root.path(),
            "ts",
            Value::Object(metadata(path.display().to_string(), "hash")),
        );
        let start = |force: bool| {
            let sent = Rc::new(RefCell::new(false));
            let send = Rc::clone(&sent);
            let status = start_with(
                root.path(),
                &json!({"path":path,"timestamp":"ts","force":force}),
                move |_, _, _| {
                    *send.borrow_mut() = true;
                    Ok(())
                },
                write_import_metadata,
                |_, _, _, _, _, _| {},
                |_, _| {},
                None,
            )
            .status();
            (status, *sent.borrow())
        };

        let now = super::now_ms() as u64;
        let attempt =
            solstone_core_import::admit_running_attempt(root.path(), "ts", now, None).unwrap();
        assert_eq!(start(true), (StatusCode::BAD_REQUEST, false));

        solstone_core_import::record_completed_attempt(
            root.path(),
            "ts",
            attempt.generation,
            now + 1,
            None,
            None,
        )
        .unwrap();
        assert_eq!(start(false), (StatusCode::BAD_REQUEST, false));
        assert_eq!(start(true), (StatusCode::OK, true));
    }

    fn projected(root: &std::path::Path, id: &str) -> solstone_core_import::ProjectionStatus {
        solstone_core_import::project_import_result(root, id).status
    }

    #[test]
    fn an_in_process_import_that_panics_or_stops_short_does_not_read_running() {
        use solstone_core_import::ProjectionStatus::{Failed, Running, Unconfirmed};
        let root = TempDir::new().unwrap();
        for (id, panics, ended) in [("panics", true, Failed), ("returns", false, Unconfirmed)] {
            let path = root.path().join(format!("imports/{id}/scan.pdf"));
            staged(
                root.path(),
                id,
                Value::Object(metadata(path.display().to_string(), "hash")),
            );
            solstone_core_import::admit_running_attempt(
                root.path(),
                id,
                super::now_ms() as u64,
                Some("document"),
            )
            .unwrap();
            assert_eq!(projected(root.path(), id), Running, "{id}");
            super::run_to_end(root.path().to_path_buf(), id.to_owned(), move || {
                assert!(!panics, "the producer panicked");
            })
            .join()
            .unwrap();
            assert_eq!(projected(root.path(), id), ended, "{id}");
        }
    }

    #[test]
    fn an_import_whose_producer_is_gone_reads_unconfirmed_and_starts_again() {
        use solstone_core_import::ProjectionStatus::{Running, Unconfirmed};
        let root = TempDir::new().unwrap();
        let path = root.path().join("imports/ts/notes.md");
        staged(
            root.path(),
            "ts",
            Value::Object(metadata(path.display().to_string(), "hash")),
        );
        // Admitted two hours ago and still held: a slow import is running, not timed out.
        let two_hours_ago = super::now_ms() as u64 - 7_200_000;
        solstone_core_import::admit_running_attempt(root.path(), "ts", two_hours_ago, None)
            .unwrap();
        assert_eq!(projected(root.path(), "ts"), Running);

        // The process running it goes away (a restart) without recording how it ended.
        solstone_core_import::release_attempt(root.path(), "ts");
        assert_eq!(projected(root.path(), "ts"), Unconfirmed);
        let response = start_with(
            root.path(),
            &json!({"path":path,"timestamp":"ts"}),
            |_, _, _| Ok(()),
            write_import_metadata,
            |_, _, _, _, _, _| {},
            |_, _| {},
            None,
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(projected(root.path(), "ts"), Running);
    }

    #[test]
    fn a_retried_import_reads_running_until_its_importer_settles_it() {
        use solstone_core_import::ProjectionStatus::{Failed, Running};
        let root = TempDir::new().unwrap();
        let path = root.path().join("imports/ts/notes.md");
        staged(
            root.path(),
            "ts",
            Value::Object(metadata(path.display().to_string(), "hash")),
        );
        let now = super::now_ms() as u64;
        let first =
            solstone_core_import::admit_running_attempt(root.path(), "ts", now - 5000, None)
                .unwrap();
        solstone_core_import::record_unconfirmed_attempt(
            root.path(),
            "ts",
            first.generation,
            now - 4000,
            Some(solstone_core_import::IMPORT_FAILED_REASON.to_owned()),
        )
        .unwrap();
        assert_eq!(projected(root.path(), "ts"), Failed);

        // The owner retries; the importer is queued and has not admitted its attempt yet.
        let response = start_with(
            root.path(),
            &json!({"path":path,"timestamp":"ts"}),
            |_, _, _| Ok(()),
            write_import_metadata,
            |_, _, _, _, _, _| panic!("a text file is not produced in process"),
            |_, _| {},
            None,
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(projected(root.path(), "ts"), Running);

        // It exits before admitting: the retry itself failed, as a new attempt.
        assert!(
            solstone_core_import::settle_exited_import(root.path(), "ts", 1, now + 1000).unwrap()
        );
        assert_eq!(projected(root.path(), "ts"), Failed);
        let stored = read_import_metadata(root.path(), "ts").unwrap();
        assert_eq!(
            solstone_core_import::get_attempt_facts(&stored)
                .unwrap()
                .generation,
            first.generation + 1
        );
    }

    #[test]
    fn a_stored_null_hint_never_reaches_the_importer() {
        let mut stored = metadata("path".to_owned(), "hash");
        for value in [Value::Null, json!("null"), json!(7), json!(" ")] {
            stored.insert("source_hint".to_owned(), value.clone());
            assert!(
                !command("path", "ts", &stored, false).contains(&"--source".to_owned()),
                "{value}"
            );
        }
    }

    #[test]
    fn criterion_24_unreachable_bus_writes_no_task_id() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("imports/ts/item.txt");
        staged(
            root.path(),
            "ts",
            Value::Object(metadata(path.display().to_string(), "hash")),
        );
        let writes = Rc::new(RefCell::new(0));
        let write_count = Rc::clone(&writes);
        let response = start_with(
            root.path(),
            &json!({"path":path,"timestamp":"ts"}),
            |_, _, _| Err(BusError::Unavailable),
            move |root, timestamp, metadata| {
                *write_count.borrow_mut() += 1;
                write_import_metadata(root, timestamp, metadata)
            },
            |_, _, _, _, _, _| {},
            |_, _| {},
            None,
        );
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(*writes.borrow(), 0);
        assert!(
            read_import_metadata(root.path(), "ts")
                .unwrap()
                .get("task_id")
                .is_none()
        );
    }

    #[tokio::test]
    async fn criterion_26_metadata_failure_after_send_returns_ok_with_task_id() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("imports/ts/item.txt");
        staged(
            root.path(),
            "ts",
            Value::Object(metadata(path.display().to_string(), "hash")),
        );
        let response = start_with(
            root.path(),
            &json!({"path":path,"timestamp":"ts"}),
            |_, _, _| Ok(()),
            |_, _, _| {
                Err(ImportError::MetadataWriteFailed {
                    path: std::path::PathBuf::from("import.json"),
                    message: "disk full".to_owned(),
                })
            },
            |_, _, _, _, _, _| {},
            |_, _| {},
            None,
        );
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        let task_id = body["task_id"].as_str().expect("task_id present");
        assert!(!task_id.is_empty());
        assert_eq!(
            read_import_metadata(root.path(), "ts")
                .unwrap()
                .get("task_id")
                .and_then(Value::as_str),
            Some(task_id)
        );
    }

    #[tokio::test]
    async fn criterion_27_meta_and_start_refuse_manifest_duplicate() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("imports/ts/item.txt");
        staged(
            root.path(),
            "ts",
            Value::Object(metadata(path.display().to_string(), "duplicate-hash")),
        );
        write_manifest(&ManifestWriteRequest {
            journal_root: root.path(),
            import_id: "complete",
            source_type: "text",
            source_hash: &SourceHash::new("duplicate-hash".to_owned()),
            entry_count: 1,
            days_affected: &[],
            files_created: &[],
            imported_via: "test",
            link_id: None,
            observer_handle: None,
            raw_retention: None,
        })
        .unwrap();
        let meta = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/meta")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"path":path})).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (_, meta) = response_json(meta).await;
        assert_eq!(meta["reason_code"], "invalid_operation_for_state");
        let start = start_with(
            root.path(),
            &json!({"path":path,"timestamp":"ts"}),
            |_, _, _| Ok(()),
            write_import_metadata,
            |_, _, _, _, _, _| {},
            |_, _| {},
            None,
        );
        let (_, start) = response_json(start).await;
        assert_eq!(start["reason_code"], "invalid_operation_for_state");
    }

    #[tokio::test]
    async fn meta_refuses_running_and_successful_imports_without_retargeting_metadata() {
        let root = TempDir::new().unwrap();
        let valid_pub = json!({
            "schema": "solstone.import.publication.v1",
            "imported_at": 1700000000000_u64,
            "source_type": "image",
            "status": "success",
            "segments": [],
            "files_created": []
        });
        for (timestamp, imported) in [("running", None), ("success", Some(valid_pub))] {
            let path = root.path().join(format!("imports/{timestamp}/item.txt"));
            let mut stored = metadata(path.display().to_string(), "hash");
            stored.insert("upload_timestamp".to_owned(), json!(super::now_ms()));
            if imported.is_none() {
                stored.insert("task_id".to_owned(), json!("active-task"));
            }
            staged(root.path(), timestamp, Value::Object(stored));
            if let Some(imported) = imported {
                fs::write(
                    root.path()
                        .join(format!("imports/{timestamp}/imported.json")),
                    serde_json::to_vec(&imported).unwrap(),
                )
                .unwrap();
            }
            let response = crate::routes(root.path().to_path_buf())
                .oneshot(
                    Request::post("/app/import/api/meta")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::to_vec(&json!({"path":path,"source_hint":"retarget"}))
                                .unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            let (_, body) = response_json(response).await;
            assert_eq!(
                body["reason_code"], "invalid_operation_for_state",
                "{timestamp}"
            );
            assert_eq!(
                read_import_metadata(root.path(), timestamp).unwrap()["source_hint"],
                "right-source",
                "{timestamp} metadata remains unchanged"
            );
        }
    }

    #[tokio::test]
    async fn meta_allows_failed_publication_import_to_update_metadata() {
        let root = TempDir::new().unwrap();
        let timestamp = "failed_import";
        let path = root.path().join(format!("imports/{timestamp}/item.txt"));
        let mut stored = metadata(path.display().to_string(), "hash");
        stored.insert("upload_timestamp".to_owned(), json!(super::now_ms()));
        staged(root.path(), timestamp, Value::Object(stored));

        let failed_pub = json!({
            "schema": "solstone.import.publication.v1",
            "imported_at": 1700000000000_u64,
            "source_type": "image",
            "status": "failure",
            "segments": [],
            "files_created": []
        });
        fs::write(
            root.path()
                .join(format!("imports/{timestamp}/imported.json")),
            serde_json::to_vec(&failed_pub).unwrap(),
        )
        .unwrap();

        let response = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/meta")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"path":path,"source_hint":"retargeted"}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(
            read_import_metadata(root.path(), timestamp).unwrap()["source_hint"],
            "retargeted"
        );
    }

    #[test]
    fn inprocess_producer_request_heartbeats_every_minute() {
        let root = Path::new("/tmp/test-journal");
        let src = Path::new("/tmp/sample.png");
        let req = inprocess_producer_request(
            root,
            src,
            "20260408_120000",
            solstone_core_import::RegistrySource::Image,
            false,
            1,
            None,
            None,
        );
        assert_eq!(req.heartbeat_interval, Some(Duration::from_secs(60)));
    }

    fn assert_no_claimed_imports(root: &Path) {
        assert_save_tmp_clean(root);
        let imports_dir = root.join("imports");
        if imports_dir.exists() {
            let siblings: Vec<_> = fs::read_dir(&imports_dir)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name() != SAVE_TMP_DIR)
                .collect();
            assert!(
                siblings.is_empty(),
                "expected no claimed import directories, found: {:?}",
                siblings.iter().map(|e| e.path()).collect::<Vec<_>>()
            );
        }
    }

    fn multipart_upload_bytes(
        client_item_id: &str,
        filename: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> (String, Vec<u8>) {
        let boundary = "testboundary123";
        let mut body = Vec::new();
        write!(
            &mut body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\n{client_item_id}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .unwrap();
        body.extend_from_slice(bytes);
        write!(&mut body, "\r\n--{boundary}--\r\n").unwrap();
        (boundary.to_owned(), body)
    }

    fn strava_card_upload(client_item_id: &str, bytes: &[u8]) -> (String, Vec<u8>) {
        let boundary = "testboundary123";
        let mut body = Vec::new();
        write!(
            &mut body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"source_hint\"\r\n\r\nstrava\r\n"
        )
        .unwrap();
        let (_, upload) =
            multipart_upload_bytes(client_item_id, "export.zip", "application/zip", bytes);
        body.extend_from_slice(&upload);
        (boundary.to_owned(), body)
    }

    #[tokio::test]
    async fn the_strava_card_takes_a_strava_download_and_takes_it_again() {
        let root = TempDir::new().unwrap();
        let zip = make_zip_bytes("activities.csv", b"Activity ID,Activity Date\n1,2\n");
        for item in ["card-1", "card-2"] {
            let (boundary, body) = strava_card_upload(item, &zip);
            let resp = crate::routes(root.path().to_path_buf())
                .oneshot(
                    Request::post("/app/import/api/save")
                        .header(
                            "content-type",
                            format!("multipart/form-data; boundary={boundary}"),
                        )
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            let (status, json) = response_json(resp).await;
            assert_eq!(status, StatusCode::OK, "{item}: {json}");
            // A finished Strava run records a manifest with the download's hash; the
            // same download is still taken, because importing it again is how a
            // deleted import comes back.
            let download = root.path().join("download.zip");
            std::fs::write(&download, &zip).unwrap();
            let hash = hash_source(&download).unwrap();
            std::fs::remove_file(&download).unwrap();
            let manifest_dir = root.path().join("imports").join("20260801_000000");
            std::fs::create_dir_all(&manifest_dir).unwrap();
            std::fs::write(
                manifest_dir.join("manifest.json"),
                serde_json::json!({"source_hash": hash.as_str(), "source_type": "strava"})
                    .to_string(),
            )
            .unwrap();
            assert!(manifest_exists(root.path(), &hash));
        }
    }

    #[test]
    fn terminal_strava_cleanup_covers_preflight_failure_without_removing_history() {
        let root = TempDir::new().unwrap();
        let id = "20261011_120000";
        let directory = root.path().join("imports").join(id);
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("export.zip");
        std::fs::write(&source, b"download").unwrap();
        write_import_metadata(
            root.path(),
            id,
            json!({"source_hint":"strava","file_path":source})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let snapshot = solstone_core_import::snapshot::create(root.path(), id, &source).unwrap();
        let copy = directory.join(&snapshot.relative);
        snapshot.persist();
        super::discard_terminal_strava_download(root.path(), id);
        assert!(
            source.exists(),
            "pending uploads remain available to preview"
        );
        let attempt =
            solstone_core_import::record_running_attempt(root.path(), id, super::now_ms() as u64)
                .unwrap();
        solstone_core_import::record_unconfirmed_attempt(
            root.path(),
            id,
            attempt.generation,
            super::now_ms() as u64,
            Some("preflight failed".to_owned()),
        )
        .unwrap();
        super::announce_exit(root.path(), id);
        assert!(!source.exists());
        assert!(!copy.exists());
        assert!(directory.join("import.json").exists());
    }

    #[test]
    fn a_strava_upload_left_unstarted_for_an_hour_is_removed() {
        let root = TempDir::new().unwrap();
        let stage = |id: &str, hint: &str, age_secs: u64| {
            let dir = root.path().join("imports").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("export.zip"), b"zip").unwrap();
            std::fs::write(
                dir.join("import.json"),
                serde_json::json!({"source_hint": hint, "original_filename": "export.zip"})
                    .to_string(),
            )
            .unwrap();
            let when = std::time::SystemTime::now() - Duration::from_secs(age_secs);
            filetime::set_file_mtime(&dir, filetime::FileTime::from_system_time(when)).unwrap();
            dir
        };
        let abandoned = stage("20260801_000000", "strava", 2 * 60 * 60);
        let fresh = stage("20260801_010000", "strava", 60);
        let other = stage("20260801_020000", "chatgpt", 2 * 60 * 60);
        super::sweep_abandoned_strava_uploads(root.path());
        assert!(
            !abandoned.exists(),
            "an abandoned Strava upload is not kept"
        );
        assert!(fresh.is_dir(), "an upload still within its hour stays");
        assert!(other.is_dir(), "other sources are untouched");
    }

    fn make_zip_bytes(entry_path: &str, contents: &[u8]) -> Vec<u8> {
        let mut cur = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cur);
            zip.start_file(entry_path, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents).unwrap();
            zip.finish().unwrap();
        }
        cur.into_inner()
    }

    #[tokio::test]
    async fn save_door_strava_refusals_and_controls() {
        let root = TempDir::new().unwrap();
        let zip1 = make_zip_bytes("activities.csv", b"dummy");
        let (boundary, body) =
            multipart_upload_bytes("item-1", "export.zip", "application/zip", &zip1);
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["reason_code"], "strava_download");
        assert_no_claimed_imports(root.path());

        let root = TempDir::new().unwrap();
        let zip2 = make_zip_bytes("export_1/activities.csv", b"dummy");
        let (boundary, body) =
            multipart_upload_bytes("item-2", "export_nested.zip", "application/zip", &zip2);
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["reason_code"], "strava_download");
        assert_no_claimed_imports(root.path());

        let root = TempDir::new().unwrap();
        let (boundary, body) = multipart_upload_bytes(
            "item-3",
            "workouts.csv",
            "text/csv",
            b"Activity ID,Activity Date\n1,2\n",
        );
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["reason_code"], "strava_download");
        assert_no_claimed_imports(root.path());

        let root = TempDir::new().unwrap();
        let (boundary, body) = multipart_upload_bytes(
            "item-4",
            "activities.csv",
            "text/csv",
            b"Title,Body\nNote,Content\n",
        );
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["reason_code"], "strava_download");
        assert_no_claimed_imports(root.path());

        let root = TempDir::new().unwrap();
        let boundary = "paste-boundary";
        let paste_body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\npaste-1\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"text\"\r\n\r\nActivity ID,Activity Date\r\n1,2026-01-01\r\n--{boundary}--\r\n"
        );
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(paste_body, boundary))
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["reason_code"], "strava_download");
        assert_no_claimed_imports(root.path());

        let root = TempDir::new().unwrap();
        let zip_bytes = make_zip_bytes("activities.csv", b"dummy");
        let temp_zip_path = root.path().join("source_for_hash.zip");
        fs::write(&temp_zip_path, &zip_bytes).unwrap();
        let hash = hash_source(&temp_zip_path).unwrap();
        let manifest_dir = root.path().join("imports/20260101_100000");
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(
            manifest_dir.join("manifest.json"),
            serde_json::to_vec(&json!({
                "source_hash": hash.as_str(),
                "source_type": "ics",
                "import_id": "20260101_100000"
            }))
            .unwrap(),
        )
        .unwrap();
        let (boundary, body) =
            multipart_upload_bytes("item-order", "export.zip", "application/zip", &zip_bytes);
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["reason_code"], "strava_download");

        let root = TempDir::new().unwrap();
        let (boundary, body) =
            multipart_upload_bytes("ctrl-upload", "note.txt", "text/plain", b"regular text");
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "staged");
        let ts = json["timestamp"].as_str().unwrap();
        assert!(root.path().join("imports").join(ts).is_dir());

        let root = TempDir::new().unwrap();
        let boundary = "paste-ctrl";
        let paste_body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\npaste-ctrl\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"text\"\r\n\r\nJust some notes\r\n--{boundary}--\r\n"
        );
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(paste_body, boundary))
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "staged");
        let ts = json["timestamp"].as_str().unwrap();
        assert!(root.path().join("imports").join(ts).is_dir());
    }

    #[tokio::test]
    async fn save_path_door_cases() {
        let root = TempDir::new().unwrap();
        let local_zip = root.path().join("owner_export.zip");
        let zip_bytes = make_zip_bytes("activities.csv", b"owner data");
        fs::write(&local_zip, &zip_bytes).unwrap();
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"p1","path":local_zip}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["reason_code"], "strava_download");
        assert_no_claimed_imports(root.path());
        assert_eq!(fs::read(&local_zip).unwrap(), zip_bytes);

        let root = TempDir::new().unwrap();
        let local_dir = root.path().join("strava_folder");
        fs::create_dir_all(&local_dir).unwrap();
        let csv_path = local_dir.join("activities.csv");
        let csv_bytes = b"Activity ID,Activity Date\n1,2026-01-01\n";
        fs::write(&csv_path, csv_bytes).unwrap();
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"p2","path":local_dir}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["reason_code"], "strava_download");
        assert_no_claimed_imports(root.path());
        assert_eq!(fs::read(&csv_path).unwrap(), csv_bytes);

        let root = TempDir::new().unwrap();
        let local_file = root.path().join("notes.txt");
        fs::write(&local_file, b"regular note").unwrap();
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"p3","path":local_file}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "staged");
        let ts = json["timestamp"].as_str().unwrap();
        assert!(root.path().join("imports").join(ts).is_dir());

        let root = TempDir::new().unwrap();
        let vault_dir = root.path().join("vault");
        fs::create_dir_all(vault_dir.join("notes")).unwrap();
        fs::write(vault_dir.join("notes/activities.csv"), b"Title,Body\n1,2\n").unwrap();
        let resp = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"p4","path":vault_dir}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, json) = response_json(resp).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "staged");
        let ts = json["timestamp"].as_str().unwrap();
        assert!(root.path().join("imports").join(ts).is_dir());
    }

    /// The phone's share sheet saves an item, then starts it with `{path, timestamp}` alone: it
    /// has no size step and sends no preview token. The journal previews that start itself and
    /// admits it, for an image (imported in process) and an audio file (queued for the importer),
    /// and either way the import reads the private copy the preview is bound to.
    #[cfg(all(unix, feature = "full-tests"))]
    #[tokio::test]
    async fn a_phone_share_starts_without_a_preview_token() {
        let root = crate::test_support::phase_root("empty");
        let tools = TempDir::new().unwrap();
        let planner = tools.path().join("solstone-core");
        fs::write(
            &planner,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprintf '%s\\n' '{\"schema\":\"solstone.import.size.v1\",\"recordings\":1,\"days\":1,\"skipped\":0,\"other_units\":null}'\n",
        )
        .unwrap();
        fs::set_permissions(&planner, fs::Permissions::from_mode(0o700)).unwrap();
        for (filename, content_type, item) in [
            (
                "IMG_4821.png",
                "image/png",
                "6f1c2a52-0d8e-4c55-9d38-2f3b8a1e0001",
            ),
            (
                "Voice Memo 12.m4a",
                "audio/mp4",
                "6f1c2a52-0d8e-4c55-9d38-2f3b8a1e0002",
            ),
        ] {
            let boundary = "phone-share";
            let body = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"imported_via\"\r\n\r\nmobile_share\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\n{item}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n{filename} bytes\r\n--{boundary}--\r\n"
            );
            let response = crate::routes(root.path().to_path_buf())
                .oneshot(multipart_save(body, boundary))
                .await
                .unwrap();
            let (status, saved) = response_json(response).await;
            assert_eq!(status, StatusCode::OK, "{filename}: {saved}");
            let path = saved["path"].as_str().unwrap().to_owned();
            let timestamp = saved["timestamp"].as_str().unwrap().to_owned();

            // What `start` runs, with the planner, the queue and the in-process importer stood in.
            let data = crate::preview::validate_start(
                root.path(),
                json!({"path": path, "timestamp": timestamp}),
                crate::preview::PreviewPlan::Program(&planner),
            )
            .await
            .unwrap_or_else(|error| panic!("{filename}: {}", error.detail()));
            let planned = fs::read_to_string(tools.path().join("solstone-core.args")).unwrap();
            let planned_as = planned.lines().skip_while(|arg| *arg != "--source").nth(1);
            let expected = (content_type == "image/png").then_some("image");
            assert_eq!(planned_as, expected, "{filename}: {planned}");
            let queued = Rc::new(RefCell::new(Vec::new()));
            let produced = Rc::new(RefCell::new(None));
            let (queue, producer) = (Rc::clone(&queued), Rc::clone(&produced));
            let response = start_with(
                root.path(),
                &data,
                move |_, _, cmd| {
                    *queue.borrow_mut() = cmd.to_vec();
                    Ok(())
                },
                write_import_metadata,
                move |_, source, _, kind, _, _| *producer.borrow_mut() = Some((source, kind)),
                |_, _| {},
                None,
            );
            assert_eq!(response.status(), StatusCode::OK, "{filename}");

            let preview = read_import_metadata(root.path(), &timestamp).unwrap()["preview"].clone();
            let snapshot = root
                .path()
                .join("imports")
                .join(&timestamp)
                .join(preview["relative"].as_str().unwrap());
            assert_ne!(snapshot, Path::new(&path));
            assert_eq!(
                fs::read_to_string(&snapshot).unwrap(),
                format!("{filename} bytes")
            );
            if content_type == "image/png" {
                assert_eq!(
                    *produced.borrow(),
                    Some((snapshot, solstone_core_import::RegistrySource::Image))
                );
                assert!(queued.borrow().is_empty());
            } else {
                let cmd = queued.borrow().clone();
                assert_eq!(cmd[3], snapshot.display().to_string());
                let token = preview["token"].as_str().unwrap();
                assert!(
                    cmd.windows(2)
                        .any(|pair| pair[0] == "--preview-token" && pair[1] == token)
                );
                assert!(produced.borrow().is_none());
            }
        }
        assert!(!root.path().join("chronicle").exists());
    }

    fn composition_fixture_with_id(id: &str) -> (TempDir, Value) {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("imports").join(id);
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("notes.txt");
        fs::write(&source, "note content").unwrap();
        let hash = solstone_core_import::hash_source(&source)
            .unwrap()
            .as_str()
            .to_owned();
        let mut metadata = json!({
            "file_path": source,
            "source_hash": hash,
            "import_id": id,
            "user_timestamp": id,
            "source_timestamp": id,
        })
        .as_object()
        .unwrap()
        .clone();
        solstone_core_import::saved_start::initialize_saved_start(&mut metadata, id).unwrap();
        write_import_metadata(root.path(), id, &metadata).unwrap();
        (
            root,
            json!({"path": source, "timestamp": id, "import_id": id}),
        )
    }

    fn composition_fixture() -> (TempDir, Value) {
        composition_fixture_with_id("20261019_120000")
    }

    fn composition_image_fixture_with_id(id: &str) -> (TempDir, Value) {
        let root = TempDir::new().unwrap();
        let dir = root.path().join("imports").join(id);
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("photo.jpg");
        fs::write(
            &source,
            b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\xff\xdb",
        )
        .unwrap();
        let hash = solstone_core_import::hash_source(&source)
            .unwrap()
            .as_str()
            .to_owned();
        let mut metadata = json!({
            "file_path": source,
            "source_hash": hash,
            "import_id": id,
            "user_timestamp": id,
            "source_timestamp": id,
            "source_hint": "image",
        })
        .as_object()
        .unwrap()
        .clone();
        solstone_core_import::saved_start::initialize_saved_start(&mut metadata, id).unwrap();
        write_import_metadata(root.path(), id, &metadata).unwrap();
        (
            root,
            json!({"path": source, "timestamp": id, "import_id": id}),
        )
    }

    fn immediate_plan(recordings: usize) -> crate::preview::PreviewPlan<'static> {
        crate::preview::PreviewPlan::Stub(std::sync::Arc::new(move |_path, _options, _meta| {
            Box::pin(async move {
                Ok(json!({
                    "schema": "solstone.import.size.v1",
                    "recordings": recordings,
                }))
            })
        }))
    }

    fn mock_send<F>(f: F) -> SendSeam
    where
        F: FnMut(&Path, &str, &[String]) -> Result<(), BusError> + Send + 'static,
    {
        std::sync::Arc::new(std::sync::Mutex::new(Box::new(f)))
    }

    fn noop_send_seams() -> StartSeams {
        StartSeams {
            send: Some(mock_send(|_r, _t, _c| Ok(()))),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn composition_second_queued_start_replays() {
        let (root, data) = composition_fixture();
        let planner_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let send_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let pc = planner_count.clone();
        let plan = crate::preview::PreviewPlan::Stub(std::sync::Arc::new(
            move |_path, _options, _meta| {
                let pc = pc.clone();
                Box::pin(async move {
                    pc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(json!({"schema": "solstone.import.size.v1", "recordings": 1}))
                })
            },
        ));

        let sc = send_count.clone();
        let seams = StartSeams {
            send: Some(mock_send(move |_root, _task_id, _cmd| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            ..Default::default()
        };

        let res1 = run_start(root.path(), data.clone(), plan.clone(), seams.clone()).await;
        let (status1, body1) = response_json(res1).await;
        assert_eq!(status1, StatusCode::OK);
        let task_id1 = body1["task_id"]
            .as_str()
            .expect("task_id present")
            .to_owned();
        assert!(!task_id1.is_empty());

        let res2 = run_start(root.path(), data.clone(), plan, seams).await;
        let (status2, body2) = response_json(res2).await;
        assert_eq!(status2, StatusCode::OK);
        assert_eq!(body2["replay"], true);
        assert_eq!(body2["task_id"], task_id1);
        assert_eq!(body2["import_id"], data["import_id"]);
        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(planner_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn composition_admitted_attempt_replay() {
        let (root, data) = composition_fixture();
        let id = data["import_id"].as_str().unwrap();
        let now = super::now_ms() as u64;
        let attempt =
            solstone_core_import::admit_running_attempt(root.path(), id, now, None).unwrap();

        let plan = immediate_plan(1);
        let seams = noop_send_seams();

        let res = run_start(root.path(), data.clone(), plan, seams).await;
        let (status, body) = response_json(res).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["replay"], true);
        assert_eq!(body["task_id"], id);
        assert_eq!(body["generation"], attempt.generation);
        assert_eq!(body["attempt_id"], attempt.attempt_id);
    }

    #[tokio::test]
    async fn composition_success_without_task_id_replays_queued_shape() {
        let (root, data) = composition_fixture();
        let id = data["import_id"].as_str().unwrap();
        let dir = root.path().join("imports").join(id);
        fs::write(
            dir.join("imported.json"),
            json!({
                "schema": "solstone.import.publication.v1",
                "imported_at": 1700000000000_u64,
                "source_type": "notes",
                "status": "success",
                "segments": [],
                "indexing": {"published": [], "declined": [], "errored": []},
                "day_markers": [],
            })
            .to_string(),
        )
        .unwrap();

        let plan = immediate_plan(1);
        let seams = noop_send_seams();

        let res = run_start(root.path(), data.clone(), plan, seams).await;
        let (status, body) = response_json(res).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["replay"], true);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["task_id"], id);
        assert_eq!(body["import_id"], id);
        assert!(body.get("generation").is_none());
        assert!(body.get("attempt_id").is_none());
    }

    #[tokio::test]
    async fn composition_image_produces_once_and_replays() {
        let (root, data) = composition_image_fixture_with_id("20261019_120000");
        let produce_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let plan = immediate_plan(1);

        let pc = produce_count.clone();
        let seams = StartSeams {
            send: Some(mock_send(|_r, _t, _c| Ok(()))),
            produce: Some(std::sync::Arc::new(std::sync::Mutex::new(Box::new(
                move |_root, _source, _id, _reg_source, _force, _gen| {
                    pc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                },
            )))),
            ..Default::default()
        };

        let res1 = run_start(root.path(), data.clone(), plan.clone(), seams.clone()).await;
        let (status1, body1) = response_json(res1).await;
        assert_eq!(status1, StatusCode::OK);
        let gen1 = body1["generation"].as_u64().expect("generation present");
        let attempt1 = body1["attempt_id"]
            .as_str()
            .expect("attempt_id present")
            .to_owned();
        assert_eq!(produce_count.load(std::sync::atomic::Ordering::SeqCst), 1);

        let res2 = run_start(root.path(), data.clone(), plan, seams).await;
        let (status2, body2) = response_json(res2).await;
        assert_eq!(status2, StatusCode::OK);
        assert_eq!(body2["replay"], true);
        assert_eq!(body2["generation"], gen1);
        assert_eq!(body2["attempt_id"], attempt1);
        assert_eq!(produce_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn composition_upload_timestamp_older_than_bound_projects_running() {
        let (root, data) = composition_fixture();
        let id = data["import_id"].as_str().unwrap();
        let old_time = (super::now_ms() as u64)
            .saturating_sub(solstone_core_import::RUNNING_ATTEMPT_BOUND_MS + 60_000);
        let mut meta = read_import_metadata(root.path(), id).unwrap();
        meta.insert("upload_timestamp".to_owned(), json!(old_time));
        write_import_metadata(root.path(), id, &meta).unwrap();

        let plan = immediate_plan(1);

        let send_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = send_count.clone();
        let seams = StartSeams {
            send: Some(mock_send(move |_root, _task_id, _cmd| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            ..Default::default()
        };

        let res1 = run_start(root.path(), data.clone(), plan.clone(), seams.clone()).await;
        let (status1, _) = response_json(res1).await;
        assert_eq!(status1, StatusCode::OK);

        let projection = solstone_core_import::project_import_result(root.path(), id);
        assert_eq!(
            projection.status,
            solstone_core_import::ProjectionStatus::Running
        );

        let res2 = run_start(root.path(), data.clone(), plan, seams).await;
        let (status2, body2) = response_json(res2).await;
        assert_eq!(status2, StatusCode::OK);
        assert_eq!(body2["replay"], true);
        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn composition_two_tokenless_starts_of_one_import() {
        let (root, data) = composition_fixture();
        let id = data["import_id"].as_str().unwrap().to_owned();

        let send_cmd = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let send_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let notify_a_started = std::sync::Arc::new(tokio::sync::Notify::new());
        let notify_release_a = std::sync::Arc::new(tokio::sync::Notify::new());
        let na = notify_a_started.clone();
        let nr = notify_release_a.clone();

        let plan1 = crate::preview::PreviewPlan::Stub(std::sync::Arc::new(
            move |_path, _options, _meta| {
                let na = na.clone();
                let nr = nr.clone();
                Box::pin(async move {
                    na.notify_one();
                    nr.notified().await;
                    Ok(json!({"schema": "solstone.import.size.v1", "recordings": 1}))
                })
            },
        ));
        let plan2 = immediate_plan(1);

        let sc = send_count.clone();
        let scmd = send_cmd.clone();
        let seams = StartSeams {
            send: Some(mock_send(move |_root, _task_id, cmd| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                *scmd.lock().unwrap() = cmd.to_vec();
                Ok(())
            })),
            ..Default::default()
        };

        let r1 = root.path().to_path_buf();
        let d1 = data.clone();
        let s1 = seams.clone();
        let h1 = tokio::spawn(async move { run_start(&r1, d1, plan1, s1).await });

        notify_a_started.notified().await;

        let r2 = root.path().to_path_buf();
        let d2 = data.clone();
        let s2 = seams;
        let h2 = tokio::spawn(async move { run_start(&r2, d2, plan2, s2).await });

        crate::start_lock::wait_for_start_lock_waiters(root.path(), &id, 1).await;
        assert!(crate::start_lock::start_lock_waiters(root.path(), &id) > 0);

        notify_release_a.notify_one();

        let res1 = h1.await.unwrap();
        let res2 = h2.await.unwrap();
        let (status1, body1) = response_json(res1).await;
        let (status2, body2) = response_json(res2).await;

        assert!(
            !status1.is_client_error(),
            "res1 was client error: {status1}"
        );
        assert!(
            !status2.is_client_error(),
            "res2 was client error: {status2}"
        );
        assert_eq!(status1, StatusCode::OK);
        assert_eq!(status2, StatusCode::OK);

        let replay1 = body1
            .get("replay")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let replay2 = body2
            .get("replay")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert!(replay1 ^ replay2, "exactly one should be replay: true");

        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        let cmd = send_cmd.lock().unwrap().clone();
        let token_arg = cmd
            .windows(2)
            .find(|pair| pair[0] == "--preview-token")
            .map(|pair| pair[1].clone())
            .expect("--preview-token found in cmd");

        let meta = read_import_metadata(root.path(), &id).unwrap();
        let stored_token = meta["preview"]["token"]
            .as_str()
            .expect("preview token stored");
        assert_eq!(stored_token, token_arg);
    }

    #[tokio::test]
    async fn composition_second_import_completes_while_first_blocked_in_planner() {
        let (root1, data1) = composition_fixture_with_id("20261019_100001");
        let id2 = "20261019_100002";
        let dir2 = root1.path().join("imports").join(id2);
        fs::create_dir_all(&dir2).unwrap();
        let source2 = dir2.join("notes.txt");
        fs::write(&source2, "note 2").unwrap();
        let hash2 = solstone_core_import::hash_source(&source2)
            .unwrap()
            .as_str()
            .to_owned();
        let mut metadata2 = json!({
            "file_path": source2,
            "source_hash": hash2,
            "import_id": id2,
            "user_timestamp": id2,
            "source_timestamp": id2,
        })
        .as_object()
        .unwrap()
        .clone();
        solstone_core_import::saved_start::initialize_saved_start(&mut metadata2, id2).unwrap();
        write_import_metadata(root1.path(), id2, &metadata2).unwrap();
        let data2 = json!({"path": source2, "timestamp": id2, "import_id": id2});

        let notify_b_done = std::sync::Arc::new(tokio::sync::Notify::new());
        let notify_a_started = std::sync::Arc::new(tokio::sync::Notify::new());

        let nb = notify_b_done.clone();
        let na = notify_a_started.clone();
        let plan1 = crate::preview::PreviewPlan::Stub(std::sync::Arc::new(
            move |_path, _options, _meta| {
                let na = na.clone();
                let nb = nb.clone();
                Box::pin(async move {
                    na.notify_one();
                    nb.notified().await;
                    Ok(json!({"schema": "solstone.import.size.v1", "recordings": 1}))
                })
            },
        ));

        let plan2 = immediate_plan(1);
        let seams = noop_send_seams();

        let r1 = root1.path().to_path_buf();
        let s1 = seams.clone();
        let h1 = tokio::spawn(async move { run_start(&r1, data1, plan1, s1).await });

        notify_a_started.notified().await;

        let res2 = run_start(root1.path(), data2, plan2, seams).await;
        let (status2, body2) = response_json(res2).await;
        assert_eq!(status2, StatusCode::OK);
        assert_eq!(body2["status"], "ok");

        notify_b_done.notify_one();

        let res1 = h1.await.unwrap();
        let (status1, body1) = response_json(res1).await;
        assert_eq!(status1, StatusCode::OK);
        assert_eq!(body1["status"], "ok");
    }

    #[tokio::test]
    async fn composition_failed_and_unconfirmed_and_timeout_start_again() {
        let plan = immediate_plan(1);

        // Case 1: Failed starts again (new send, no replay)
        let (root1, data1) = composition_fixture_with_id("20261019_100001");
        let id1 = data1["import_id"].as_str().unwrap();
        let now = super::now_ms() as u64;
        let first =
            solstone_core_import::admit_running_attempt(root1.path(), id1, now - 5000, None)
                .unwrap();
        solstone_core_import::record_unconfirmed_attempt(
            root1.path(),
            id1,
            first.generation,
            now - 4000,
            Some(solstone_core_import::IMPORT_FAILED_REASON.to_owned()),
        )
        .unwrap();
        let send_count1 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc1 = send_count1.clone();
        let seams1 = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                sc1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            ..Default::default()
        };
        let res1 = run_start(root1.path(), data1, plan.clone(), seams1).await;
        let (status1, body1) = response_json(res1).await;
        assert_eq!(status1, StatusCode::OK);
        assert_ne!(body1.get("replay"), Some(&Value::Bool(true)));
        assert_eq!(send_count1.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Case 2: Unconfirmed with attempt released and no successful publication starts again
        let (root2, data2) = composition_fixture_with_id("20261019_100002");
        let id2 = data2["import_id"].as_str().unwrap();
        solstone_core_import::admit_running_attempt(root2.path(), id2, now - 5000, None).unwrap();
        solstone_core_import::release_attempt(root2.path(), id2);
        let send_count2 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc2 = send_count2.clone();
        let seams2 = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                sc2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            ..Default::default()
        };
        let res2 = run_start(root2.path(), data2, plan.clone(), seams2).await;
        let (status2, body2) = response_json(res2).await;
        assert_eq!(status2, StatusCode::OK);
        assert_ne!(body2.get("replay"), Some(&Value::Bool(true)));
        assert_eq!(send_count2.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Case 3: The existing retry of a queued task older than bound is preserved.
        let (root3, data3) = composition_fixture_with_id("20261019_100003");
        let id3 = data3["import_id"].as_str().unwrap();
        let old_task_id =
            (now - solstone_core_import::RUNNING_ATTEMPT_BOUND_MS - 10_000).to_string();
        let mut meta3 = read_import_metadata(root3.path(), id3).unwrap();
        meta3.insert("task_id".to_owned(), json!(old_task_id));
        write_import_metadata(root3.path(), id3, &meta3).unwrap();
        let send_count3 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc3 = send_count3.clone();
        let seams3 = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                sc3.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            ..Default::default()
        };
        let res3 = run_start(root3.path(), data3, plan.clone(), seams3).await;
        let (status3, body3) = response_json(res3).await;
        assert_eq!(status3, StatusCode::OK);
        assert_ne!(body3.get("replay"), Some(&Value::Bool(true)));
        assert_eq!(send_count3.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Case 4: force: true on Running is 409 import_preview_required, kind already_running
        let (root4, mut data4) = composition_fixture_with_id("20261019_100004");
        let id4 = data4["import_id"].as_str().unwrap();
        solstone_core_import::admit_running_attempt(root4.path(), id4, now, None).unwrap();
        data4["force"] = json!(true);
        let res4 = run_start(root4.path(), data4, plan, noop_send_seams()).await;
        let (status4, body4) = response_json(res4).await;
        assert_eq!(status4, StatusCode::CONFLICT);
        assert_eq!(body4["reason_code"], "import_preview_required");
        assert_eq!(body4["kind"], "already_running");
    }

    #[tokio::test]
    async fn composition_drop_future_while_hook_blocks_second_replays() {
        let (root, data) = composition_fixture();
        let id = data["import_id"].as_str().unwrap().to_owned();

        let (entry_tx, mut entry_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));

        let send_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = send_count.clone();

        let hook: std::sync::Arc<TaskIdHook> = std::sync::Arc::new(move |op| {
            if let TaskIdOp::Record { .. } = op {
                let _ = entry_tx.send(());
                let _ = release_rx.lock().unwrap().recv();
            }
            Ok(())
        });

        let seams1 = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            task_id_hook: Some(hook),
            ..Default::default()
        };

        let r1 = root.path().to_path_buf();
        let d1 = data.clone();
        let p1 = immediate_plan(1);
        let task1 = tokio::spawn(async move { run_start(&r1, d1, p1, seams1).await });

        entry_rx.recv().await.unwrap();

        let r2 = root.path().to_path_buf();
        let d2 = data.clone();
        let sc2 = send_count.clone();
        let p2 = immediate_plan(1);
        let seams2 = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                sc2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            ..Default::default()
        };
        let task2 = tokio::spawn(async move { run_start(&r2, d2, p2, seams2).await });

        crate::start_lock::wait_for_start_lock_waiters(root.path(), &id, 1).await;
        assert!(crate::start_lock::start_lock_waiters(root.path(), &id) > 0);
        assert!(!task2.is_finished());

        task1.abort();

        let _ = release_tx.send(());

        let res2 = task2.await.unwrap();
        let (status2, body2) = response_json(res2).await;
        assert_eq!(status2, StatusCode::OK);
        assert_eq!(body2["replay"], true);
        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn composition_preview_route_waits_while_tokenless_start_in_planner() {
        let (root, data) = composition_fixture();
        let id = data["import_id"].as_str().unwrap().to_owned();

        let notify_start_in_planner = std::sync::Arc::new(tokio::sync::Notify::new());
        let notify_release_planner = std::sync::Arc::new(tokio::sync::Notify::new());
        let n_in = notify_start_in_planner.clone();
        let n_rel = notify_release_planner.clone();

        let plan = crate::preview::PreviewPlan::Stub(std::sync::Arc::new(
            move |_path, _options, _meta| {
                let n_in = n_in.clone();
                let n_rel = n_rel.clone();
                Box::pin(async move {
                    n_in.notify_one();
                    n_rel.notified().await;
                    Ok(json!({"schema": "solstone.import.size.v1", "recordings": 1}))
                })
            },
        ));

        let sent_token = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let st = sent_token.clone();

        let seams = StartSeams {
            send: Some(mock_send(move |_r, _t, cmd| {
                let token = cmd
                    .windows(2)
                    .find(|p| p[0] == "--preview-token")
                    .map(|p| p[1].clone())
                    .unwrap_or_default();
                *st.lock().unwrap() = token;
                Ok(())
            })),
            ..Default::default()
        };

        let r1 = root.path().to_path_buf();
        let d1 = data.clone();
        let start_task = tokio::spawn(async move { run_start(&r1, d1, plan, seams).await });

        notify_start_in_planner.notified().await;

        let preview_app = crate::routes(root.path().to_path_buf());
        let d2 = data.clone();
        let preview_task = tokio::spawn(async move {
            preview_app
                .oneshot(
                    Request::post("/app/import/api/preview")
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_vec(&d2).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap()
        });

        crate::start_lock::wait_for_start_lock_waiters(root.path(), &id, 1).await;
        assert!(crate::start_lock::start_lock_waiters(root.path(), &id) > 0);

        notify_release_planner.notify_one();

        let start_res = start_task.await.unwrap();
        let (start_status, _) = response_json(start_res).await;
        assert!(
            !start_status.is_client_error(),
            "Start failed with {start_status}"
        );
        assert_eq!(start_status, StatusCode::OK);

        let preview_res = preview_task.await.unwrap();
        let (preview_status, preview_body) = response_json(preview_res).await;
        assert_eq!(preview_status, StatusCode::BAD_REQUEST);
        assert_eq!(preview_body["reason_code"], "import_preview_failed");
        assert_eq!(preview_body["kind"], "already_running");
        assert!(
            preview_body["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("this import is already running")
        );

        let meta = read_import_metadata(root.path(), &id).unwrap();
        assert_eq!(meta["preview"]["token"], *sent_token.lock().unwrap());
    }

    #[tokio::test]
    async fn composition_retry_of_failed_import_blocked_in_send_projects_running() {
        let (root, data) = composition_fixture();
        let id = data["import_id"].as_str().unwrap();
        let now = super::now_ms() as u64;

        let first =
            solstone_core_import::admit_running_attempt(root.path(), id, now - 50_000, None)
                .unwrap();
        solstone_core_import::record_unconfirmed_attempt(
            root.path(),
            id,
            first.generation,
            now - 40_000,
            Some(solstone_core_import::IMPORT_FAILED_REASON.to_owned()),
        )
        .unwrap();

        let old_task_id = (now - 60_000).to_string();
        let mut meta = read_import_metadata(root.path(), id).unwrap();
        meta.insert("task_id".to_owned(), json!(old_task_id));
        write_import_metadata(root.path(), id, &meta).unwrap();

        let plan = immediate_plan(1);

        let (entry_tx, mut entry_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
        let sent_task_id = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let stid = sent_task_id.clone();

        let seams = StartSeams {
            send: Some(mock_send(move |_r, task_id, _cmd| {
                *stid.lock().unwrap() = task_id.to_owned();
                let _ = entry_tx.send(task_id.to_owned());
                let _ = release_rx.lock().unwrap().recv();
                Ok(())
            })),
            ..Default::default()
        };

        let r = root.path().to_path_buf();
        let d = data.clone();
        let h = tokio::spawn(async move { run_start(&r, d, plan, seams).await });

        let received_task_id = entry_rx.recv().await.unwrap();

        let cur_meta = read_import_metadata(root.path(), id).unwrap();
        let on_disk_id = cur_meta.get("task_id").and_then(Value::as_str).unwrap();
        assert_eq!(on_disk_id, received_task_id);
        assert_eq!(on_disk_id, *sent_task_id.lock().unwrap());

        let projection = solstone_core_import::project_import_result(root.path(), id);
        assert_eq!(
            projection.status,
            solstone_core_import::ProjectionStatus::Running
        );

        let _ = release_tx.send(());
        let res = h.await.unwrap();
        let (status, _) = response_json(res).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn composition_unconfirmed_with_publication_success_replays_unless_force() {
        let (root, mut data) = composition_fixture();
        let id = data["import_id"].as_str().unwrap();
        let dir = root.path().join("imports").join(id);
        fs::write(
            dir.join("imported.json"),
            json!({
                "schema": "solstone.import.publication.v1",
                "imported_at": 1700000000000_u64,
                "source_type": "notes",
                "status": "success",
                "segments": [],
                "indexing": {"published": [], "declined": [], "errored": []},
                "day_markers": [],
            })
            .to_string(),
        )
        .unwrap();

        let now = super::now_ms() as u64;
        solstone_core_import::admit_running_attempt(root.path(), id, now - 5000, None).unwrap();
        solstone_core_import::release_attempt(root.path(), id);

        let plan = immediate_plan(1);

        let send_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = send_count.clone();
        let seams = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            ..Default::default()
        };

        let res1 = run_start(root.path(), data.clone(), plan.clone(), seams.clone()).await;
        let (status1, body1) = response_json(res1).await;
        assert_eq!(status1, StatusCode::OK);
        assert_eq!(body1["replay"], true);
        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 0);

        data["force"] = json!(true);
        let res2 = run_start(root.path(), data, plan, seams).await;
        let (status2, body2) = response_json(res2).await;
        assert_eq!(status2, StatusCode::OK);
        assert_ne!(body2.get("replay"), Some(&Value::Bool(true)));
        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn composition_pre_send_record_hook_error_returns_500() {
        let (root, data) = composition_fixture();
        let plan = immediate_plan(1);

        let send_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = send_count.clone();

        let hook: std::sync::Arc<TaskIdHook> = std::sync::Arc::new(move |op| {
            if let TaskIdOp::Record { .. } = op {
                return Err(ImportError::MetadataWriteFailed {
                    path: PathBuf::from("import.json"),
                    message: "simulated disk failure".to_owned(),
                });
            }
            Ok(())
        });

        let seams = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            task_id_hook: Some(hook),
            ..Default::default()
        };

        let res = run_start(root.path(), data, plan, seams).await;
        let (status, body) = response_json(res).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["reason_code"], "import_metadata_failed");
        assert!(body.get("task_id").is_none());
        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn composition_send_fails_503_restores_prior_state() {
        let plan = immediate_plan(1);

        let failing_send_seams = || StartSeams {
            send: Some(mock_send(|_r, _t, _c| Err(BusError::Unavailable))),
            ..Default::default()
        };

        // 1. Pending first start:
        let (root1, data1) = composition_fixture_with_id("20261019_100001");
        let id1 = data1["import_id"].as_str().unwrap().to_owned();
        let res1 = run_start(
            root1.path(),
            data1.clone(),
            plan.clone(),
            failing_send_seams(),
        )
        .await;
        let (status1, body1) = response_json(res1).await;
        assert_eq!(status1, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body1["reason_code"], "import_queue_unreachable");
        let meta1 = read_import_metadata(root1.path(), &id1).unwrap();
        assert!(meta1.get("task_id").is_none());

        let ok_send_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let osc = ok_send_count.clone();
        let ok_seams = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                osc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            ..Default::default()
        };
        let retry_res = run_start(root1.path(), data1, plan.clone(), ok_seams).await;
        let (retry_status, _) = response_json(retry_res).await;
        assert_eq!(retry_status, StatusCode::OK);
        assert_eq!(ok_send_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            solstone_core_import::saved_start::saved_start_state(
                &read_import_metadata(root1.path(), &id1).unwrap(),
                &id1
            ),
            solstone_core_import::saved_start::SavedStartState::Consumed
        );

        // 2. Attempt-based Failed retry:
        let (root2, data2) = composition_fixture_with_id("20261019_100002");
        let id2 = data2["import_id"].as_str().unwrap().to_owned();
        let now = super::now_ms() as u64;
        let attempt =
            solstone_core_import::admit_running_attempt(root2.path(), &id2, now - 50_000, None)
                .unwrap();
        solstone_core_import::record_unconfirmed_attempt(
            root2.path(),
            &id2,
            attempt.generation,
            now - 40_000,
            Some(solstone_core_import::IMPORT_FAILED_REASON.to_owned()),
        )
        .unwrap();
        solstone_core_import::record_queued_task(root2.path(), &id2, "prior_task_id").unwrap();

        let res2 = run_start(root2.path(), data2, plan.clone(), failing_send_seams()).await;
        let (status2, _) = response_json(res2).await;
        assert_eq!(status2, StatusCode::SERVICE_UNAVAILABLE);
        let meta2_after = read_import_metadata(root2.path(), &id2).unwrap();
        assert_eq!(
            meta2_after.get("task_id").and_then(Value::as_str),
            Some("prior_task_id")
        );

        // 3. Legacy timeout-Failed row (old numeric task id, no attempt):
        let (root3, data3) = composition_fixture_with_id("20261019_100003");
        let id3 = data3["import_id"].as_str().unwrap().to_owned();
        let old_numeric_id =
            (now - solstone_core_import::RUNNING_ATTEMPT_BOUND_MS - 20_000).to_string();
        let mut meta3 = read_import_metadata(root3.path(), &id3).unwrap();
        meta3.insert("task_id".to_owned(), json!(old_numeric_id));
        write_import_metadata(root3.path(), &id3, &meta3).unwrap();

        let res3 = run_start(root3.path(), data3, plan.clone(), failing_send_seams()).await;
        let (status3, _) = response_json(res3).await;
        assert_eq!(status3, StatusCode::SERVICE_UNAVAILABLE);
        let meta3_after = read_import_metadata(root3.path(), &id3).unwrap();
        assert_eq!(
            meta3_after.get("task_id").and_then(Value::as_str),
            Some(old_numeric_id.as_str())
        );

        // 4. If on-disk id was changed before restore, that id is left as changed:
        let (root4, data4) = composition_fixture_with_id("20261019_100004");
        let id4 = data4["import_id"].as_str().unwrap().to_owned();
        let r4_path = root4.path().to_path_buf();
        let id4_str = id4.clone();

        let hook: std::sync::Arc<TaskIdHook> = std::sync::Arc::new(move |op| {
            if let TaskIdOp::Restore { .. } = op {
                solstone_core_import::record_queued_task(&r4_path, &id4_str, "externally_changed")
                    .unwrap();
            }
            Ok(())
        });

        let mut seams4 = failing_send_seams();
        seams4.task_id_hook = Some(hook);

        let res4 = run_start(root4.path(), data4, plan, seams4).await;
        let (status4, _) = response_json(res4).await;
        assert_eq!(status4, StatusCode::SERVICE_UNAVAILABLE);
        let meta4_after = read_import_metadata(root4.path(), &id4).unwrap();
        assert_eq!(
            meta4_after.get("task_id").and_then(Value::as_str),
            Some("externally_changed")
        );
    }

    #[tokio::test]
    async fn composition_write_closure_failure_returns_200_and_replays() {
        let (root, data) = composition_fixture();
        let plan = immediate_plan(1);

        let send_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = send_count.clone();

        let seams = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            write: Some(std::sync::Arc::new(std::sync::Mutex::new(Box::new(
                |_r, _t, _m| {
                    Err(ImportError::MetadataWriteFailed {
                        path: PathBuf::from("import.json"),
                        message: "disk full".to_owned(),
                    })
                },
            )))),
            ..Default::default()
        };

        let res1 = run_start(root.path(), data.clone(), plan.clone(), seams.clone()).await;
        let (status1, body1) = response_json(res1).await;
        assert_eq!(status1, StatusCode::OK);
        let task_id1 = body1["task_id"].as_str().expect("task_id present");
        assert!(!task_id1.is_empty());
        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 1);

        let res2 = run_start(root.path(), data, plan, seams).await;
        let (status2, body2) = response_json(res2).await;
        assert_eq!(status2, StatusCode::OK);
        assert_eq!(body2["replay"], true);
        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn composition_send_fails_and_restore_hook_fails_returns_503() {
        let (root, data) = composition_fixture();
        let plan = immediate_plan(1);

        let send_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = send_count.clone();

        let hook: std::sync::Arc<TaskIdHook> = std::sync::Arc::new(|op| {
            if let TaskIdOp::Restore { .. } = op {
                return Err(ImportError::MetadataWriteFailed {
                    path: PathBuf::from("import.json"),
                    message: "restore failed".to_owned(),
                });
            }
            Ok(())
        });

        let seams = StartSeams {
            send: Some(mock_send(move |_r, _t, _c| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(BusError::Unavailable)
            })),
            task_id_hook: Some(hook),
            ..Default::default()
        };

        let res = run_start(root.path(), data, plan, seams).await;
        let (status, body) = response_json(res).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["reason_code"], "import_queue_unreachable");
        assert_eq!(send_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn composition_refusal_kinds_and_error_codes() {
        let (root, data) = composition_fixture();

        // 1. Missing path: 409 import_preview_required, kind invalid_request
        let mut missing_path_data = data.clone();
        missing_path_data.as_object_mut().unwrap().remove("path");
        let plan_ok = immediate_plan(1);
        let res1 = run_start(
            root.path(),
            missing_path_data,
            plan_ok.clone(),
            noop_send_seams(),
        )
        .await;
        let (status1, body1) = response_json(res1).await;
        assert_eq!(status1, StatusCode::CONFLICT);
        assert_eq!(body1["reason_code"], "import_preview_required");
        assert_eq!(body1["kind"], "invalid_request");

        // 2. Nothing to import: kind nothing_to_import
        let plan_zero = immediate_plan(0);
        let res2 = run_start(root.path(), data.clone(), plan_zero, noop_send_seams()).await;
        let (status2, body2) = response_json(res2).await;
        assert_eq!(status2, StatusCode::CONFLICT);
        assert_eq!(body2["reason_code"], "import_preview_required");
        assert_eq!(body2["kind"], "nothing_to_import");

        // 3. Missing staged import: kind not_found
        let missing_staged_data = json!({
            "path": "/imports/nonexistent/notes.txt",
            "timestamp": "20261019_130000",
            "import_id": "20261019_130000",
        });
        let res3 = run_start(root.path(), missing_staged_data, plan_ok, noop_send_seams()).await;
        let (status3, body3) = response_json(res3).await;
        assert_eq!(status3, StatusCode::CONFLICT);
        assert_eq!(body3["reason_code"], "import_preview_required");
        assert_eq!(body3["kind"], "not_found");

        // 4. Planner stub returning PlanFailed("marker-solstone-preview"): marker in detail not error
        let plan_fail =
            crate::preview::PreviewPlan::Stub(std::sync::Arc::new(|_path, _options, _meta| {
                Box::pin(async {
                    Err(crate::preview::PreviewError::PlanFailed(
                        "marker-solstone-preview".to_owned(),
                    ))
                })
            }));
        let res4 = run_start(root.path(), data, plan_fail, noop_send_seams()).await;
        let (status4, body4) = response_json(res4).await;
        assert_eq!(status4, StatusCode::CONFLICT);
        assert_eq!(body4["reason_code"], "import_preview_required");
        assert_eq!(body4["kind"], "plan_failed");
        let detail = body4["detail"].as_str().unwrap_or_default();
        let error = body4["error"].as_str().unwrap_or_default();
        assert!(detail.contains("marker-solstone-preview"));
        assert!(!error.contains("marker-solstone-preview"));
    }

    #[tokio::test]
    async fn composition_save_and_save_path_refusal_details_and_kinds() {
        // 1. save (/app/import/api/save):
        let root = TempDir::new().unwrap();
        let b = "boundary18";
        let body1 = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nitem1\r\n--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"doc1.txt\"\r\nContent-Type: text/plain\r\n\r\ncontent A\r\n--{b}--\r\n"
        );
        let resp1 = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(body1, b))
            .await
            .unwrap();
        let (s1, _) = response_json(resp1).await;
        assert_eq!(s1, StatusCode::OK);

        let body2 = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nitem1\r\n--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"doc1.txt\"\r\nContent-Type: text/plain\r\n\r\ncontent B different\r\n--{b}--\r\n"
        );
        let resp2 = crate::routes(root.path().to_path_buf())
            .oneshot(multipart_save(body2, b))
            .await
            .unwrap();
        let (s2, b2) = response_json(resp2).await;
        assert_eq!(s2, StatusCode::BAD_REQUEST);
        assert_eq!(b2["reason_code"], "invalid_operation_for_state");
        assert_eq!(b2["kind"], "client_item_conflict");
        assert_eq!(
            b2["detail"],
            "client_item_id already staged for different content; use a new client_item_id or re-fetch the existing item"
        );

        let root_ai = TempDir::new().unwrap();
        let body_ai_1 = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nitem_first\r\n--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"doc.txt\"\r\nContent-Type: text/plain\r\n\r\nalready imported content\r\n--{b}--\r\n"
        );
        let resp_ai_1 = crate::routes(root_ai.path().to_path_buf())
            .oneshot(multipart_save(body_ai_1, b))
            .await
            .unwrap();
        let (_, b_ai_1) = response_json(resp_ai_1).await;
        let stamp = b_ai_1["timestamp"].as_str().unwrap();
        let hash_ai = read_import_metadata(root_ai.path(), stamp).unwrap()["source_hash"]
            .as_str()
            .unwrap()
            .to_owned();

        write_manifest(&ManifestWriteRequest {
            journal_root: root_ai.path(),
            import_id: "complete",
            source_type: "text",
            source_hash: &SourceHash::new(hash_ai),
            entry_count: 1,
            days_affected: &[],
            files_created: &[],
            imported_via: "test",
            link_id: None,
            observer_handle: None,
            raw_retention: None,
        })
        .unwrap();

        let body_ai = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nitem_ai\r\n--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"doc.txt\"\r\nContent-Type: text/plain\r\n\r\nalready imported content\r\n--{b}--\r\n"
        );
        let resp_ai = crate::routes(root_ai.path().to_path_buf())
            .oneshot(multipart_save(body_ai, b))
            .await
            .unwrap();
        let (s_ai, b_ai) = response_json(resp_ai).await;
        assert_eq!(s_ai, StatusCode::BAD_REQUEST);
        assert_eq!(b_ai["reason_code"], "invalid_operation_for_state");
        assert_eq!(b_ai["kind"], "already_imported");
        assert_eq!(b_ai["detail"], "content already imported");

        // 2. save_path (/app/import/api/save-path):
        let root_sp = TempDir::new().unwrap();
        let file_a = root_sp.path().join("a.txt");
        let file_b = root_sp.path().join("b.txt");
        fs::write(&file_a, "content A").unwrap();
        fs::write(&file_b, "content B different").unwrap();

        let resp_spa = crate::routes(root_sp.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"p1","path":file_a})).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (s_spa, _) = response_json(resp_spa).await;
        assert_eq!(s_spa, StatusCode::OK);

        let resp_spb = crate::routes(root_sp.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"p1","path":file_b})).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (s_spb, b_spb) = response_json(resp_spb).await;
        assert_eq!(s_spb, StatusCode::BAD_REQUEST);
        assert_eq!(b_spb["reason_code"], "invalid_operation_for_state");
        assert_eq!(b_spb["kind"], "client_item_conflict");
        assert_eq!(
            b_spb["detail"],
            "client_item_id already staged for different content; use a new client_item_id or re-fetch the existing item"
        );

        let root_spai = TempDir::new().unwrap();
        let file_ai = root_spai.path().join("ai.txt");
        fs::write(&file_ai, "manifest content").unwrap();
        let hash_spai = solstone_core_import::hash_source(&file_ai)
            .unwrap()
            .as_str()
            .to_owned();
        write_manifest(&ManifestWriteRequest {
            journal_root: root_spai.path(),
            import_id: "complete_sp",
            source_type: "text",
            source_hash: &SourceHash::new(hash_spai),
            entry_count: 1,
            days_affected: &[],
            files_created: &[],
            imported_via: "test",
            link_id: None,
            observer_handle: None,
            raw_retention: None,
        })
        .unwrap();

        let resp_spai = crate::routes(root_spai.path().to_path_buf())
            .oneshot(
                Request::post("/app/import/api/save-path")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({"client_item_id":"p_ai","path":file_ai}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (s_spai, b_spai) = response_json(resp_spai).await;
        assert_eq!(s_spai, StatusCode::BAD_REQUEST);
        assert_eq!(b_spai["reason_code"], "invalid_operation_for_state");
        assert_eq!(b_spai["kind"], "already_imported");
        assert_eq!(b_spai["detail"], "content already imported");
    }
}
