// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Json, Multipart, State},
    http::StatusCode,
    response::Response,
};
use chrono::Local;
use serde_json::{Map, Value, json};
use solstone_core_import::{
    ImportError, ImportMetadata, SourceHash, find_manifest_by_hash, hash_source,
    read_import_metadata, relocate_import, write_import_metadata,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, atomic_replace, contained_path, create_directory_with_mode, install_file,
};
use tempfile::NamedTempFile;

use crate::{
    AppState,
    callosum::{BusError, request_required},
    http::{error, import_not_found, json as json_response},
    multipart, save_stream,
};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_millis() as i64
}

fn import_timestamp() -> String {
    Local::now().format("%Y%m%d_%H%M%S").to_string()
}

fn filename_timestamp(filename: &str) -> Option<String> {
    let bytes = filename.as_bytes();
    for start in 0..bytes.len().saturating_sub(14) {
        let candidate = &bytes[start..start + 15];
        if candidate.get(8) != Some(&b'_')
            || !candidate[..8].iter().all(u8::is_ascii_digit)
            || !candidate[9..].iter().all(u8::is_ascii_digit)
        {
            continue;
        }
        let candidate = std::str::from_utf8(candidate).expect("ASCII timestamp candidate");
        if chrono::NaiveDateTime::parse_from_str(candidate, "%Y%m%d_%H%M%S").is_ok() {
            return Some(candidate.to_owned());
        }
    }
    None
}

const METADATA_CREATION_FIELDS: [&str; 8] = [
    "SubSecCreateDate",
    "SubSecDateTimeOriginal",
    "CreateDate",
    "CreationDate",
    "DateTimeOriginal",
    "MediaCreateDate",
    "TrackCreateDate",
    "ContentCreateDate",
];
const EXIFTOOL_TIMEOUT: Duration = Duration::from_secs(2);

/// Planned metadata-helper invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataCommandPlan {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub path: PathBuf,
    pub timeout: Duration,
}

/// Result of running one metadata-helper plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetadataCommandOutcome {
    Completed(Vec<u8>),
    Unavailable,
    TimedOut,
}

fn metadata_command_plan(path: &Path) -> MetadataCommandPlan {
    MetadataCommandPlan {
        program: PathBuf::from("exiftool"),
        args: vec![OsString::from("-json"), path.as_os_str().to_os_string()],
        path: path.to_path_buf(),
        timeout: EXIFTOOL_TIMEOUT,
    }
}

fn parse_metadata_timestamp(stdout: &[u8]) -> Option<String> {
    let parsed = serde_json::from_slice::<Value>(stdout).ok()?;
    metadata_timestamp_from_fields(parsed.as_array()?.first()?.as_object()?)
}

/// Run the planned metadata helper and classify the process outcome.
pub fn run_metadata_command(plan: &MetadataCommandPlan) -> MetadataCommandOutcome {
    let mut command = Command::new(&plan.program);
    for arg in &plan.args {
        command.arg(arg);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return MetadataCommandOutcome::Unavailable,
    };
    let Some(deadline) = Instant::now().checked_add(plan.timeout) else {
        let _ = child.kill();
        let _ = child.wait();
        return MetadataCommandOutcome::Unavailable;
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return MetadataCommandOutcome::Unavailable;
                }
                let mut stdout = Vec::new();
                match child.stdout.take() {
                    Some(mut pipe) => match pipe.read_to_end(&mut stdout) {
                        Ok(_) => return MetadataCommandOutcome::Completed(stdout),
                        Err(_) => return MetadataCommandOutcome::Unavailable,
                    },
                    None => return MetadataCommandOutcome::Unavailable,
                }
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return MetadataCommandOutcome::TimedOut;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return MetadataCommandOutcome::Unavailable;
            }
        }
    }
}

#[cfg(test)]
fn metadata_timestamp_with_runner(
    path: &Path,
    mut runner: impl FnMut(&MetadataCommandPlan) -> MetadataCommandOutcome,
) -> Option<String> {
    match runner(&metadata_command_plan(path)) {
        MetadataCommandOutcome::Completed(output) => parse_metadata_timestamp(&output),
        MetadataCommandOutcome::Unavailable | MetadataCommandOutcome::TimedOut => None,
    }
}

fn metadata_timestamp(value: &str) -> Option<String> {
    let mut normalized = value.trim().replace('T', " ");
    if normalized.starts_with("0000:") || normalized.starts_with("0000-") {
        return None;
    }
    if normalized.as_bytes().get(4) == Some(&b':') && normalized.as_bytes().get(7) == Some(&b':') {
        normalized.replace_range(7..8, "-");
        normalized.replace_range(4..5, "-");
    }
    if normalized.ends_with('Z') {
        normalized.pop();
        normalized.push_str("+00:00");
    }
    let local = chrono::DateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S%.f%:z")
        .ok()
        .map(|value| value.with_timezone(&Local).naive_local())
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S%.f").ok()
        })?;
    Some(local.format("%Y%m%d_%H%M%S").to_string())
}

fn metadata_timestamp_from_fields(metadata: &Map<String, Value>) -> Option<String> {
    let timestamps = METADATA_CREATION_FIELDS
        .iter()
        .filter_map(|field| metadata.get(*field).and_then(Value::as_str))
        .filter_map(metadata_timestamp)
        .collect::<BTreeSet<_>>();
    (timestamps.len() == 1)
        .then(|| timestamps.into_iter().next())
        .flatten()
}

fn metadata_timestamp_from_file(path: &Path) -> Option<String> {
    match run_metadata_command(&metadata_command_plan(path)) {
        MetadataCommandOutcome::Completed(output) => parse_metadata_timestamp(&output),
        MetadataCommandOutcome::Unavailable | MetadataCommandOutcome::TimedOut => None,
    }
}

fn deterministic_timestamp(path: &Path, filename: &str) -> Option<String> {
    let from_filename = filename_timestamp(filename);
    let from_metadata = metadata_timestamp_from_file(path);
    match (from_filename, from_metadata) {
        (Some(filename), Some(metadata)) if filename != metadata => None,
        (Some(filename), _) => Some(filename),
        (_, Some(metadata)) => Some(metadata),
        (None, None) => None,
    }
}

fn timestamp_for_upload(
    data: &Value,
    path: &Path,
    filename: &str,
) -> (String, &'static str, bool, Option<&'static str>) {
    if let Some(timestamp) = deterministic_timestamp(path, filename) {
        return (timestamp, "deterministic", false, None);
    }
    if form_bool(data, "deterministic_only") {
        return (
            import_timestamp(),
            "upload_fallback",
            false,
            Some("no_deterministic_match"),
        );
    }
    // The native web adapter has no model transport of its own.  Its import core
    // treats an unavailable model as a no-match, just as the reference does.
    (
        import_timestamp(),
        "upload_fallback",
        true,
        Some("model_no_match"),
    )
}

fn form_bool(data: &Value, key: &str) -> bool {
    matches!(
        data.get(key).and_then(Value::as_str).map(str::trim),
        Some(value)
            if value.eq_ignore_ascii_case("true")
                || value == "1"
                || value.eq_ignore_ascii_case("yes")
    )
}

fn clean_optional(value: Option<&Value>) -> Option<String> {
    value
        .map(|value| match value {
            Value::String(value) => value.trim().to_owned(),
            other => other.to_string().trim_matches('"').trim().to_owned(),
        })
        .filter(|value| !value.is_empty())
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

fn source_for(filename: &str, content_type: Option<&str>) -> &'static str {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "tiff"
    ) {
        "image"
    } else if matches!(extension.as_str(), "pdf" | "doc" | "docx") {
        "document"
    } else if content_type.is_some_and(|value| value.starts_with("audio/")) {
        "audio"
    } else {
        "text"
    }
}

fn safe_filename(filename: &str) -> Option<String> {
    Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && *value != "." && *value != "..")
        .map(ToOwned::to_owned)
}

fn failure(status: StatusCode, reason: &str, message: &str, detail: impl Into<String>) -> Response {
    error(status, message, reason, detail.into())
}

fn metadata_failed(detail: impl Into<String>) -> Response {
    failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "import_metadata_failed",
        "I couldn't update that import metadata.",
        detail,
    )
}

fn missing(detail: impl Into<String>) -> Response {
    failure(
        StatusCode::BAD_REQUEST,
        "missing_required_field",
        "I couldn't find a required field.",
        detail,
    )
}

fn invalid_state(detail: impl Into<String>) -> Response {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid_operation_for_state",
        "I couldn't take that action in the current state.",
        detail,
    )
}

fn manifest_exists(root: &Path, hash: &SourceHash) -> bool {
    find_manifest_by_hash(root, hash)
        .ok()
        .and_then(|scan| scan.found)
        .is_some()
}

fn import_is_running_or_successful(
    root: &Path,
    timestamp: &str,
    metadata: &ImportMetadata,
) -> bool {
    let imported = root.join("imports").join(timestamp).join("imported.json");
    if let Ok(bytes) = std::fs::read(imported)
        && let Ok(result) = serde_json::from_slice::<Value>(&bytes)
    {
        return result.get("error").is_none_or(Value::is_null);
    }
    if metadata.get("task_id").and_then(Value::as_str).is_none() {
        return false;
    }
    let uploaded = metadata
        .get("upload_timestamp")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_ms);
    now_ms().saturating_sub(uploaded) <= 3_600_000
}

fn summary(metadata: &ImportMetadata) -> Value {
    json!({
        "schema_version": 1,
        "status": "staged",
        "replay": false,
        "path": metadata.get("file_path").cloned().unwrap_or(Value::String(String::new())),
        "timestamp": metadata.get("user_timestamp").cloned().unwrap_or(Value::String(String::new())),
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

fn replay_summary(metadata: &ImportMetadata) -> Value {
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
        StatusCode::PAYLOAD_TOO_LARGE,
        "multipart_part_too_large",
        "I couldn't bring in that file because it's too large.",
        detail,
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
            StatusCode::BAD_REQUEST,
            "ingest_no_files",
            "I couldn't find any files to bring in.",
            "cannot read multipart part",
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
            json!(Local::now().naive_local().to_string()),
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
                    StatusCode::BAD_REQUEST,
                    "ingest_no_files",
                    "I couldn't find any files to bring in.",
                    "invalid multipart body",
                );
            }
        };
        if part_count == multipart::MAX_PARTS {
            return failure(
                StatusCode::BAD_REQUEST,
                "ingest_no_files",
                "I couldn't find any files to bring in.",
                "too many multipart parts",
            );
        }
        part_count += 1;
        if field.headers().len() > multipart::MAX_HEADERS {
            return failure(
                StatusCode::BAD_REQUEST,
                "ingest_no_files",
                "I couldn't find any files to bring in.",
                "too many multipart headers",
            );
        }
        let filename = field.file_name().map(ToOwned::to_owned);
        if filename
            .as_ref()
            .is_some_and(|value| value.len() > multipart::MAX_FILENAME_BYTES)
        {
            return failure(
                StatusCode::BAD_REQUEST,
                "ingest_no_files",
                "I couldn't find any files to bring in.",
                "multipart filename is too long",
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
                        StatusCode::BAD_REQUEST,
                        "ingest_no_files",
                        "I couldn't find any files to bring in.",
                        detail,
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
                    StatusCode::BAD_REQUEST,
                    "ingest_no_files",
                    "I couldn't find any files to bring in.",
                    "No input",
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
                StatusCode::BAD_REQUEST,
                "ingest_no_files",
                "I couldn't find any files to bring in.",
                "No input",
            );
        }
    };
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
            return json_response(StatusCode::OK, replay_summary(&existing));
        }
        return invalid_state(
            "client_item_id already staged for different content; use a new client_item_id or re-fetch the existing item",
        );
    }
    if manifest_exists(&state.root, &source_hash) {
        return invalid_state("content already imported");
    }
    if let Some(existing) = staged_by_source_hash(&state.root, &source_hash) {
        return json_response(StatusCode::OK, summary(&existing));
    }
    let original_for_timestamp = match &incoming {
        Incoming::File {
            original_filename, ..
        } => original_filename.as_str(),
        Incoming::Paste { .. } => "paste.txt",
    };
    let (timestamp, method, model_called, no_match_reason) =
        timestamp_for_upload(&data, &source_path, original_for_timestamp);
    let (file_path, original_filename, mime_type, file_size) = match incoming {
        Incoming::File {
            filename,
            original_filename,
            mime_type,
            counted,
        } => {
            let target = match contained_import_file(&state.root, &timestamp, &filename) {
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
            let file_path = match stage_bytes(&state.root, &timestamp, "paste.txt", &bytes) {
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
        timestamp: &timestamp,
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
    metadata.insert("file_size".to_owned(), json!(file_size));
    metadata.insert(
        "timestamp_detection_model_called".to_owned(),
        json!(model_called),
    );
    metadata.insert(
        "timestamp_detection_no_match_reason".to_owned(),
        no_match_reason.map_or(Value::Null, |reason| json!(reason)),
    );
    if let Err(error) = contained_import_file(&state.root, &timestamp, "import.json") {
        return metadata_failed(error.to_string());
    }
    if let Err(error) = write_import_metadata(&state.root, &timestamp, &metadata) {
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
        return failure(
            StatusCode::NOT_FOUND,
            "file_not_found",
            "I couldn't find that file.",
            format!("Path not found: {local_path}"),
        );
    }
    let source_hash = match hash_source(local) {
        Ok(hash) => hash,
        Err(error) => return metadata_failed(error.to_string()),
    };
    if manifest_exists(&state.root, &source_hash) {
        return invalid_state("content already imported");
    }
    let timestamp = import_timestamp();
    let original_filename = local
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_owned();
    let metadata = staged_metadata(StagedMetadata {
        timestamp: &timestamp,
        original_filename: &original_filename,
        file_path: local_path,
        source_hash,
        source: source_for(&original_filename, None),
        mime_type: None,
        client_item_id,
        data: &data,
        is_local_path: true,
        method: "path_fallback",
    });
    if let Err(error) = contained_import_file(&state.root, &timestamp, "import.json") {
        return metadata_failed(error.to_string());
    }
    if let Err(error) = write_import_metadata(&state.root, &timestamp, &metadata) {
        return metadata_failed(format!("Failed to write metadata: {error}"));
    }
    json_response(StatusCode::OK, summary(&metadata))
}

pub(crate) async fn meta(State(state): State<AppState>, Json(data): Json<Value>) -> Response {
    let raw_path = text_value(&data, "path");
    if raw_path.is_empty() {
        return missing("Missing import path");
    }
    let Some(timestamp) = Path::new(&raw_path)
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|value| value.to_str())
    else {
        return import_not_found("Import metadata not found");
    };
    if contained_import_file(&state.root, timestamp, "import.json").is_err() {
        return import_not_found("Import metadata not found");
    }
    let mut metadata = match read_import_metadata(&state.root, timestamp) {
        Ok(metadata) => metadata,
        Err(_) => return import_not_found("Import metadata not found"),
    };
    if import_is_running_or_successful(&state.root, timestamp, &metadata) {
        return invalid_state("import already started or processed");
    }
    if metadata
        .get("source_hash")
        .and_then(Value::as_str)
        .is_some_and(|hash| manifest_exists(&state.root, &SourceHash::new(hash.to_owned())))
    {
        return invalid_state("content already imported");
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
            StatusCode::CONFLICT,
            "import_conflict",
            "I couldn't start that import because it already exists.",
            format!("Import already exists for timestamp {timestamp}"),
        ),
        error => metadata_failed(format!("Failed to rename import directory: {error}")),
    }
}

fn queue_error() -> Response {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "import_queue_unreachable",
        "your journal's background service isn't running. start it, then try again.",
        "your journal's background service isn't running. start it, then try again.",
    )
}

fn metadata_error_with_task(task_id: &str, detail: String) -> Response {
    json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({"task_id":task_id,"error":"I couldn't update that import metadata.","reason_code":"import_metadata_failed","detail":detail}),
    )
}

fn command(path: &str, timestamp: &str, metadata: &ImportMetadata, force: bool) -> Vec<String> {
    // This request path is an intentionally unvalidated trust boundary: iOS echoes metadata.file_path.
    let mut cmd = vec![
        "journal".to_owned(),
        "importer".to_owned(),
        path.to_owned(),
        timestamp.to_owned(),
    ];
    if let Some(setting) = metadata
        .get("setting")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        cmd.extend(["--setting".to_owned(), setting.to_owned()]);
    }
    if let Some(source_hint) = clean_optional(metadata.get("source_hint")) {
        cmd.extend(["--source".to_owned(), source_hint]);
    }
    if force {
        cmd.push("--force".to_owned());
    }
    cmd
}

pub(crate) async fn start(State(state): State<AppState>, Json(data): Json<Value>) -> Response {
    start_with(&state.root, &data, request_required, write_import_metadata)
}

fn start_with<S, W>(root: &Path, data: &Value, mut send: S, mut write: W) -> Response
where
    S: FnMut(&Path, &str, &[String]) -> Result<(), BusError>,
    W: FnMut(&Path, &str, &ImportMetadata) -> Result<PathBuf, ImportError>,
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
    let imports = root.join("imports");
    let file_path = Path::new(path);
    let is_local_path = !file_path.starts_with(&imports);
    let original_timestamp = if is_local_path {
        timestamp
    } else {
        file_path
            .parent()
            .and_then(|value| value.file_name())
            .and_then(|value| value.to_str())
            .unwrap_or(timestamp)
    };
    if contained_import_file(root, original_timestamp, "import.json").is_err() {
        return import_not_found(&format!(
            "Import metadata not found for {original_timestamp}"
        ));
    }
    let mut metadata = match read_import_metadata(root, original_timestamp) {
        Ok(metadata) => metadata,
        Err(_) => {
            return import_not_found(&format!(
                "Import metadata not found for {original_timestamp}"
            ));
        }
    };
    if metadata
        .get("source_hash")
        .and_then(Value::as_str)
        .is_some_and(|hash| manifest_exists(root, &SourceHash::new(hash.to_owned())))
    {
        return invalid_state("content already imported; will not start");
    }
    let mut command_path = path.to_owned();
    if !is_local_path && original_timestamp != timestamp {
        if let Err(error) = contained_import_file(root, timestamp, "import.json") {
            return metadata_failed(error.to_string());
        }
        let new_directory = match relocate_import(root, original_timestamp, timestamp) {
            Ok(path) => path,
            Err(error) => return relocation_error(error, original_timestamp, timestamp),
        };
        let filename = file_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        command_path = new_directory.join(filename).display().to_string();
        metadata.insert("file_path".to_owned(), json!(command_path));
        if let Err(error) = write(root, timestamp, &metadata) {
            return metadata_failed(format!("Failed to update file path in metadata: {error}"));
        }
    }
    let task_id = now_ms().to_string();
    let cmd = command(&command_path, timestamp, &metadata, force);
    if send(root, &task_id, &cmd).is_err() {
        return queue_error();
    }
    metadata.insert("task_id".to_owned(), json!(task_id));
    metadata.insert(
        "source_hint".to_owned(),
        clean_optional(metadata.get("source_hint")).map_or(Value::Null, Value::String),
    );
    if let Err(error) = write(root, timestamp, &metadata) {
        return metadata_error_with_task(
            &task_id,
            format!(
                "Supervisor accepted task {task_id}, but metadata could not be updated: {error}"
            ),
        );
    }
    json_response(StatusCode::OK, json!({"status":"ok","task_id":task_id}))
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell, ffi::OsString, fs, os::unix::fs::PermissionsExt, rc::Rc, time::Duration,
    };

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        response::Response,
    };
    use serde_json::{Map, Value, json};
    use sha2::{Digest, Sha256};
    use solstone_core_import::{
        ImportError, ImportMetadata, ManifestWriteRequest, SourceHash, hash_source,
        read_import_metadata, relocate_import, write_import_metadata, write_manifest,
    };
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::{BusError, SAVE_TMP_DIR, command, start_with};
    use crate::multipart;

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
        assert_eq!(hash_source(&owner).unwrap(), before);
        let mut import_files = Vec::new();
        files_below(&root.path().join("imports"), &mut import_files);
        assert_eq!(
            import_files,
            vec![
                root.path()
                    .join("imports")
                    .join(timestamp)
                    .join("import.json")
            ]
        );
        assert_eq!(fs::read(&owner).unwrap(), b"owner bytes");
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
        let (timestamp, method, model_called, reason) = super::timestamp_for_upload(
            &json!({"deterministic_only":"true"}),
            std::path::Path::new("notes.txt"),
            "notes_20260801_120000.txt",
        );
        assert_eq!(timestamp, "20260801_120000");
        assert_eq!(method, "deterministic");
        assert!(!model_called);
        assert_eq!(reason, None);
    }

    #[test]
    fn deterministic_filename_timestamp_wins_when_payload_is_only_on_disk() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("payload.bin");
        fs::write(&path, b"not an image and not exif").unwrap();
        let (timestamp, method, model_called, reason) =
            super::timestamp_for_upload(&json!({}), &path, "notes_20260801_120000.txt");
        assert_eq!(timestamp, "20260801_120000");
        assert_eq!(method, "deterministic");
        assert!(!model_called);
        assert_eq!(reason, None);
    }

    #[test]
    fn deterministic_only_upload_does_not_claim_a_model_attempt() {
        let (_, method, model_called, reason) = super::timestamp_for_upload(
            &json!({"deterministic_only":"true"}),
            std::path::Path::new("notes.txt"),
            "notes.txt",
        );
        assert_eq!(method, "upload_fallback");
        assert!(!model_called);
        assert_eq!(reason, Some("no_deterministic_match"));
    }

    #[test]
    fn deterministic_only_form_values_match_the_browser_contract() {
        for value in ["true", "TRUE", "1", "yes", "yEs"] {
            let (_, method, model_called, reason) = super::timestamp_for_upload(
                &json!({"deterministic_only":value}),
                std::path::Path::new("notes.txt"),
                "notes.txt",
            );
            assert_eq!(method, "upload_fallback", "{value}");
            assert!(!model_called, "{value}");
            assert_eq!(reason, Some("no_deterministic_match"), "{value}");
        }
    }

    #[test]
    fn deterministic_metadata_timestamp_accepts_the_exiftool_creation_shape() {
        assert_eq!(
            super::metadata_timestamp("2026:08:01 12:34:56"),
            Some("20260801_123456".to_owned())
        );
        let fields = json!({"CreateDate":"2026-08-01T12:34:56Z"})
            .as_object()
            .unwrap()
            .clone();
        let expected = chrono::DateTime::parse_from_rfc3339("2026-08-01T12:34:56Z")
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y%m%d_%H%M%S")
            .to_string();
        assert_eq!(
            super::metadata_timestamp_from_fields(&fields),
            Some(expected)
        );
    }

    #[test]
    fn metadata_plan_uses_exiftool_json_and_the_input_path() {
        let path = std::path::Path::new("photo.jpg");
        let mut recorded = None;
        let result = super::metadata_timestamp_with_runner(path, |plan| {
            recorded = Some((
                plan.program.clone(),
                plan.args.clone(),
                plan.path.clone(),
                plan.timeout,
            ));
            super::MetadataCommandOutcome::Unavailable
        });
        assert_eq!(result, None);
        let (program, args, planned_path, timeout) = recorded.expect("recorded plan");
        assert_eq!(program, std::path::Path::new("exiftool"));
        assert_eq!(args, [OsString::from("-json"), OsString::from("photo.jpg")]);
        assert_eq!(planned_path, path);
        assert_eq!(timeout, Duration::from_secs(2));
    }

    #[test]
    fn metadata_runner_success_parses_the_authored_exiftool_timestamp() {
        let path = std::path::Path::new("photo.jpg");
        let result = super::metadata_timestamp_with_runner(path, |_| {
            super::MetadataCommandOutcome::Completed(
                br#"[{"CreateDate":"2026:08:01 12:34:56"}]"#.to_vec(),
            )
        });
        assert_eq!(result.as_deref(), Some("20260801_123456"));
    }

    #[test]
    fn metadata_runner_malformed_output_is_none() {
        let path = std::path::Path::new("photo.jpg");
        let result = super::metadata_timestamp_with_runner(path, |_| {
            super::MetadataCommandOutcome::Completed(b"not-json".to_vec())
        });
        assert_eq!(result, None);
    }

    #[test]
    fn metadata_runner_unavailable_is_none() {
        let path = std::path::Path::new("photo.jpg");
        let result = super::metadata_timestamp_with_runner(path, |_| {
            super::MetadataCommandOutcome::Unavailable
        });
        assert_eq!(result, None);
    }

    #[test]
    fn metadata_runner_timeout_is_none() {
        let path = std::path::Path::new("photo.jpg");
        let result = super::metadata_timestamp_with_runner(path, |_| {
            super::MetadataCommandOutcome::TimedOut
        });
        assert_eq!(result, None);
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
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(fs::read(&owner).unwrap(), b"only owner copy");
        assert!(!root.path().join("imports/old-ts").exists());
        assert_eq!(
            read_import_metadata(root.path(), "new-ts").unwrap()["file_path"],
            owner.display().to_string()
        );
    }

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
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            *captured.borrow(),
            vec![
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
    async fn criterion_26_metadata_failure_after_send_keeps_task_id_top_level() {
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
        );
        let (status, body) = response_json(response).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body["task_id"].as_str().is_some());
        assert!(body.get("extra").is_none());
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
        );
        let (_, start) = response_json(start).await;
        assert_eq!(start["reason_code"], "invalid_operation_for_state");
    }

    #[tokio::test]
    async fn meta_refuses_running_and_successful_imports_without_retargeting_metadata() {
        let root = TempDir::new().unwrap();
        for (timestamp, imported) in [("running", None), ("success", Some(json!({})))] {
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
}
