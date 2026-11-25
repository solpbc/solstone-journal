// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! How a linked device or browser brings an owner-chosen item into the journal:
//! - below `RESUMABLE_THRESHOLD_BYTES` (`16 MiB`), a single multipart `POST /app/import/api/save`
//!   stages the item;
//! - at or above `RESUMABLE_THRESHOLD_BYTES` (when the journal advertises
//!   `"resumable_import_uploads"` in `GET /api/system/status`), a chunked upload under
//!   `/app/import/api/uploads` stages the item with `4 MiB` resumable chunks and whole-file
//!   SHA-256 midstate verification;
//! - in both paths, `POST /app/import/api/start` with the staged response's `path` and
//!   `timestamp` runs it.
//!
//! The schema and vectors under `contracts/mobile-import/` are generated from this module, and
//! `solstone-core-repository-contracts` refuses them when they drift from it.

use axum::http::StatusCode;
use serde_json::{Map, Value, json};

use crate::lifecycle;
use crate::multipart;
use crate::preview;
use crate::uploads;

pub const SAVE_PATH: &str = "/app/import/api/save";
pub const START_PATH: &str = "/app/import/api/start";
pub const CAPABILITY_RESUMABLE_IMPORT_UPLOADS: &str = "resumable_import_uploads";

/// Server-allowlisted device nouns accepted on resumable uploads and `import.status` events.
pub const DEVICE_NOUN_ALLOWLIST: &[&str] = &[
    "iphone", "ipad", "phone", "mac", "PC", "computer", "browser",
];

/// Recommended transient retry backoff schedule (in seconds, with +-20% jitter) for chunk uploads.
pub const RETRY_BACKOFF_SECONDS: &[u64] = &[1, 2, 5, 10, 30, 60];

/// Valid `phase` values on `GET /app/import/api/uploads*` and `import.status` progress events.
pub const UPLOAD_PHASES: &[&str] = &["sending", "paused", "checking"];

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

/// Protocol-specific refusal codes returned by `/app/import/api/uploads*` routes (in addition
/// to shared `ImportReasonCode` values on `POST /uploads` and `POST /uploads/{id}/commit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadReasonCode {
    UploadNotFound,
    OffsetMismatch,
    IncompleteUpload,
    JournalBusy,
    ContentLengthRequired,
    IncompleteChunkBody,
    InvalidChunkLength,
    InvalidOffset,
    InvalidChunkChecksum,
    ChunkExceedsTotalBytes,
    InvalidChunkFraming,
    ChunkTooLarge,
    ChunkChecksumMismatch,
    WholeFileChecksumMismatch,
    InsufficientJournalSpace,
    UnsupportedMediaType,
}

impl UploadReasonCode {
    pub const fn all() -> &'static [Self] {
        &[
            Self::UploadNotFound,
            Self::OffsetMismatch,
            Self::IncompleteUpload,
            Self::JournalBusy,
            Self::ContentLengthRequired,
            Self::IncompleteChunkBody,
            Self::InvalidChunkLength,
            Self::InvalidOffset,
            Self::InvalidChunkChecksum,
            Self::ChunkExceedsTotalBytes,
            Self::InvalidChunkFraming,
            Self::ChunkTooLarge,
            Self::ChunkChecksumMismatch,
            Self::WholeFileChecksumMismatch,
            Self::InsufficientJournalSpace,
            Self::UnsupportedMediaType,
        ]
    }

    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::UploadNotFound => "upload_not_found",
            Self::OffsetMismatch => "offset_mismatch",
            Self::IncompleteUpload => "incomplete_upload",
            Self::JournalBusy => "journal_busy",
            Self::ContentLengthRequired => "content_length_required",
            Self::IncompleteChunkBody => "incomplete_chunk_body",
            Self::InvalidChunkLength => "invalid_chunk_length",
            Self::InvalidOffset => "invalid_offset",
            Self::InvalidChunkChecksum => "invalid_chunk_checksum",
            Self::ChunkExceedsTotalBytes => "chunk_exceeds_total_bytes",
            Self::InvalidChunkFraming => "invalid_chunk_framing",
            Self::ChunkTooLarge => "chunk_too_large",
            Self::ChunkChecksumMismatch => "chunk_checksum_mismatch",
            Self::WholeFileChecksumMismatch => "whole_file_checksum_mismatch",
            Self::InsufficientJournalSpace => "insufficient_journal_space",
            Self::UnsupportedMediaType => "unsupported_media_type",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            Self::UploadNotFound => StatusCode::NOT_FOUND,
            Self::OffsetMismatch | Self::IncompleteUpload | Self::JournalBusy => {
                StatusCode::CONFLICT
            }
            Self::ContentLengthRequired
            | Self::IncompleteChunkBody
            | Self::InvalidChunkLength
            | Self::InvalidOffset
            | Self::InvalidChunkChecksum
            | Self::ChunkExceedsTotalBytes => StatusCode::BAD_REQUEST,
            Self::InvalidChunkFraming => StatusCode::EXPECTATION_FAILED,
            Self::ChunkTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::ChunkChecksumMismatch | Self::WholeFileChecksumMismatch => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            Self::InsufficientJournalSpace => StatusCode::INSUFFICIENT_STORAGE,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
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

fn extend_defs(target: &mut Map<String, Value>, source: Value) {
    if let Value::Object(entries) = source {
        target.extend(entries);
    }
}

fn base_schema_defs() -> Value {
    let reason_codes = wire(ImportReasonCode::all(), ImportReasonCode::as_wire);
    let upload_reason_codes = wire(UploadReasonCode::all(), UploadReasonCode::as_wire);
    let sources = wire(SavedSource::all(), SavedSource::as_wire);
    let error_kinds = wire(ImportErrorKind::all(), ImportErrorKind::as_wire);
    json!({
        "reason_code": {"type": "string", "enum": reason_codes},
        "reason_status": {
            "description": "The one HTTP status each reason code is sent with.",
            "type": "object",
            "properties": ImportReasonCode::all().iter().map(|code| (code.as_wire().to_owned(), json!({"const": code.status().as_u16()}))).collect::<Map<_, _>>()
        },
        "upload_reason_code": {"type": "string", "enum": upload_reason_codes},
        "upload_reason_status": {
            "description": "The HTTP status each upload protocol reason code is sent with.",
            "type": "object",
            "properties": UploadReasonCode::all().iter().map(|code| (code.as_wire().to_owned(), json!({"const": code.status().as_u16()}))).collect::<Map<_, _>>()
        },
        "device_noun": {"type": "string", "enum": DEVICE_NOUN_ALLOWLIST},
        "upload_phase": {"type": "string", "enum": UPLOAD_PHASES},
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
    })
}

fn resumable_route_defs() -> Value {
    json!({
        "upload_error_response": {
            "description": "Refusal returned by /app/import/api/uploads* routes. Always sent only after draining bounded chunk bodies (<= 8 MiB).",
            "type": "object",
            "required": ["error", "reason_code", "detail"],
            "properties": {
                "error": {"type": "string"},
                "reason_code": {
                    "anyOf": [
                        {"$ref": "#/$defs/upload_reason_code"},
                        {"$ref": "#/$defs/reason_code"}
                    ]
                },
                "detail": {"type": "string"},
                "kind": {"$ref": "#/$defs/error_kind"},
                "next_offset": {"type": "integer", "minimum": 0},
                "total_bytes": {"type": "integer", "minimum": 1},
                "max_chunk_bytes": {"type": "integer", "minimum": 1},
                "required_bytes": {"type": "integer", "minimum": 0},
                "available_bytes": {"type": "integer", "minimum": 0},
                "expected_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
                "actual_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"}
            }
        },
        "sources_resumable_advertisement": {
            "description": "Top-level resumable limits object on GET /app/import/api/sources.",
            "type": "object",
            "required": ["threshold_bytes", "chunk_bytes", "max_file_bytes"],
            "properties": {
                "threshold_bytes": {"const": uploads::RESUMABLE_THRESHOLD_BYTES},
                "chunk_bytes": {"const": uploads::RESUMABLE_CHUNK_BYTES},
                "max_file_bytes": {"const": uploads::MAX_UPLOAD_FILE_BYTES}
            }
        },
        "upload_create_request": {
            "description": "JSON request (<= 64 KiB) to POST /app/import/api/uploads. Native apps supply sha256 upfront so pre-upload deduplication can skip sending bytes; browser clients may omit sha256 on create and supply it on POST /uploads/{upload_id}/commit.",
            "type": "object",
            "required": ["client_item_id", "filename", "total_bytes"],
            "properties": {
                "client_item_id": {"type": "string", "minLength": 1, "maxLength": 256},
                "filename": {"type": "string", "minLength": 1, "maxLength": multipart::MAX_FILENAME_BYTES},
                "mime_type": {"type": "string"},
                "total_bytes": {"type": "integer", "minimum": 1, "maximum": uploads::MAX_UPLOAD_FILE_BYTES},
                "sha256": {"type": "string", "pattern": "^[0-9a-fA-F]{64}$"},
                "source_hint": {"type": "string"},
                "imported_via": {"type": "string"},
                "device_noun": {"$ref": "#/$defs/device_noun"},
                "area": {"type": "string", "maxLength": 96},
                "setting": {"type": "string"},
                "caption": {"type": "string"},
                "captured_at": {"type": "string"},
                "observer_handle": {"type": "string"},
                "context": {
                    "description": "JSON object or JSON-encoded object string (<= 16 KiB).",
                    "anyOf": [{"type": "object"}, {"type": "string"}]
                },
                "client": {"type": "object", "description": "Optional client metadata object (<= 16 KiB serialized)."}
            }
        },
        "upload_open_response": {
            "description": "201 Created (new session) or 200 OK (resuming an open session for the same client_item_id).",
            "type": "object",
            "required": ["upload_id", "client_item_id", "status", "next_offset", "total_bytes", "chunk_bytes", "expires_at"],
            "properties": {
                "upload_id": {"type": "string", "format": "uuid"},
                "client_item_id": {"type": "string"},
                "status": {"const": "open"},
                "next_offset": {"type": "integer", "minimum": 0},
                "total_bytes": {"type": "integer", "minimum": 1, "maximum": uploads::MAX_UPLOAD_FILE_BYTES},
                "chunk_bytes": {"const": uploads::RESUMABLE_CHUNK_BYTES},
                "expires_at": {"type": "string", "format": "date-time"}
            }
        },
        "upload_already_staged_response": {
            "description": "200 OK from POST /app/import/api/uploads when the file is already staged and ready for POST /app/import/api/start.",
            "type": "object",
            "required": ["status", "replay", "save_response"],
            "properties": {
                "status": {"const": "already_staged"},
                "replay": {"type": "boolean"},
                "save_response": {"$ref": "#/$defs/save_response"}
            }
        },
        "upload_already_imported_response": {
            "description": "200 OK from POST /app/import/api/uploads when sha256 matches an already-imported item (excluding repeatable sources Strava and Apple Health).",
            "type": "object",
            "required": ["status", "import_id", "reason_code", "kind"],
            "properties": {
                "status": {"const": "already_imported"},
                "import_id": {"type": ["string", "null"]},
                "reason_code": {"const": ImportReasonCode::InvalidOperationForState.as_wire()},
                "kind": {"const": ImportErrorKind::AlreadyImported.as_wire()}
            }
        }
    })
}

fn resumable_transfer_and_event_defs() -> Value {
    json!({
        "upload_chunk_headers": {
            "description": "Required HTTP headers on PUT /app/import/api/uploads/{upload_id}?offset={offset}. Never send Transfer-Encoding: chunked or Expect: 100-continue.",
            "type": "object",
            "required": ["Content-Type", "Content-Length"],
            "properties": {
                "Content-Type": {"const": "application/octet-stream"},
                "Content-Length": {"type": "integer", "minimum": 1, "maximum": uploads::RESUMABLE_CHUNK_BYTES},
                "X-Chunk-Sha256": {"type": "string", "pattern": "^[0-9a-fA-F]{64}$"}
            }
        },
        "upload_chunk_response": {
            "description": "200 OK from PUT /app/import/api/uploads/{upload_id}?offset={offset}.",
            "type": "object",
            "required": ["upload_id", "next_offset", "total_bytes", "complete", "expires_at"],
            "properties": {
                "upload_id": {"type": "string", "format": "uuid"},
                "next_offset": {"type": "integer", "minimum": 1},
                "total_bytes": {"type": "integer", "minimum": 1},
                "complete": {"type": "boolean"},
                "expires_at": {"type": "string", "format": "date-time"}
            }
        },
        "upload_status_response": {
            "description": "200 OK from GET /app/import/api/uploads/{upload_id} and element schema in GET /app/import/api/uploads items and GET /app/import/api/list active_uploads.",
            "type": "object",
            "required": [
                "upload_id",
                "client_item_id",
                "filename",
                "status",
                "phase",
                "next_offset",
                "total_bytes",
                "chunk_bytes",
                "imported_via",
                "device_noun",
                "source_hint",
                "created_at",
                "last_activity_at",
                "expires_at"
            ],
            "properties": {
                "upload_id": {"type": "string", "format": "uuid"},
                "client_item_id": {"type": "string"},
                "filename": {"type": "string"},
                "status": {"const": "open"},
                "phase": {"$ref": "#/$defs/upload_phase"},
                "next_offset": {"type": "integer", "minimum": 0},
                "total_bytes": {"type": "integer", "minimum": 1},
                "chunk_bytes": {"const": uploads::RESUMABLE_CHUNK_BYTES},
                "imported_via": {"type": "string"},
                "device_noun": {"anyOf": [{"$ref": "#/$defs/device_noun"}, {"type": "null"}]},
                "source_hint": {"type": ["string", "null"]},
                "created_at": {"type": "string", "format": "date-time"},
                "last_activity_at": {"type": "string", "format": "date-time"},
                "expires_at": {"type": "string", "format": "date-time"}
            }
        },
        "upload_list_response": {
            "description": "200 OK from GET /app/import/api/uploads.",
            "type": "object",
            "required": ["items", "total"],
            "properties": {
                "items": {"type": "array", "items": {"$ref": "#/$defs/upload_status_response"}},
                "total": {"type": "integer", "minimum": 0}
            }
        },
        "upload_commit_request": {
            "description": "Optional JSON body (<= 64 KiB) for POST /app/import/api/uploads/{upload_id}/commit. Required when sha256 was omitted on POST /uploads; may be empty or {} when sha256 was supplied on POST /uploads. On 5xx, the journal retains the verified partial upload so retrying commit succeeds without re-sending chunks.",
            "type": "object",
            "properties": {
                "sha256": {"type": "string", "pattern": "^[0-9a-fA-F]{64}$"}
            }
        },
        "upload_commit_response": {
            "$ref": "#/$defs/save_response"
        },
        "upload_abort_response": {
            "description": "200 OK from DELETE /app/import/api/uploads/{upload_id}.",
            "type": "object",
            "required": ["deleted", "upload_id"],
            "properties": {
                "deleted": {"const": true},
                "upload_id": {"type": "string", "format": "uuid"}
            }
        },
        "host_import_status_progress": {
            "description": "Additive fields on window.solstoneJournalHost import.status (CustomEvent solstone-journal-host-status) during resumable or chunked uploads.",
            "type": "object",
            "required": ["item", "state"],
            "properties": {
                "id": {"type": "string"},
                "item": {"type": "string", "description": "Matches client_item_id so consumers can correlate host events with SSE upload_progress."},
                "name": {"type": "string"},
                "state": {"type": "string", "enum": ["waiting", "uploading", "attention", "delivered", "gone"]},
                "device_noun": {"$ref": "#/$defs/device_noun"},
                "size_bytes": {"type": "integer", "minimum": 0},
                "bytes_sent": {"type": "integer", "minimum": 0},
                "bytes_total": {"type": "integer", "minimum": 1},
                "resumable": {"type": "boolean"},
                "phase": {"$ref": "#/$defs/upload_phase"},
                "import_id": {"type": "string"},
                "message": {"type": "string"}
            }
        },
        "sse_importer_upload_events": {
            "description": "Additive Callosum / SSE envelopes on tract 'importer' delivered over /sse/events.",
            "oneOf": [
                {
                    "type": "object",
                    "required": ["tract", "event", "upload_id", "client_item_id", "filename", "bytes_received", "total_bytes", "phase"],
                    "properties": {
                        "tract": {"const": "importer"},
                        "event": {"const": "upload_progress"},
                        "upload_id": {"type": "string"},
                        "client_item_id": {"type": "string"},
                        "filename": {"type": "string"},
                        "bytes_received": {"type": "integer", "minimum": 0},
                        "total_bytes": {"type": "integer", "minimum": 1},
                        "phase": {"$ref": "#/$defs/upload_phase"},
                        "imported_via": {"type": "string"},
                        "device_noun": {"anyOf": [{"$ref": "#/$defs/device_noun"}, {"type": "null"}]},
                        "source_hint": {"type": ["string", "null"]}
                    }
                },
                {
                    "type": "object",
                    "required": ["tract", "event", "upload_id", "client_item_id", "import_id", "timestamp", "total_bytes"],
                    "properties": {
                        "tract": {"const": "importer"},
                        "event": {"const": "upload_committed"},
                        "upload_id": {"type": "string"},
                        "client_item_id": {"type": "string"},
                        "import_id": {"type": ["string", "null"]},
                        "timestamp": {"type": ["string", "null"]},
                        "total_bytes": {"type": "integer", "minimum": 1}
                    }
                },
                {
                    "type": "object",
                    "required": ["tract", "event", "upload_id", "client_item_id", "reason"],
                    "properties": {
                        "tract": {"const": "importer"},
                        "event": {"const": "upload_aborted"},
                        "upload_id": {"type": "string"},
                        "client_item_id": {"type": ["string", "null"]},
                        "reason": {"type": "string", "enum": ["cancelled", "checksum_mismatch", "refused"]}
                    }
                }
            ]
        }
    })
}

pub fn schema_json() -> Value {
    let mut defs = Map::new();
    extend_defs(&mut defs, base_schema_defs());
    extend_defs(&mut defs, resumable_route_defs());
    extend_defs(&mut defs, resumable_transfer_and_event_defs());

    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Solstone mobile import v1",
        "description": "A linked device brings an owner-chosen item into the journal over its tunnel: items below 16 MiB use a single multipart POST /app/import/api/save, items at or above 16 MiB (when resumable_import_uploads is advertised) use resumable 4 MiB chunks under /app/import/api/uploads, and POST /app/import/api/start runs the staged item.",
        "x-solstone-rust-source": SOURCE,
        "x-solstone-regeneration-command": REGENERATION_COMMAND,
        "x-solstone-routes": {
            "save": format!("POST {SAVE_PATH}"),
            "start": format!("POST {START_PATH}"),
            "system_status": "GET /api/system/status",
            "sources": "GET /app/import/api/sources",
            "uploads_create": format!("POST {}", uploads::UPLOAD_COLLECTION_PATH),
            "uploads_list": format!("GET {}", uploads::UPLOAD_COLLECTION_PATH),
            "uploads_status": format!("GET {}", uploads::UPLOAD_ITEM_PATH),
            "uploads_chunk": format!("PUT {}?offset={{offset}}", uploads::UPLOAD_ITEM_PATH),
            "uploads_commit": format!("POST {}", uploads::UPLOAD_COMMIT_PATH),
            "uploads_abort": format!("DELETE {}", uploads::UPLOAD_ITEM_PATH)
        },
        "x-solstone-resumable-policy": {
            "capability": CAPABILITY_RESUMABLE_IMPORT_UPLOADS,
            "threshold_bytes": uploads::RESUMABLE_THRESHOLD_BYTES,
            "chunk_bytes": uploads::RESUMABLE_CHUNK_BYTES,
            "max_file_bytes": uploads::MAX_UPLOAD_FILE_BYTES,
            "max_control_json_bytes": uploads::MAX_JSON_CONTROL_BYTES,
            "bridge_drain_ceiling_bytes": uploads::BRIDGE_DRAIN_CEILING_BYTES,
            "min_free_disk_bytes": uploads::MIN_FREE_DISK_BYTES,
            "max_active_uploads_bytes": uploads::MAX_ACTIVE_UPLOADS_BYTES,
            "partial_upload_ttl_ms": uploads::PARTIAL_UPLOAD_TTL_MS,
            "active_paused_threshold_ms": uploads::ACTIVE_PAUSED_THRESHOLD_MS,
            "retry_backoff_seconds": RETRY_BACKOFF_SECONDS,
            "fallback_on_404_or_405_when_capability_absent": true,
            "pairing_switch_rule": "Bind upload_id and remote_offset to target journal instance_id; if target journal changes mid-upload, discard remote upload_id and start fresh from offset 0 on the new journal while retaining the staged local file.",
            "chunk_wire_framing": {
                "content_length_required": true,
                "transfer_encoding_chunked_forbidden": true,
                "expect_100_continue_forbidden": true,
                "chunk_sha256_header": "X-Chunk-Sha256",
                "server_drains_body_before_error_up_to_bytes": uploads::BRIDGE_DRAIN_CEILING_BYTES
            }
        },
        "type": "object",
        "$defs": Value::Object(defs)
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

fn base_vectors(metadata: &Map<String, Value>, saved: &Value, replay: &Value) -> Value {
    let start_request = json!({"path": saved["path"], "timestamp": saved["timestamp"]});
    json!({
        "save_request": {
            "client_item_id": metadata["client_item_id"],
            "imported_via": metadata["imported_via"],
            "file": {"filename": metadata["original_filename"], "content_type": metadata["mime_type"]}
        },
        "save_staged_200": {"status": 200, "body": saved},
        "save_replay_200": {"status": 200, "body": replay},
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
        }
    })
}

fn resumable_vectors(metadata: &Map<String, Value>, saved: &Value, replay: &Value) -> Value {
    let upload_id = "550e8400-e29b-41d4-a716-446655440000";
    let sample_sha = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    let total_bytes: u64 = 33_554_432;
    json!({
        "upload_create_request": {
            "client_item_id": metadata["client_item_id"],
            "filename": metadata["original_filename"],
            "mime_type": metadata["mime_type"],
            "total_bytes": total_bytes,
            "sha256": sample_sha,
            "imported_via": metadata["imported_via"],
            "device_noun": "iphone"
        },
        "upload_created_201": {
            "status": 201,
            "body": {
                "upload_id": upload_id,
                "client_item_id": metadata["client_item_id"],
                "status": "open",
                "next_offset": 0,
                "total_bytes": total_bytes,
                "chunk_bytes": uploads::RESUMABLE_CHUNK_BYTES,
                "expires_at": "2026-12-01T22:00:00Z"
            }
        },
        "upload_resumed_200": {
            "status": 200,
            "body": {
                "upload_id": upload_id,
                "client_item_id": metadata["client_item_id"],
                "status": "open",
                "next_offset": uploads::RESUMABLE_CHUNK_BYTES,
                "total_bytes": total_bytes,
                "chunk_bytes": uploads::RESUMABLE_CHUNK_BYTES,
                "expires_at": "2026-12-01T22:05:00Z"
            }
        },
        "upload_already_staged_200": {
            "status": 200,
            "body": {
                "status": "already_staged",
                "replay": true,
                "save_response": replay
            }
        },
        "upload_already_imported_200": {
            "status": 200,
            "body": {
                "status": "already_imported",
                "import_id": "20261019_101500",
                "reason_code": ImportReasonCode::InvalidOperationForState.as_wire(),
                "kind": ImportErrorKind::AlreadyImported.as_wire()
            }
        },
        "upload_chunk_200": {
            "status": 200,
            "body": {
                "upload_id": upload_id,
                "next_offset": uploads::RESUMABLE_CHUNK_BYTES,
                "total_bytes": total_bytes,
                "complete": false,
                "expires_at": "2026-12-01T22:00:02Z"
            }
        },
        "upload_offset_mismatch_409": {
            "status": UploadReasonCode::OffsetMismatch.status().as_u16(),
            "body": {
                "error": "upload offset does not match journal position.",
                "reason_code": UploadReasonCode::OffsetMismatch.as_wire(),
                "detail": "client sent offset 0, journal expects 4194304",
                "next_offset": uploads::RESUMABLE_CHUNK_BYTES,
                "total_bytes": total_bytes
            }
        },
        "upload_chunk_checksum_mismatch_422": {
            "status": UploadReasonCode::ChunkChecksumMismatch.status().as_u16(),
            "body": {
                "error": "chunk integrity check failed.",
                "reason_code": UploadReasonCode::ChunkChecksumMismatch.as_wire(),
                "detail": "expected 0000000000000000000000000000000000000000000000000000000000000000, got 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
                "next_offset": 0,
                "total_bytes": total_bytes
            }
        },
        "upload_insufficient_space_507": {
            "status": UploadReasonCode::InsufficientJournalSpace.status().as_u16(),
            "body": {
                "error": "your journal doesn't have enough free space for that file.",
                "reason_code": UploadReasonCode::InsufficientJournalSpace.as_wire(),
                "detail": "journal volume does not have enough free space for this upload plus 256 MiB headroom",
                "required_bytes": total_bytes + uploads::MIN_FREE_DISK_BYTES,
                "available_bytes": 104_857_600
            }
        },
        "upload_status_200": {
            "status": 200,
            "body": {
                "upload_id": upload_id,
                "client_item_id": metadata["client_item_id"],
                "filename": metadata["original_filename"],
                "status": "open",
                "phase": "sending",
                "next_offset": uploads::RESUMABLE_CHUNK_BYTES,
                "total_bytes": total_bytes,
                "chunk_bytes": uploads::RESUMABLE_CHUNK_BYTES,
                "imported_via": metadata["imported_via"],
                "device_noun": "iphone",
                "source_hint": null,
                "created_at": "2026-11-24T22:00:00Z",
                "last_activity_at": "2026-11-24T22:00:02Z",
                "expires_at": "2026-12-01T22:00:02Z"
            }
        },
        "upload_commit_request": {
            "sha256": sample_sha
        },
        "upload_committed_200": {
            "status": 200,
            "body": saved
        },
        "upload_whole_checksum_mismatch_422": {
            "status": UploadReasonCode::WholeFileChecksumMismatch.status().as_u16(),
            "body": {
                "error": "the uploaded file did not match its checksum, so your journal discarded the partial copy.",
                "reason_code": UploadReasonCode::WholeFileChecksumMismatch.as_wire(),
                "detail": "expected sha256 ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff, computed 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
                "expected_sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "actual_sha256": sample_sha
            }
        },
        "upload_abort_200": {
            "status": 200,
            "body": {
                "deleted": true,
                "upload_id": upload_id
            }
        },
        "host_import_status_uploading": {
            "id": "pick-1",
            "item": metadata["client_item_id"],
            "name": metadata["original_filename"],
            "state": "uploading",
            "device_noun": "iphone",
            "size_bytes": total_bytes,
            "bytes_sent": uploads::RESUMABLE_CHUNK_BYTES,
            "bytes_total": total_bytes,
            "resumable": true,
            "phase": "sending"
        },
        "refusals": ImportReasonCode::all().iter().map(|code| json!({"status": code.status().as_u16(), "reason_code": code.as_wire()})).collect::<Vec<_>>(),
        "upload_refusals": UploadReasonCode::all().iter().map(|code| json!({"status": code.status().as_u16(), "reason_code": code.as_wire()})).collect::<Vec<_>>()
    })
}

pub fn vectors_json() -> Value {
    let metadata = fixture_metadata();
    let saved = lifecycle::summary(&metadata);
    let replay = lifecycle::replay_summary(&metadata);
    let mut vectors = Map::new();
    extend_defs(&mut vectors, base_vectors(&metadata, &saved, &replay));
    extend_defs(&mut vectors, resumable_vectors(&metadata, &saved, &replay));
    json!({
        "source": SOURCE,
        "regeneration_command": REGENERATION_COMMAND,
        "vectors": Value::Object(vectors)
    })
}
