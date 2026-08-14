// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
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
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};

use crate::{
    AppState,
    callosum::{BusError, request_required},
    http::{error, import_not_found, json as json_response},
    multipart,
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

fn summary(metadata: &ImportMetadata) -> Value {
    json!({
        "schema_version": 1,
        "status": "staged",
        "replay": false,
        "path": metadata.get("file_path").cloned().unwrap_or(Value::String(String::new())),
        "timestamp": metadata.get("user_timestamp").cloned().unwrap_or(Value::String(String::new())),
        "client_item_id": metadata.get("client_item_id").cloned().unwrap_or(Value::String(String::new())),
        "source": metadata.get("source").cloned().unwrap_or_else(|| json!("text")),
        "facet": metadata.get("facet").cloned().unwrap_or(Value::Null),
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

fn stage_bytes(
    root: &Path,
    timestamp: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<PathBuf, ImportError> {
    let directory = root.join("imports").join(timestamp);
    create_directory_with_mode(&directory, 0o700).map_err(|error| ImportError::PathResolution {
        path: directory.clone(),
        message: error.to_string(),
    })?;
    let target = directory.join(filename);
    atomic_replace(&target, bytes, AtomicWriteOptions { mode: Some(0o600) }).map_err(|error| {
        ImportError::PromotionFailed {
            path: target.clone(),
            message: error.to_string(),
        }
    })?;
    Ok(target)
}

fn temporary_hash(bytes: &[u8]) -> Result<(SourceHash, bool), String> {
    let mut temporary = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    temporary
        .write_all(bytes)
        .map_err(|error| error.to_string())?;
    let path = temporary.path().to_path_buf();
    let source_hash = hash_source(&path).map_err(|error| error.to_string())?;
    drop(temporary);
    Ok((source_hash, !path.exists()))
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
            "facet".to_owned(),
            clean_optional(data.get("facet")).map_or(Value::Null, Value::String),
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

pub(crate) async fn save(State(state): State<AppState>, multipart: Multipart) -> Response {
    let parts = match multipart::collect(multipart).await {
        Ok(parts) => parts,
        Err(detail) => {
            return failure(
                StatusCode::BAD_REQUEST,
                "ingest_no_files",
                "I couldn't find any files to bring in.",
                detail,
            );
        }
    };
    let mut fields = BTreeMap::new();
    let mut upload = None;
    for part in parts {
        if part.name == "file" {
            upload = Some(part);
        } else if let Ok(value) = String::from_utf8(part.bytes) {
            fields.insert(part.name, value);
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
    let (filename, original_filename, mime_type, bytes) = match upload {
        Some(part) if part.filename.is_some() => {
            let original = part.filename.unwrap();
            let Some(filename) = safe_filename(&original) else {
                return failure(
                    StatusCode::BAD_REQUEST,
                    "ingest_no_files",
                    "I couldn't find any files to bring in.",
                    "No input",
                );
            };
            (filename, original, part.content_type, part.bytes)
        }
        _ if !text.is_empty() => (
            "paste.txt".to_owned(),
            "paste.txt".to_owned(),
            Some("text/plain".to_owned()),
            text.into_bytes(),
        ),
        _ => {
            return failure(
                StatusCode::BAD_REQUEST,
                "ingest_no_files",
                "I couldn't find any files to bring in.",
                "No input",
            );
        }
    };
    let (source_hash, temporary_removed) = match temporary_hash(&bytes) {
        Ok(result) => result,
        Err(error) => return metadata_failed(format!("Failed to stage temporary file: {error}")),
    };
    debug_assert!(
        temporary_removed,
        "request-scoped upload temporary survives"
    );
    if manifest_exists(&state.root, &source_hash) {
        return invalid_state("content already imported");
    }
    let timestamp = import_timestamp();
    let file_path = match stage_bytes(&state.root, &timestamp, &filename, &bytes) {
        Ok(path) => path,
        Err(error) => return metadata_failed(error.to_string()),
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
        method: "upload_fallback",
    });
    metadata.insert("file_size".to_owned(), json!(bytes.len()));
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
    let mut metadata = match read_import_metadata(&state.root, timestamp) {
        Ok(metadata) => metadata,
        Err(_) => return import_not_found("Import metadata not found"),
    };
    if metadata
        .get("source_hash")
        .and_then(Value::as_str)
        .is_some_and(|hash| manifest_exists(&state.root, &SourceHash::new(hash.to_owned())))
    {
        return invalid_state("content already imported");
    }
    let mut changed = Map::new();
    for key in [
        "facet",
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
        let value = if matches!(key, "facet" | "setting" | "source_hint" | "observer_handle") {
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
    if let Some(facet) = metadata
        .get("facet")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        cmd.extend(["--facet".to_owned(), facet.to_owned()]);
    }
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
    use std::{cell::RefCell, fs, os::unix::fs::PermissionsExt, rc::Rc};

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

    use super::{BusError, command, start_with, temporary_hash};

    fn staged(root: &std::path::Path, timestamp: &str, metadata: Value) {
        write_import_metadata(root, timestamp, &metadata.as_object().unwrap().clone()).unwrap();
    }

    fn metadata(file_path: String, source_hash: &str) -> ImportMetadata {
        Map::from_iter([
            ("file_path".to_owned(), json!(file_path)),
            ("source_hash".to_owned(), json!(source_hash)),
            ("source".to_owned(), json!("wrong-source")),
            ("source_hint".to_owned(), json!("right-source")),
            ("facet".to_owned(), json!("work")),
            ("setting".to_owned(), json!("notes")),
        ])
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
        let (_, body) = response_json(response).await;
        let path = std::path::PathBuf::from(body["path"].as_str().unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"hello upload");
        assert!(temporary_hash(b"temporary bytes").unwrap().1);
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
                "--facet",
                "work",
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
}
