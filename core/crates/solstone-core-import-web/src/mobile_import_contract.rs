// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! How a linked device brings an owner-chosen item into the journal: a multipart save stages
//! the item, then a JSON start with the save's `path` and `timestamp` runs it. The schema and
//! vectors under `contracts/mobile-import/` are generated from this module, and
//! `solstone-core-repository-contracts` refuses them when they drift from it.

use axum::http::StatusCode;
use serde_json::{Map, Value, json};

use crate::lifecycle;
use crate::multipart;
use crate::preview;

pub const SAVE_PATH: &str = "/app/import/api/save";
pub const START_PATH: &str = "/app/import/api/start";

const SOURCE: &str = "core/crates/solstone-core-import-web/src/mobile_import_contract.rs";
const REGENERATION_COMMAND: &str = "cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib mobile_import_contract::regenerate_mobile_import_contract -- --ignored";

/// The refusals a save or a start can return. Each code has exactly one status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportReasonCode {
    MissingRequiredField,
    IngestNoFiles,
    MultipartPartTooLarge,
    StravaDownload,
    UnsupportedImageFormat,
    InvalidOperationForState,
    ImportNotFound,
    ImportConflict,
    ImportPreviewRequired,
    ImportQueueUnreachable,
    ImportMetadataFailed,
}

impl ImportReasonCode {
    pub const fn all() -> &'static [Self] {
        &[
            Self::MissingRequiredField,
            Self::IngestNoFiles,
            Self::MultipartPartTooLarge,
            Self::StravaDownload,
            Self::UnsupportedImageFormat,
            Self::InvalidOperationForState,
            Self::ImportNotFound,
            Self::ImportConflict,
            Self::ImportPreviewRequired,
            Self::ImportQueueUnreachable,
            Self::ImportMetadataFailed,
        ]
    }

    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::MissingRequiredField => "missing_required_field",
            Self::IngestNoFiles => "ingest_no_files",
            Self::MultipartPartTooLarge => "multipart_part_too_large",
            Self::StravaDownload => "strava_download",
            Self::UnsupportedImageFormat => "unsupported_image_format",
            Self::InvalidOperationForState => "invalid_operation_for_state",
            Self::ImportNotFound => "import_not_found",
            Self::ImportConflict => "import_conflict",
            Self::ImportPreviewRequired => "import_preview_required",
            Self::ImportQueueUnreachable => "import_queue_unreachable",
            Self::ImportMetadataFailed => "import_metadata_failed",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            Self::MissingRequiredField | Self::IngestNoFiles | Self::InvalidOperationForState => {
                StatusCode::BAD_REQUEST
            }
            Self::ImportNotFound => StatusCode::NOT_FOUND,
            Self::ImportConflict | Self::ImportPreviewRequired => StatusCode::CONFLICT,
            Self::MultipartPartTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::StravaDownload | Self::UnsupportedImageFormat => StatusCode::UNPROCESSABLE_ENTITY,
            Self::ImportMetadataFailed => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ImportQueueUnreachable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

/// What a save records an item as, which decides how it imports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedSource {
    Image,
    Document,
    Audio,
    Text,
}

impl SavedSource {
    pub const fn all() -> &'static [Self] {
        &[Self::Image, Self::Document, Self::Audio, Self::Text]
    }

    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Document => "document",
            Self::Audio => "audio",
            Self::Text => "text",
        }
    }
}

/// What a save or start is refused for when the refusal has a typed kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportErrorKind {
    AlreadyImported,
    AlreadyRunning,
    BodyConfirmationRequired,
    ClientItemConflict,
    NothingToImport,
    SourceChanged,
    InvalidRequest,
    NotFound,
    PlanFailed,
}

impl ImportErrorKind {
    pub const fn all() -> &'static [Self] {
        &[
            Self::AlreadyImported,
            Self::AlreadyRunning,
            Self::BodyConfirmationRequired,
            Self::ClientItemConflict,
            Self::NothingToImport,
            Self::SourceChanged,
            Self::InvalidRequest,
            Self::NotFound,
            Self::PlanFailed,
        ]
    }

    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::AlreadyImported => "already_imported",
            Self::AlreadyRunning => "already_running",
            Self::BodyConfirmationRequired => "body_confirmation_required",
            Self::ClientItemConflict => "client_item_conflict",
            Self::NothingToImport => "nothing_to_import",
            Self::SourceChanged => "source_changed",
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::PlanFailed => "plan_failed",
        }
    }
}

/// The body of a start that sent its import to the task queue.
pub(crate) fn queued_start_response(task_id: &str, import_id: &str) -> Value {
    json!({"status": "ok", "task_id": task_id, "import_id": import_id})
}

/// The body of a start that runs its import in this process.
pub(crate) fn inprocess_start_response(
    task_id: &str,
    import_id: &str,
    generation: u64,
    attempt_id: &str,
) -> Value {
    json!({
        "status": "ok",
        "import_id": import_id,
        "task_id": task_id,
        "generation": generation,
        "attempt_id": attempt_id
    })
}

fn with_replay(mut val: Value) -> Value {
    val["replay"] = json!(true);
    val
}

fn wire<T: Copy>(values: &[T], as_wire: fn(T) -> &'static str) -> Vec<&'static str> {
    values.iter().map(|value| as_wire(*value)).collect()
}

pub fn schema_json() -> Value {
    let reason_codes = wire(ImportReasonCode::all(), ImportReasonCode::as_wire);
    let sources = wire(SavedSource::all(), SavedSource::as_wire);
    let error_kinds = wire(ImportErrorKind::all(), ImportErrorKind::as_wire);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Solstone mobile import v1",
        "description": "A linked device brings an owner-chosen item into the journal over its tunnel: a multipart save stages the item, then a JSON start with the save's path and timestamp runs it.",
        "x-solstone-rust-source": SOURCE,
        "x-solstone-regeneration-command": REGENERATION_COMMAND,
        "x-solstone-routes": {"save": format!("POST {SAVE_PATH}"), "start": format!("POST {START_PATH}")},
        "type": "object",
        "$defs": {
            "reason_code": {"type": "string", "enum": reason_codes},
            "reason_status": {
                "description": "The one HTTP status each reason code is sent with.",
                "type": "object",
                "properties": ImportReasonCode::all().iter().map(|code| (code.as_wire().to_owned(), json!({"const": code.status().as_u16()}))).collect::<Map<_, _>>()
            },
            "source": {"type": "string", "enum": sources},
            "error_kind": {"type": "string", "enum": error_kinds},
            "error_response": {
                "description": "Every refusal. error is a sentence the owner may be shown as is; reason_code and detail are for machines.",
                "type": "object",
                "required": ["error", "reason_code"],
                "properties": {
                    "error": {"type": "string"},
                    "reason_code": {"$ref": "#/$defs/reason_code"},
                    "detail": {"type": "string"},
                    "kind": {"$ref": "#/$defs/error_kind"}
                }
            },
            "save_request": {
                "description": "Multipart parts, each a string except file. One of file (with a filename) or text carries the item; a file wins when both are sent. Fields other than these are ignored.",
                "type": "object",
                "required": ["client_item_id"],
                "x-solstone-multipart-limits": {
                    "max_parts": multipart::MAX_PARTS,
                    "max_headers_per_part": multipart::MAX_HEADERS,
                    "max_filename_bytes": multipart::MAX_FILENAME_BYTES,
                    "max_field_bytes": multipart::MAX_PART_BYTES,
                    "max_file_bytes": multipart::MAX_SAVE_FILE_BYTES
                },
                "properties": {
                    "client_item_id": {"description": "The client's idempotency key, stable across retries of one item.", "type": "string", "minLength": 1},
                    "file": {"description": "The item's bytes; the part's filename is required and its content type is recorded.", "type": "string", "contentMediaType": "application/octet-stream"},
                    "text": {"description": "Pasted text, staged as paste.txt.", "type": "string"},
                    "imported_via": {"description": "The door the item came through; web_dashboard when absent.", "type": "string"},
                    "source_hint": {"description": "The import card selected by the owner. Omit for general file intake.", "type": "string"},
                    "area": {"description": "Import area selected on the journal page.", "type":"string","maxLength":96},
                    "context": {"description": "JSON object of choices made on the import page. Unknown keys are retained inside context.", "type":"string","contentMediaType":"application/json"},
                    "setting": {"type": "string"},
                    "observer_handle": {"type": "string"},
                    "caption": {"description": "Owner-written text shared alongside an image.", "type": "string"},
                    "captured_at": {"description": "Client-observed photo capture timestamp (RFC 3339 with offset when known).", "type": "string"},
                    "client": {"description": "Kept when it is a JSON object, otherwise stored as {}.", "type": "string"}
                }
            },
            "save_response": {
                "description": "200: the item is staged. replay is true when this client_item_id was already staged with the same content. Content staged earlier under another client_item_id returns that earlier item.",
                "type": "object",
                "required": ["schema_version", "status", "replay", "path", "timestamp", "source_timestamp", "client_item_id", "source", "recommended_action", "metadata", "diagnostics"],
                "properties": {
                    "schema_version": {"const": 1},
                    "status": {"const": "staged"},
                    "replay": {"type": "boolean"},
                    "path": {"description": "Sent back unchanged in start_request.", "type": "string"},
                    "timestamp": {"description": "The import directory id, sent back unchanged in start_request.", "type": "string"},
                    "source_timestamp": {"type": "string"},
                    "resolved_timestamp": {"type": ["string", "null"]},
                    "client_item_id": {"type": "string"},
                    "source": {"$ref": "#/$defs/source"},
                    "setting": {"type": ["string", "null"]},
                    "recommended_action": {"const": "start"},
                    "metadata": {"type": "object"},
                    "diagnostics": {"type": "object"}
                }
            },
            "start_request": {
                "description": "Without preview_token the journal previews the import itself, under the same rules as a preview the owner saw.",
                "type": "object",
                "required": ["path", "timestamp"],
                "properties": {
                    "path": {"type": "string"},
                    "timestamp": {"type": "string"},
                    "import_id": {"type": "string"},
                    "preview_token": {"type": "string"}
                }
            },
            "start_response": {
                "description": "200: the import is queued, or running in process when generation and attempt_id are present. replay is true when this start found the import already queued, running or finished. A replay is classified by the same fields as a fresh body: status, task_id, and generation and attempt_id when present.",
                "type": "object",
                "required": ["status", "task_id", "import_id"],
                "properties": {
                    "status": {"const": "ok"},
                    "task_id": {"type": "string"},
                    "import_id": {"type": "string"},
                    "generation": {"type": "integer", "minimum": 0},
                    "attempt_id": {"type": "string"},
                    "replay": {"type": "boolean"}
                }
            }
        }
    })
}

fn fixture_metadata() -> Map<String, Value> {
    let mut metadata = Map::new();
    for (key, value) in [
        ("import_id", json!("20261019_101500")),
        ("user_timestamp", json!("20261019_101500")),
        ("source_timestamp", json!("20261019_101500")),
        (
            "file_path",
            json!("/journal/imports/20261019_101500/scan.pdf"),
        ),
        (
            "client_item_id",
            json!("123e4567-e89b-42d3-a456-426614174000"),
        ),
        ("source", json!(SavedSource::Document.as_wire())),
        ("setting", Value::Null),
        ("original_filename", json!("scan.pdf")),
        ("mime_type", json!("application/pdf")),
        ("imported_via", json!("mobile_share")),
        ("observer_handle", Value::Null),
        ("source_hint", Value::Null),
        ("client", json!({})),
        ("timestamp_detection_method", json!("upload_fallback")),
        ("timestamp_detection_model_called", json!(false)),
        ("timestamp_detection_no_match_reason", Value::Null),
        ("source_inference", json!("default")),
    ] {
        metadata.insert(key.to_owned(), value);
    }
    metadata
}

pub fn vectors_json() -> Value {
    let metadata = fixture_metadata();
    let saved = lifecycle::summary(&metadata);
    let start_request = json!({"path": saved["path"], "timestamp": saved["timestamp"]});
    json!({
        "source": SOURCE,
        "regeneration_command": REGENERATION_COMMAND,
        "vectors": {
            "save_request": {
                "client_item_id": metadata["client_item_id"],
                "imported_via": metadata["imported_via"],
                "file": {"filename": metadata["original_filename"], "content_type": metadata["mime_type"]}
            },
            "save_staged_200": {"status": 200, "body": saved},
            "save_replay_200": {"status": 200, "body": lifecycle::replay_summary(&metadata)},
            "start_request": start_request,
            "start_queued_200": {"status": 200, "body": queued_start_response("1792404900000", "20261019_101500")},
            "start_inprocess_200": {"status": 200, "body": inprocess_start_response("1792404900000", "20261019_101500", 1, "attempt-1")},
            "start_queued_replay_200": {"status": 200, "body": with_replay(queued_start_response("1792404900000", "20261019_101500"))},
            "start_inprocess_replay_200": {"status": 200, "body": with_replay(inprocess_start_response("1792404900000", "20261019_101500", 1, "attempt-1"))},
            "save_already_imported_400": {
                "status": 400,
                "body": lifecycle::invalid_state_body("content already imported", Some(ImportErrorKind::AlreadyImported)),
            },
            "save_client_item_conflict_400": {
                "status": 400,
                "body": lifecycle::invalid_state_body(
                    "client_item_id already staged for different content; use a new client_item_id or re-fetch the existing item",
                    Some(ImportErrorKind::ClientItemConflict),
                ),
            },
            "start_already_imported_400": {
                "status": 400,
                "body": lifecycle::invalid_state_body("content already imported; will not start", Some(ImportErrorKind::AlreadyImported)),
            },
            "start_nothing_to_import_409": {
                "status": 409,
                "body": preview::preview_error_body(
                    ImportReasonCode::ImportPreviewRequired.as_wire(),
                    &preview::PreviewError::NothingToImport,
                ),
            },
            "refusals": ImportReasonCode::all().iter().map(|code| json!({"status": code.status().as_u16(), "reason_code": code.as_wire()})).collect::<Vec<_>>()
        }
    })
}
