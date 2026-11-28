// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Health integrity & safe recovery HTTP APIs.
//!
//! Every route validates journal-relative paths strictly (rejecting path
//! traversal, absolute paths, symlinks, and non-target directories), never
//! leaks host filesystem paths, and refreshes the durable integrity receipt
//! after owner-confirmed recovery actions. Search recovery routes only enqueue
//! work hints and send a supervisor wakeup, honoring the single-index-writer
//! contract.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use axum::extract::Query;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, USER_AGENT, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_facets::facet_trash::{Facets, facet_name};
use solstone_core_facets::{FacetWriteError, declare_undeclared_facet};
use solstone_core_format::segment::is_date_key;
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_indexer_todo::{
    enqueue_clear_removed, enqueue_search_rebuild, enqueue_search_update,
};
use solstone_core_journal_io::contained_path;
use solstone_core_journal_io::jsonl_integrity::quarantine_damaged_jsonl_lines;
use solstone_core_processing_record::{MediaKind, media_kind};
use solstone_core_removals::{Action, Page, Surface};
use solstone_core_retention::layout::{parse_quarantined_ingest_rel, segment_rel};
use solstone_core_retention::original_deletion::record_original_missing_acknowledged;
use solstone_core_retention::original_trash::trash_originals;
use solstone_core_retention::quarantined_ingest_trash::QuarantinedIngest;
use solstone_core_retention::receipt::Target;
use solstone_core_segment::DEFAULT_STREAM;
use solstone_core_system_health::integrity::{
    ChecksumVerifyOptions, IntegrityScanOptions, IntegrityScopeKind, OLDER_MISSING_CUTOFF_DAY,
    read_integrity_receipt, run_integrity_scan, verify_kept_originals, write_integrity_receipt,
};
use solstone_core_trash::TrashError;

const MAX_SET_ASIDE_PATHS: usize = 200;
const MAX_RECORD_MISSING_ITEMS: usize = 5_000;
const MAX_QUARANTINED_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) fn routes(journal_root: PathBuf) -> Router {
    let check_root = journal_root.clone();
    let verify_root = journal_root.clone();
    let set_aside_root = journal_root.clone();
    let record_missing_root = journal_root.clone();
    let trash_orig_root = journal_root.clone();
    let dl_failed_root = journal_root.clone();
    let trash_failed_root = journal_root.clone();
    let facet_add_root = journal_root.clone();
    let facet_discard_root = journal_root.clone();
    let search_update_root = journal_root.clone();
    let search_clear_root = journal_root.clone();
    let search_rebuild_root = journal_root;

    Router::new()
        .route(
            "/app/health/api/integrity/check",
            post(move |body| check_integrity(check_root.clone(), body)),
        )
        .route(
            "/app/health/api/integrity/verify-originals",
            post(move || verify_originals(verify_root.clone())),
        )
        .route(
            "/app/health/api/integrity/set-aside-lines",
            post(move |body| set_aside_lines(set_aside_root.clone(), body)),
        )
        .route(
            "/app/health/api/integrity/record-missing",
            post(move |body| record_missing(record_missing_root.clone(), body)),
        )
        .route(
            "/app/health/api/integrity/trash-original",
            post(move |headers, body| trash_original(trash_orig_root.clone(), headers, body)),
        )
        .route(
            "/app/health/api/integrity/observer-failed/download",
            get(move |query| observer_failed_download(dl_failed_root.clone(), query)),
        )
        .route(
            "/app/health/api/integrity/observer-failed/trash",
            post(move |headers, body| {
                observer_failed_trash(trash_failed_root.clone(), headers, body)
            }),
        )
        .route(
            "/app/health/api/integrity/facets/add",
            post(move |body| facets_add(facet_add_root.clone(), body)),
        )
        .route(
            "/app/health/api/integrity/facets/discard",
            post(move |headers, body| facets_discard(facet_discard_root.clone(), headers, body)),
        )
        .route(
            "/app/health/api/search/update",
            post(move || search_update(search_update_root.clone())),
        )
        .route(
            "/app/health/api/search/clear-removed",
            post(move || search_clear_removed(search_clear_root.clone())),
        )
        .route(
            "/app/health/api/search/rebuild",
            post(move || search_rebuild(search_rebuild_root.clone())),
        )
}

fn error_response(status: StatusCode, reason_code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "status": "error",
            "reason_code": reason_code,
            "error": message,
        })),
    )
        .into_response()
}

fn health_surface(headers: &HeaderMap) -> Surface {
    Surface::of_request(
        Page::Health,
        headers.get(USER_AGENT).and_then(|v| v.to_str().ok()),
    )
}

fn trash_error_response(error: &TrashError) -> Response {
    match error {
        TrashError::Missing(_) | TrashError::NotInTrash => {
            error_response(StatusCode::NOT_FOUND, "not_found", "item not found")
        }
        TrashError::InFlight | TrashError::Unfinished(_) | TrashError::Held(_) => error_response(
            StatusCode::CONFLICT,
            "conflict",
            "item cannot be moved right now",
        ),
        TrashError::UnknownKind | TrashError::Io(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "trash_failed",
            "could not move item to trash",
        ),
    }
}

fn notify_supervisor_drain(root: &Path) -> bool {
    crate::actions::send(
        root,
        &solstone_core_callosum::CallosumEnvelope {
            tract: "supervisor".into(),
            event: "drain".into(),
            ts: None,
            extra: serde_json::Map::new(),
        },
    )
}

fn journal_relative_slash(root: &Path, path: &Path) -> Option<String> {
    let owned_rel = if let Ok(rel) = path.strip_prefix(root) {
        rel.to_path_buf()
    } else if let Ok(canonical_root) = fs::canonicalize(root) {
        if let Ok(rel) = path.strip_prefix(&canonical_root) {
            rel.to_path_buf()
        } else {
            fs::canonicalize(path)
                .ok()?
                .strip_prefix(&canonical_root)
                .ok()?
                .to_path_buf()
        }
    } else {
        return None;
    };
    let mut parts = Vec::new();
    for component in owned_rel.components() {
        let Component::Normal(part) = component else {
            return None;
        };
        parts.push(part.to_str()?);
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn validate_single_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && Path::new(name)
            .components()
            .collect::<Vec<_>>()
            .as_slice()
            .len()
            == 1
        && matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
}

fn is_owner_transcript_leaf(name: &str) -> bool {
    if !validate_single_component(name) || !name.ends_with(".jsonl") {
        return false;
    }
    name == "audio.jsonl"
        || name == "screen.jsonl"
        || name == "conversation_transcript.jsonl"
        || name == "imported_audio.jsonl"
        || name.starts_with("browser_")
        || name.ends_with("_desktop_activity.jsonl")
        || (name.starts_with('p') && name.len() > 7 && name.as_bytes()[1].is_ascii_digit())
}

fn is_raw_media_leaf(name: &str) -> bool {
    if !validate_single_component(name) {
        return false;
    }
    let Some(ext) = Path::new(name).extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let lower = ext.to_ascii_lowercase();
    matches!(
        media_kind(&lower),
        Some(MediaKind::Audio | MediaKind::Video | MediaKind::Image)
    ) || lower == "pdf"
        || solstone_core_doc_extract::is_supported_extension(&lower)
}

/// Parse a journal-relative chronicle segment directory path:
/// - `chronicle/<day>/<segment>` -> `(day, "_default", segment)`
/// - `chronicle/<day>/<stream>/<segment>` -> `(day, stream, segment)`
fn parse_segment_dir_rel(rel: &str) -> Option<(String, String, String)> {
    if rel.contains('\\') || rel.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        ["chronicle", day, segment]
            if is_date_key(day)
                && validate_single_component(segment)
                && *segment != "observer"
                && *segment != "health" =>
        {
            Some((
                (*day).to_owned(),
                DEFAULT_STREAM.to_owned(),
                (*segment).to_owned(),
            ))
        }
        ["chronicle", day, stream, segment]
            if is_date_key(day)
                && validate_single_component(stream)
                && *stream != "observer"
                && *stream != "health"
                && validate_single_component(segment) =>
        {
            Some((
                (*day).to_owned(),
                (*stream).to_owned(),
                (*segment).to_owned(),
            ))
        }
        _ => None,
    }
}

/// Parse a journal-relative chronicle segment file path:
/// - `chronicle/<day>/<segment>/<file>` -> `(day, "_default", segment, file)`
/// - `chronicle/<day>/<stream>/<segment>/<file>` -> `(day, stream, segment, file)`
fn parse_segment_file_rel(rel: &str) -> Option<(String, String, String, String)> {
    if rel.contains('\\') || rel.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        ["chronicle", day, segment, file]
            if is_date_key(day)
                && validate_single_component(segment)
                && *segment != "observer"
                && *segment != "health"
                && validate_single_component(file) =>
        {
            Some((
                (*day).to_owned(),
                DEFAULT_STREAM.to_owned(),
                (*segment).to_owned(),
                (*file).to_owned(),
            ))
        }
        ["chronicle", day, stream, segment, file]
            if is_date_key(day)
                && validate_single_component(stream)
                && *stream != "observer"
                && *stream != "health"
                && validate_single_component(segment)
                && validate_single_component(file) =>
        {
            Some((
                (*day).to_owned(),
                (*stream).to_owned(),
                (*segment).to_owned(),
                (*file).to_owned(),
            ))
        }
        _ => None,
    }
}

fn validate_segment_target(day: &str, stream: &str, segment: &str) -> Option<Target> {
    if !is_date_key(day) || !validate_single_component(segment) {
        return None;
    }
    let normalized_stream = if stream.is_empty() || stream == DEFAULT_STREAM {
        DEFAULT_STREAM
    } else if validate_single_component(stream) && stream != "observer" && stream != "health" {
        stream
    } else {
        return None;
    };
    Some(Target {
        day: day.to_owned(),
        stream: normalized_stream.to_owned(),
        dir: segment.to_owned(),
    })
}

async fn check_integrity(root: PathBuf, body: Option<Json<Value>>) -> Response {
    let mode = body
        .as_ref()
        .and_then(|Json(v)| v.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("routine");
    let scope = match mode {
        "routine" => IntegrityScopeKind::Routine,
        "continue" => IntegrityScopeKind::Continue,
        "full" => IntegrityScopeKind::Full,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_mode",
                "mode must be routine, continue, or full",
            );
        }
    };
    let result = tokio::task::spawn_blocking(move || {
        run_integrity_scan(
            &root,
            IntegrityScanOptions {
                scope,
                ..Default::default()
            },
        )
    })
    .await;
    match result {
        Ok(Ok(receipt)) => Json(json!({ "status": "ok", "integrity": receipt })).into_response(),
        Ok(Err(_)) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "integrity_check_failed",
            "integrity check could not complete",
        ),
    }
}

async fn verify_originals(root: PathBuf) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        verify_kept_originals(&root, ChecksumVerifyOptions::default())
    })
    .await;
    match result {
        Ok(Ok(receipt)) => Json(json!({ "status": "ok", "integrity": receipt })).into_response(),
        Ok(Err(_)) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "verify_originals_failed",
            "original checksum verification could not complete",
        ),
    }
}

async fn set_aside_lines(root: PathBuf, body: Option<Json<Value>>) -> Response {
    let Some(Json(payload)) = body else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request body is required",
        );
    };
    let mut raw_paths: Vec<String> = Vec::new();
    if let Some(single) = payload.get("path").and_then(Value::as_str) {
        raw_paths.push(single.to_owned());
    }
    if let Some(list) = payload.get("paths").and_then(Value::as_array) {
        for item in list {
            let Some(p) = item.as_str() else {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_path",
                    "paths must contain strings",
                );
            };
            raw_paths.push(p.to_owned());
        }
    }
    if raw_paths.is_empty() || raw_paths.len() > MAX_SET_ASIDE_PATHS {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_path",
            "provide between 1 and 200 transcript paths",
        );
    }

    let mut validated: Vec<(String, PathBuf)> = Vec::with_capacity(raw_paths.len());
    let mut seen = BTreeSet::new();
    for rel in raw_paths {
        let Some((day, stream, segment, file)) = parse_segment_file_rel(&rel) else {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_path",
                "path must name a chronicle transcript file",
            );
        };
        if !is_owner_transcript_leaf(&file) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_path",
                "path must name an owner transcript jsonl file",
            );
        }
        let canonical_rel = format!("{}/{file}", segment_rel(&day, &stream, &segment));
        if !seen.insert(canonical_rel.clone()) {
            continue;
        }
        let Ok(full) = contained_path(&root, &canonical_rel) else {
            return error_response(StatusCode::BAD_REQUEST, "invalid_path", "invalid path");
        };
        let meta = match fs::symlink_metadata(&full) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return error_response(StatusCode::NOT_FOUND, "not_found", "transcript not found");
            }
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_path",
                    "path cannot be read",
                );
            }
        };
        if !meta.file_type().is_file() {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_path",
                "path must be a regular file",
            );
        }
        validated.push((canonical_rel, full));
    }

    let result = tokio::task::spawn_blocking(move || {
        let mut results = Vec::with_capacity(validated.len());
        for (rel, full) in validated {
            let outcome = quarantine_damaged_jsonl_lines(&full)?;
            let aside_rel = outcome
                .aside_path
                .as_deref()
                .and_then(|p| journal_relative_slash(&root, p));
            results.push(json!({
                "path": rel,
                "kept_lines": outcome.kept_lines,
                "quarantined_lines": outcome.set_aside.len(),
                "aside_path": aside_rel,
            }));
        }
        let receipt = run_integrity_scan(&root, IntegrityScanOptions::default())?;
        Ok::<_, std::io::Error>((results, receipt))
    })
    .await;

    match result {
        Ok(Ok((results, receipt))) => Json(json!({
            "status": "ok",
            "results": results,
            "integrity": receipt,
        }))
        .into_response(),
        Ok(Err(_)) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "set_aside_failed",
            "could not set aside damaged lines",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct RecordMissingSegmentInput {
    path: Option<String>,
    day: Option<String>,
    stream: Option<String>,
    segment: Option<String>,
    file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecordMissingBody {
    path: Option<String>,
    day: Option<String>,
    stream: Option<String>,
    segment: Option<String>,
    file: Option<String>,
    segments: Option<Vec<RecordMissingSegmentInput>>,
    #[serde(default)]
    older_missing_originals: bool,
}

fn resolve_missing_item(
    path: Option<&str>,
    day: Option<&str>,
    stream: Option<&str>,
    segment: Option<&str>,
    file: Option<&str>,
) -> Option<(Target, Option<String>)> {
    let target = if let Some(rel) = path {
        let (d, s, seg) = parse_segment_dir_rel(rel)?;
        validate_segment_target(&d, &s, &seg)?
    } else {
        let d = day?;
        let seg = segment?;
        let s = stream.unwrap_or(DEFAULT_STREAM);
        validate_segment_target(d, s, seg)?
    };
    let validated_file = match file.filter(|f| !f.is_empty()) {
        None => None,
        Some(f) if validate_single_component(f) => Some(f.to_owned()),
        Some(_) => return None,
    };
    Some((target, validated_file))
}

async fn record_missing(root: PathBuf, body: Option<Json<Value>>) -> Response {
    let Some(Json(raw_body)) = body else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request body is required",
        );
    };
    let Ok(req) = serde_json::from_value::<RecordMissingBody>(raw_body) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid request payload",
        );
    };

    let result = tokio::task::spawn_blocking(move || {
        let mut items: Vec<(Target, Option<String>)> = Vec::new();
        if req.older_missing_originals {
            let current = read_integrity_receipt(&root);
            for entry in current.older_missing_originals {
                if entry.day.as_str() >= OLDER_MISSING_CUTOFF_DAY {
                    continue;
                }
                if let Some((target, _)) = resolve_missing_item(
                    Some(&entry.path),
                    Some(&entry.day),
                    Some(&entry.stream),
                    Some(&entry.segment),
                    None,
                ) {
                    items.push((target, None));
                }
            }
        } else if let Some(list) = req.segments {
            if list.is_empty() || list.len() > MAX_RECORD_MISSING_ITEMS {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "segments must be non-empty",
                ));
            }
            for entry in list {
                let Some(resolved) = resolve_missing_item(
                    entry.path.as_deref(),
                    entry.day.as_deref(),
                    entry.stream.as_deref(),
                    entry.segment.as_deref(),
                    entry.file.as_deref(),
                ) else {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "invalid_path",
                        "invalid segment specification",
                    ));
                };
                items.push(resolved);
            }
        } else {
            let Some(resolved) = resolve_missing_item(
                req.path.as_deref(),
                req.day.as_deref(),
                req.stream.as_deref(),
                req.segment.as_deref(),
                req.file.as_deref(),
            ) else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "invalid_path",
                    "provide path or day/segment, segments list, or older_missing_originals: true",
                ));
            };
            items.push(resolved);
        }

        let mut recorded = 0usize;
        let mut acknowledged_rels = BTreeSet::new();
        for (target, file) in items {
            let rel = segment_rel(&target.day, &target.stream, &target.dir);
            let Ok(seg_dir) = contained_path(&root, &rel) else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "invalid_path",
                    "invalid segment path",
                ));
            };
            if !fs::symlink_metadata(&seg_dir).is_ok_and(|m| m.file_type().is_dir()) {
                if req.older_missing_originals {
                    continue;
                }
                return Err((StatusCode::NOT_FOUND, "not_found", "segment not found"));
            }
            record_original_missing_acknowledged(
                &seg_dir,
                file.as_deref(),
                jiff::Timestamp::now().as_millisecond(),
            )
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "record_missing_failed",
                    "could not record missing original acknowledgment",
                )
            })?;
            recorded += 1;
            acknowledged_rels.insert(rel);
        }

        let mut receipt =
            run_integrity_scan(&root, IntegrityScanOptions::default()).map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "integrity_check_failed",
                    "could not refresh integrity receipt",
                )
            })?;
        // If acknowledging older entries outside the routine rolling window,
        // ensure they are cleared from the receipt immediately as well.
        if !acknowledged_rels.is_empty() {
            let before_older = receipt.older_missing_originals.len();
            let before_findings = receipt.findings.len();
            receipt
                .older_missing_originals
                .retain(|entry| !acknowledged_rels.contains(&entry.path));
            receipt.findings.retain(|finding| {
                !(finding.kind == "missing_original"
                    && finding
                        .path
                        .as_deref()
                        .is_some_and(|p| acknowledged_rels.contains(p)))
            });
            if receipt.older_missing_originals.len() != before_older
                || receipt.findings.len() != before_findings
            {
                receipt.recompute_verdict();
                let _ = write_integrity_receipt(&root, &receipt);
            }
        }
        Ok((recorded, receipt))
    })
    .await;

    match result {
        Ok(Ok((recorded, receipt))) => Json(json!({
            "status": "ok",
            "recorded": recorded,
            "integrity": receipt,
        }))
        .into_response(),
        Ok(Err((status, code, msg))) => error_response(status, code, msg),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "record_missing_failed",
            "could not record missing original acknowledgment",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct TrashOriginalBody {
    path: Option<String>,
    day: Option<String>,
    stream: Option<String>,
    segment: Option<String>,
    file: Option<String>,
}

async fn trash_original(root: PathBuf, headers: HeaderMap, body: Option<Json<Value>>) -> Response {
    let Some(Json(raw_body)) = body else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request body is required",
        );
    };
    let Ok(req) = serde_json::from_value::<TrashOriginalBody>(raw_body) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid request payload",
        );
    };
    let (target, file) = if let Some(rel) = req.path.as_deref() {
        let Some((day, stream, segment, file)) = parse_segment_file_rel(rel) else {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_path",
                "path must name a chronicle original media file",
            );
        };
        let Some(target) = validate_segment_target(&day, &stream, &segment) else {
            return error_response(StatusCode::BAD_REQUEST, "invalid_path", "invalid segment");
        };
        (target, file)
    } else {
        let (Some(day), Some(segment), Some(file)) = (
            req.day.as_deref(),
            req.segment.as_deref(),
            req.file.as_deref(),
        ) else {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_path",
                "provide path or day, segment, and file",
            );
        };
        let stream = req.stream.as_deref().unwrap_or(DEFAULT_STREAM);
        let Some(target) = validate_segment_target(day, stream, segment) else {
            return error_response(StatusCode::BAD_REQUEST, "invalid_path", "invalid segment");
        };
        (target, file.to_owned())
    };
    if !is_raw_media_leaf(&file) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_path",
            "only original media files may be trashed through this endpoint",
        );
    }

    let surface = health_surface(&headers);
    let result = tokio::task::spawn_blocking(move || {
        let rel = format!(
            "{}/{file}",
            segment_rel(&target.day, &target.stream, &target.dir)
        );
        let Ok(full) = contained_path(&root, &rel) else {
            return Err(Err((
                StatusCode::BAD_REQUEST,
                "invalid_path",
                "invalid path",
            )));
        };
        let file_len = match fs::symlink_metadata(&full) {
            Ok(meta) if meta.file_type().is_file() => meta.len(),
            Ok(_) => {
                return Err(Err((
                    StatusCode::BAD_REQUEST,
                    "invalid_path",
                    "target must be a regular file",
                )));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(Err((
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "original media file not found",
                )));
            }
            Err(_) => {
                return Err(Err((
                    StatusCode::BAD_REQUEST,
                    "invalid_path",
                    "target cannot be inspected",
                )));
            }
        };
        let is_checksum_mismatch = file_len > 0
            && read_integrity_receipt(&root)
                .findings
                .iter()
                .any(|finding| {
                    finding.kind == "checksum_mismatch"
                        && finding.path.as_deref() == Some(rel.as_str())
                });
        if file_len > 0 && !is_checksum_mismatch {
            return Err(Err((
                StatusCode::CONFLICT,
                "original_not_damaged",
                "only a zero-byte or checksum-mismatched original may be trashed from health",
            )));
        }
        let item = trash_originals(
            &root,
            &target,
            vec![file],
            Action::OriginalsRelease,
            surface,
        )
        .map_err(Ok)?;
        let receipt = run_integrity_scan(&root, IntegrityScanOptions::default()).map_err(|_| {
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "integrity_check_failed",
                "original trashed but integrity receipt refresh failed",
            ))
        })?;
        Ok((item, receipt))
    })
    .await;

    match result {
        Ok(Ok((item, receipt))) => Json(json!({
            "status": "ok",
            "trash": {
                "id": item.id,
                "erase_at": item.erase_at,
            },
            "integrity": receipt,
        }))
        .into_response(),
        Ok(Err(Ok(trash_err))) => trash_error_response(&trash_err),
        Ok(Err(Err((status, code, msg)))) => error_response(status, code, msg),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "trash_failed",
            "could not trash original file",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct PathQuery {
    path: Option<String>,
}

async fn observer_failed_download(root: PathBuf, Query(query): Query<PathQuery>) -> Response {
    let Some(rel) = query.path.filter(|p| !p.is_empty()) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_path",
            "path query parameter is required",
        );
    };
    let Some((_, _, _, leaf)) = parse_quarantined_ingest_rel(&rel) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_path",
            "path must reference a quarantined failed-ingest file",
        );
    };

    let result = tokio::task::spawn_blocking(move || {
        let Ok(full) = contained_path(&root, &rel) else {
            return Err((StatusCode::BAD_REQUEST, "invalid_path", "invalid path"));
        };
        let meta = match fs::symlink_metadata(&full) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err((
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "quarantined file not found",
                ));
            }
            Err(_) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "invalid_path",
                    "cannot inspect path",
                ));
            }
        };
        if !meta.file_type().is_file() {
            return Err((
                StatusCode::BAD_REQUEST,
                "invalid_path",
                "target must be a regular file",
            ));
        }
        if meta.len() > MAX_QUARANTINED_DOWNLOAD_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "file_too_large",
                "quarantined file exceeds download limit",
            ));
        }
        let bytes = fs::read(&full).map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "read_failed",
                "could not read quarantined file",
            )
        })?;
        Ok((leaf, bytes))
    })
    .await;

    match result {
        Ok(Ok((leaf, bytes))) => {
            let safe_leaf: String = leaf
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let disposition = format!("attachment; filename=\"{safe_leaf}\"");
            let mut headers = HeaderMap::new();
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if let Ok(val) = HeaderValue::from_str(&disposition) {
                headers.insert(CONTENT_DISPOSITION, val);
            }
            (StatusCode::OK, headers, bytes).into_response()
        }
        Ok(Err((status, code, msg))) => error_response(status, code, msg),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "read_failed",
            "could not read quarantined file",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct PathBody {
    path: Option<String>,
}

async fn observer_failed_trash(
    root: PathBuf,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let Some(Json(raw_body)) = body else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request body is required",
        );
    };
    let Ok(req) = serde_json::from_value::<PathBody>(raw_body) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid request payload",
        );
    };
    let Some(rel) = req.path.filter(|p| !p.is_empty()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_path", "path is required");
    };
    if parse_quarantined_ingest_rel(&rel).is_none() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_path",
            "path must reference a quarantined failed-ingest file",
        );
    }

    let surface = health_surface(&headers);
    let result = tokio::task::spawn_blocking(move || {
        let item = QuarantinedIngest::trash(&root, &rel, Action::QuarantinedIngestDiscard, surface)
            .map_err(Ok)?;
        let receipt = run_integrity_scan(&root, IntegrityScanOptions::default()).map_err(|_| {
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "integrity_check_failed",
                "file trashed but integrity receipt refresh failed",
            ))
        })?;
        Ok((item, receipt))
    })
    .await;

    match result {
        Ok(Ok((item, receipt))) => Json(json!({
            "status": "ok",
            "trash": {
                "id": item.id,
                "erase_at": item.erase_at,
            },
            "integrity": receipt,
        }))
        .into_response(),
        Ok(Err(Ok(trash_err))) => trash_error_response(&trash_err),
        Ok(Err(Err((status, code, msg)))) => error_response(status, code, msg),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "trash_failed",
            "could not trash quarantined file",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct FacetAddBody {
    facet: Option<String>,
    title: Option<String>,
}

async fn facets_add(root: PathBuf, body: Option<Json<Value>>) -> Response {
    let Some(Json(raw_body)) = body else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request body is required",
        );
    };
    let Ok(req) = serde_json::from_value::<FacetAddBody>(raw_body) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid request payload",
        );
    };
    let Some(facet) = req.facet.filter(|f| facet_name(f)) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_facet",
            "valid facet directory name is required",
        );
    };
    let title = req.title;
    let result = tokio::task::spawn_blocking(move || {
        declare_undeclared_facet(&root, &facet, title.as_deref()).map_err(|err| match err {
            FacetWriteError::DeclarationMissing { .. } => (
                StatusCode::NOT_FOUND,
                "not_found",
                "undeclared facet folder not found",
            ),
            FacetWriteError::AlreadyExists { .. } => (
                StatusCode::CONFLICT,
                "already_declared",
                "facet is already declared",
            ),
            FacetWriteError::RetiredFileDamaged { .. } => (
                StatusCode::CONFLICT,
                "record_damaged",
                "facets retired record is damaged",
            ),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "facet_add_failed",
                "could not declare facet folder",
            ),
        })?;
        let receipt = run_integrity_scan(&root, IntegrityScanOptions::default()).map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "integrity_check_failed",
                "facet declared but integrity receipt refresh failed",
            )
        })?;
        Ok((facet, receipt))
    })
    .await;

    match result {
        Ok(Ok((facet, receipt))) => Json(json!({
            "status": "ok",
            "facet": facet,
            "integrity": receipt,
        }))
        .into_response(),
        Ok(Err((status, code, msg))) => error_response(status, code, msg),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "facet_add_failed",
            "could not declare facet folder",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct FacetDiscardBody {
    facet: Option<String>,
}

async fn facets_discard(root: PathBuf, headers: HeaderMap, body: Option<Json<Value>>) -> Response {
    let Some(Json(raw_body)) = body else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "request body is required",
        );
    };
    let Ok(req) = serde_json::from_value::<FacetDiscardBody>(raw_body) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid request payload",
        );
    };
    let Some(facet) = req.facet.filter(|f| facet_name(f)) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_facet",
            "valid facet directory name is required",
        );
    };
    let surface = health_surface(&headers);
    let result = tokio::task::spawn_blocking(move || {
        let index = RetentionIndex::new(&root);
        let item = Facets::new(&index)
            .trash_undeclared(&root, &facet, Action::FacetDelete, surface)
            .map_err(Ok)?;
        let receipt = run_integrity_scan(&root, IntegrityScanOptions::default()).map_err(|_| {
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "integrity_check_failed",
                "facet trashed but integrity receipt refresh failed",
            ))
        })?;
        Ok((facet, item, receipt))
    })
    .await;

    match result {
        Ok(Ok((facet, item, receipt))) => Json(json!({
            "status": "ok",
            "facet": facet,
            "trash": {
                "id": item.id,
                "erase_at": item.erase_at,
            },
            "integrity": receipt,
        }))
        .into_response(),
        Ok(Err(Ok(trash_err))) => trash_error_response(&trash_err),
        Ok(Err(Err((status, code, msg)))) => error_response(status, code, msg),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "trash_failed",
            "could not trash undeclared facet folder",
        ),
    }
}

async fn search_update(root: PathBuf) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        enqueue_search_update(&root)?;
        let notified = notify_supervisor_drain(&root);
        Ok::<_, std::io::Error>(notified)
    })
    .await;
    match result {
        Ok(Ok(notified)) => Json(json!({
            "status": "queued",
            "action": "update",
            "supervisor_notified": notified,
        }))
        .into_response(),
        Ok(Err(_)) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "search_queue_failed",
            "could not queue search update",
        ),
    }
}

async fn search_clear_removed(root: PathBuf) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        enqueue_clear_removed(&root)?;
        let notified = notify_supervisor_drain(&root);
        Ok::<_, std::io::Error>(notified)
    })
    .await;
    match result {
        Ok(Ok(notified)) => Json(json!({
            "status": "queued",
            "action": "clear_removed",
            "supervisor_notified": notified,
        }))
        .into_response(),
        Ok(Err(_)) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "search_queue_failed",
            "could not queue removed-entries cleanup",
        ),
    }
}

async fn search_rebuild(root: PathBuf) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        enqueue_search_rebuild(&root)?;
        let notified = notify_supervisor_drain(&root);
        Ok::<_, std::io::Error>(notified)
    })
    .await;
    match result {
        Ok(Ok(notified)) => Json(json!({
            "status": "queued",
            "action": "rebuild",
            "supervisor_notified": notified,
        }))
        .into_response(),
        Ok(Err(_)) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "search_queue_failed",
            "could not queue search rebuild",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    async fn post_json(app: Router, uri: &str, payload: Value) -> (StatusCode, Value) {
        let resp = app
            .oneshot(
                Request::post(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        (status, body)
    }

    #[tokio::test]
    async fn integrity_check_and_verify_originals_routes_work_end_to_end() {
        let root = crate::test_support::root();
        let seg = root.path().join("chronicle/20261120/100000_60");
        fs::create_dir_all(&seg).unwrap();
        let audio = b"hello-audio";
        let sha = format!("{:x}", Sha256::digest(audio));
        fs::write(seg.join("audio.ogg"), audio).unwrap();
        fs::write(
            seg.join("ingest.json"),
            json!({"files": {"audio.ogg": {"sha256": sha}}}).to_string(),
        )
        .unwrap();

        let (status, bad) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/check",
            json!({"mode": "bogus"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(bad["reason_code"], "invalid_mode");

        let (status, checked) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/check",
            json!({"mode": "full"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(checked["status"], "ok");
        assert_eq!(checked["integrity"]["days_checked"], 1);

        let (status, verified) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/verify-originals",
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(verified["integrity"]["checksums"]["checksums_verified"], 1);
        assert_eq!(
            verified["integrity"]["checksums"]["checksums_total_kept"],
            1
        );
        assert_eq!(verified["integrity"]["checksums"]["mismatches"], 0);
    }

    #[tokio::test]
    async fn recovery_endpoints_validate_paths_recover_cleanly_and_refresh_receipt() {
        let root = crate::test_support::root();

        // 1. Damaged transcript JSONL -> set-aside-lines
        let seg = root.path().join("chronicle/20261120/100000_60");
        fs::create_dir_all(&seg).unwrap();
        fs::write(seg.join("audio.ogg"), b"").unwrap();
        fs::write(
            seg.join("audio.jsonl"),
            "{\"start\":\"10:00:00\",\"text\":\"good\"}\n{broken\n",
        )
        .unwrap();

        // Path traversal rejected
        let (status, err) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/set-aside-lines",
            json!({"path": "chronicle/20261120/../../config/journal.json"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(err["reason_code"], "invalid_path");

        let (status, set_aside) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/set-aside-lines",
            json!({"path": "chronicle/20261120/100000_60/audio.jsonl"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{set_aside}");
        assert_eq!(set_aside["results"][0]["kept_lines"], 1);
        assert_eq!(set_aside["results"][0]["quarantined_lines"], 1);
        assert!(
            set_aside["results"][0]["aside_path"]
                .as_str()
                .is_some_and(|p| p.starts_with("chronicle/20261120/100000_60/")),
            "aside path must be journal-relative: {set_aside}"
        );

        // 2. Trash damaged 0-byte original (transcript stays intact!)
        let (status, trashed) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/trash-original",
            json!({"path": "chronicle/20261120/100000_60/audio.ogg"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{trashed}");
        assert!(trashed["trash"]["id"].is_string());
        assert!(!seg.join("audio.ogg").exists());
        assert!(seg.join("audio.jsonl").is_file());

        // Refuses trashing non-media files
        let (status, err) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/trash-original",
            json!({"path": "chronicle/20261120/100000_60/audio.jsonl"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(err["reason_code"], "invalid_path");

        // Refuses trashing a healthy non-zero original without a checksum_mismatch finding
        let healthy_seg = root.path().join("chronicle/20261120/140000_60");
        fs::create_dir_all(&healthy_seg).unwrap();
        fs::write(healthy_seg.join("audio.ogg"), b"valid-audio-bytes").unwrap();
        let (status, err) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/trash-original",
            json!({"path": "chronicle/20261120/140000_60/audio.ogg"}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(err["reason_code"], "original_not_damaged");
        assert!(healthy_seg.join("audio.ogg").is_file());

        // 3. Record missing original (both post-cutoff single and pre-cutoff bulk)
        let missing_recent = root.path().join("chronicle/20261005/110000_60");
        fs::create_dir_all(&missing_recent).unwrap();
        fs::write(
            missing_recent.join("audio.jsonl"),
            "{\"start\":\"11:00:00\",\"text\":\"hi\"}\n",
        )
        .unwrap();
        let missing_old = root.path().join("chronicle/20260801/120000_60");
        fs::create_dir_all(&missing_old).unwrap();
        fs::write(
            missing_old.join("audio.jsonl"),
            "{\"start\":\"12:00:00\",\"text\":\"old\"}\n",
        )
        .unwrap();

        let _ = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/check",
            json!({"mode": "full"}),
        )
        .await;

        let (status, ack_single) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/record-missing",
            json!({"path": "chronicle/20261005/110000_60"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{ack_single}");
        assert_eq!(ack_single["recorded"], 1);
        assert!(
            solstone_core_retention::recorded_original_deletions(&missing_recent).is_empty(),
            "acknowledging missing original must never fabricate original_deleted"
        );

        let (status, ack_old) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/record-missing",
            json!({"older_missing_originals": true}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{ack_old}");
        assert_eq!(ack_old["recorded"], 1);
        assert!(
            ack_old["integrity"]["older_missing_originals"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        // 4. Observer/failed download + trash
        let q_rel = "chronicle/20261120/observer/failed/130000_60/1700000000000/audio.ogg";
        let q_full = root.path().join(q_rel);
        fs::create_dir_all(q_full.parent().unwrap()).unwrap();
        fs::write(&q_full, b"stranded-payload").unwrap();

        // Reject traversal on download
        let bad_dl = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::get(
                    "/app/health/api/integrity/observer-failed/download?path=config/journal.json",
                )
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad_dl.status(), StatusCode::BAD_REQUEST);

        let dl = crate::routes(root.path().to_path_buf())
            .oneshot(
                Request::get(format!(
                    "/app/health/api/integrity/observer-failed/download?path={q_rel}"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dl.status(), StatusCode::OK);
        assert_eq!(
            dl.headers()
                .get(X_CONTENT_TYPE_OPTIONS)
                .and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
        let dl_bytes = to_bytes(dl.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&dl_bytes[..], b"stranded-payload");

        let (status, q_trashed) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/observer-failed/trash",
            json!({"path": q_rel}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{q_trashed}");
        assert!(q_trashed["trash"]["id"].is_string());
        assert!(!q_full.exists());

        // 5. Undeclared facet add & discard
        fs::create_dir_all(root.path().join("facets/garden")).unwrap();
        fs::create_dir_all(root.path().join("facets/tempfolder")).unwrap();

        let (status, added) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/facets/add",
            json!({"facet": "garden", "title": "Garden"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{added}");
        assert!(root.path().join("facets/garden/facet.json").is_file());

        let (status, discarded) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/facets/discard",
            json!({"facet": "tempfolder"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{discarded}");
        assert!(discarded["trash"]["id"].is_string());
        assert!(!root.path().join("facets/tempfolder").exists());

        // 6. Async search recovery routes enqueue todo tokens without touching SQLite
        let (status, upd) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/search/update",
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(upd["status"], "queued");

        let (status, clr) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/search/clear-removed",
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(clr["status"], "queued");

        let (status, reb) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/search/rebuild",
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reb["status"], "queued");

        let todo = solstone_core_indexer_todo::inspect(root.path()).unwrap();
        assert!(todo.keys.iter().any(|k| k == "@scan-full"));
        assert!(todo.keys.iter().any(|k| k == "@rebuild"));
    }

    #[tokio::test]
    async fn set_aside_lines_accepts_imported_transcript_leaves_and_rejects_other_jsonls() {
        let root = crate::test_support::root();
        let seg = root
            .path()
            .join("chronicle/20261120/import.chatgpt/090000_60");
        fs::create_dir_all(&seg).unwrap();
        fs::write(
            seg.join("conversation_transcript.jsonl"),
            "{\"text\":\"keep\"}\n{bad\n",
        )
        .unwrap();
        fs::write(
            seg.join("imported_audio.jsonl"),
            "{\"text\":\"keep2\"}\n{bad2\n",
        )
        .unwrap();
        fs::write(seg.join("events.jsonl"), "{bad\n").unwrap();
        fs::write(seg.join("photo_01.jsonl"), "{bad\n").unwrap();

        let conv_rel = "chronicle/20261120/import.chatgpt/090000_60/conversation_transcript.jsonl";
        let audio_rel = "chronicle/20261120/import.chatgpt/090000_60/imported_audio.jsonl";

        let (status, ok) = post_json(
            crate::routes(root.path().to_path_buf()),
            "/app/health/api/integrity/set-aside-lines",
            json!({"paths": [conv_rel, audio_rel]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{ok}");
        assert_eq!(ok["results"].as_array().unwrap().len(), 2);
        assert_eq!(ok["results"][0]["quarantined_lines"], 1);
        assert_eq!(ok["results"][1]["quarantined_lines"], 1);

        for rejected in [
            "chronicle/20261120/import.chatgpt/090000_60/events.jsonl",
            "chronicle/20261120/import.chatgpt/090000_60/photo_01.jsonl",
        ] {
            let (status, body) = post_json(
                crate::routes(root.path().to_path_buf()),
                "/app/health/api/integrity/set-aside-lines",
                json!({"path": rejected}),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}: {body}");
        }
    }
}
