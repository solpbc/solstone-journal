// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! HTTP routes for the phase 1 email source (`/app/sources/api/email/*`).
//!
//! Aligns with the shared `/app/sources/api/<source>` contract:
//! - `GET    /app/sources/api/email`
//! - `GET    /app/sources/api/email/choice-preview`
//! - `POST   /app/sources/api/email/connect`
//! - `POST   /app/sources/api/email/scan/resume`
//! - `POST   /app/sources/api/email/scan/cancel`
//! - `POST   /app/sources/api/email/choice`
//! - `POST   /app/sources/api/email/sync`
//! - `PUT    /app/sources/api/email/password`
//! - `DELETE /app/sources/api/email`
//! - `GET    /app/sources/api/email/runs/{id}/delete`
//! - `DELETE /app/sources/api/email/runs/{id}`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{
    Router,
    body::Bytes,
    extract::{Path as AxumPath, Query},
    http::{HeaderMap, StatusCode, header::USER_AGENT},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_email::error::EmailError;
use solstone_core_email::imap::{GmailImap, ImapSource};
use solstone_core_email::model::{MessageType, RangePreset};
use solstone_core_email::store::{self, EMAIL_STREAM, PROVIDER_GMAIL};
use solstone_core_email::sync::{
    self, CreatedEmailSegment, SyncOptions, find_run, is_valid_run_id, run_day_bounds,
    run_live_message_count, run_live_segments,
};
use solstone_core_import::publish::{CreatedSegment, PublicationInput, PublicationStatus, publish};
use solstone_core_removals::{Action, Page, Surface, Unit};
use solstone_core_segment::{ImportSource, Kind as SegmentKind, StreamHints};

use crate::http::json_response;
use crate::request_body::{JsonBody, json_body};

/// Injectable IMAP connection factory so production uses live read-only Gmail
/// TLS/IMAP while tests inject an in-memory mock IMAP server.
pub(crate) type ImapOpener = Arc<
    dyn Fn(&str, &str) -> solstone_core_email::error::Result<Box<dyn ImapSource + Send>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub(crate) struct EmailService {
    root: PathBuf,
    busy: Arc<AtomicBool>,
    opener: ImapOpener,
}

impl EmailService {
    pub(crate) fn live(root: PathBuf) -> Self {
        let opener: ImapOpener = Arc::new(|account, app_password| {
            let client = GmailImap::connect_gmail(account, app_password)?;
            Ok(Box::new(client))
        });
        Self::with_opener(root, opener)
    }

    pub(crate) fn with_opener(root: PathBuf, opener: ImapOpener) -> Self {
        Self {
            root,
            busy: Arc::new(AtomicBool::new(false)),
            opener,
        }
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Fast non-blocking check used by `/api/shell/badges` to surface email
/// `needs_attention` on the `sources` rail item and Home attention count.
#[must_use]
pub fn needs_attention(root: &Path) -> bool {
    sync::needs_attention(root)
}

pub(crate) fn routes_with_service(svc: EmailService) -> Router {
    let s_get = svc.clone();
    let s_delete = svc.clone();
    let s_preview = svc.clone();
    let s_connect = svc.clone();
    let s_resume = svc.clone();
    let s_cancel = svc.clone();
    let s_choice = svc.clone();
    let s_sync = svc.clone();
    let s_password = svc.clone();
    let s_run_preview = svc.clone();
    let s_run_delete = svc;

    Router::new()
        .route(
            "/app/sources/api/email",
            get(move || get_status(s_get.clone()))
                .delete(move || disconnect_email(s_delete.clone())),
        )
        .route(
            "/app/sources/api/email/choice-preview",
            get(move |query| get_choice_preview(s_preview.clone(), query)),
        )
        .route(
            "/app/sources/api/email/connect",
            post(move |body| connect_email(s_connect.clone(), body)),
        )
        .route(
            "/app/sources/api/email/scan/resume",
            post(move || resume_scan(s_resume.clone())),
        )
        .route(
            "/app/sources/api/email/scan/cancel",
            post(move || cancel_scan(s_cancel.clone())),
        )
        .route(
            "/app/sources/api/email/choice",
            post(move |body| save_choice_route(s_choice.clone(), body)),
        )
        .route(
            "/app/sources/api/email/sync",
            post(move || trigger_sync(s_sync.clone())),
        )
        .route(
            "/app/sources/api/email/password",
            put(move |body| replace_password(s_password.clone(), body)),
        )
        .route(
            "/app/sources/api/email/runs/{id}/delete",
            get(move |path| run_delete_preview(s_run_preview.clone(), path)),
        )
        .route(
            "/app/sources/api/email/runs/{id}",
            delete(move |path, headers, body| {
                run_delete_execute(s_run_delete.clone(), path, headers, body)
            }),
        )
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({
            "error": message,
            "code": code,
        })),
    )
        .into_response()
}

fn map_email_error(err: &EmailError) -> Response {
    let status = match err {
        EmailError::AuthFailed
        | EmailError::AllMailHidden
        | EmailError::ImapDisabled
        | EmailError::InvalidInput(_)
        | EmailError::NotConnected => StatusCode::BAD_REQUEST,
        EmailError::Busy => StatusCode::CONFLICT,
        EmailError::Unreachable(_) | EmailError::Protocol(_) => StatusCode::SERVICE_UNAVAILABLE,
        EmailError::Cancelled => StatusCode::OK,
        EmailError::ReadOnlyGuard(_) | EmailError::Sqlite(_) | EmailError::Storage(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    error_response(status, err.code(), &err.to_string())
}

fn publish_email_batch(root: &Path, run_id: &str, created: &[CreatedEmailSegment]) {
    if created.is_empty() {
        return;
    }
    let segments: Vec<CreatedSegment> = created
        .iter()
        .map(|seg| CreatedSegment {
            day: seg.day.clone(),
            segment: seg.segment.clone(),
            stream: EMAIL_STREAM.to_owned(),
            hints: StreamHints {
                kind: Some(SegmentKind::Imported(ImportSource::Named("email".into()))),
                host: None,
                platform: None,
            },
        })
        .collect();
    let files: Vec<PathBuf> = created.iter().map(|seg| seg.file.clone()).collect();
    match publish(PublicationInput {
        journal: root,
        import_dir: None,
        import_id: run_id,
        importer: "email",
        revision: None,
        segments: &segments,
        files_created: &files,
        index_only: &[],
        may_write_record: None,
    }) {
        Ok(record) if record.status == PublicationStatus::Success => {}
        Ok(_) => {
            log::warn!("email batch publication reported partial or failed status for {run_id}");
        }
        Err(err) => {
            log::warn!("email batch publication failed for {run_id}: {err}");
        }
    }
}

fn update_email_schedule(root: &Path, install: bool) -> Result<(), EmailError> {
    solstone_core_system::schedule::mutate_schedule_entries(
        &root.join("config/schedules.json"),
        |entries| {
            if install {
                entries.insert(
                    "sync:email".to_owned(),
                    json!({
                        "cmd": [
                            "solstone",
                            "journal",
                            "importer",
                            "--sync",
                            "email",
                            "--save",
                            "--scheduled"
                        ],
                        "every": "hourly"
                    }),
                );
            } else {
                entries.remove("sync:email");
            }
            solstone_core_system::schedule::ScheduleMutation {
                changed: true,
                value: (),
            }
        },
    )
    .map_err(|e| EmailError::Storage(e.to_string()))?;
    Ok(())
}

fn snapshot_value(svc: &EmailService) -> Result<Value, EmailError> {
    let zone = store::journal_timezone(&svc.root);
    let mut snap = sync::status_snapshot(&svc.root, &zone)?;
    if svc.is_busy() {
        snap.syncing = true;
    }
    serde_json::to_value(snap).map_err(|e| EmailError::Storage(e.to_string()))
}

fn spawn_background_scan_with_source(
    svc: &EmailService,
    mut source: Box<dyn ImapSource + Send>,
) -> bool {
    if svc.busy.swap(true, Ordering::SeqCst) {
        let _ = source.logout();
        return false;
    }
    let guard = BusyGuard(Arc::clone(&svc.busy));
    let root = svc.root.clone();
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let zone = store::journal_timezone(&root);
        let now = Timestamp::now();
        if let Err(err) = sync::run_header_scan(&root, source.as_mut(), now, &zone, false)
            && !matches!(err, EmailError::Cancelled)
        {
            log::warn!("email header scan stopped: {err}");
        }
        let _ = source.logout();
    });
    true
}

fn spawn_background_scan(svc: &EmailService) -> Result<bool, EmailError> {
    let Some(creds) = store::read_credentials(&svc.root)? else {
        return Err(EmailError::NotConnected);
    };
    if svc.busy.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }
    let guard = BusyGuard(Arc::clone(&svc.busy));
    let root = svc.root.clone();
    let opener = Arc::clone(&svc.opener);
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let zone = store::journal_timezone(&root);
        let now = Timestamp::now();
        match (opener)(&creds.account, &creds.app_password) {
            Ok(mut source) => {
                if let Err(err) = sync::run_header_scan(&root, source.as_mut(), now, &zone, false)
                    && !matches!(err, EmailError::Cancelled)
                {
                    log::warn!("email header scan stopped: {err}");
                }
                let _ = source.logout();
            }
            Err(err) => {
                if let Some(reason) = err.attention_reason() {
                    let _ = sync::record_attention(&root, reason);
                }
                log::warn!("email header scan connect failed: {err}");
            }
        }
    });
    Ok(true)
}

fn spawn_background_sync_with_optional_source(
    svc: &EmailService,
    preopened: Option<Box<dyn ImapSource + Send>>,
) -> Result<bool, EmailError> {
    let Some(creds) = store::read_credentials(&svc.root)? else {
        if let Some(mut s) = preopened {
            let _ = s.logout();
        }
        return Err(EmailError::NotConnected);
    };
    if svc.busy.swap(true, Ordering::SeqCst) {
        if let Some(mut s) = preopened {
            let _ = s.logout();
        }
        return Ok(false);
    }
    let guard = BusyGuard(Arc::clone(&svc.busy));
    let root = svc.root.clone();
    let opener = Arc::clone(&svc.opener);
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let zone = store::journal_timezone(&root);
        let now = Timestamp::now();
        let opened = match preopened {
            Some(source) => Ok(source),
            None => (opener)(&creds.account, &creds.app_password),
        };
        match opened {
            Ok(mut source) => {
                let publisher = |run_id: &str, batch: &[CreatedEmailSegment]| {
                    publish_email_batch(&root, run_id, batch);
                };
                if let Err(err) = sync::run_sync_with_publisher(
                    &root,
                    source.as_mut(),
                    now,
                    &zone,
                    SyncOptions::default(),
                    Some(&publisher),
                ) && !matches!(err, EmailError::Cancelled)
                {
                    log::warn!("email sync stopped: {err}");
                }
                let _ = source.logout();
            }
            Err(err) => {
                if let Some(reason) = err.attention_reason() {
                    let _ = sync::record_attention(&root, reason);
                }
                log::warn!("email sync connect failed: {err}");
            }
        }
    });
    Ok(true)
}

async fn get_status(svc: EmailService) -> Response {
    let svc_clone = svc.clone();
    match tokio::task::spawn_blocking(move || snapshot_value(&svc_clone)).await {
        Ok(Ok(val)) => json_response(val),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to read email status",
        ),
    }
}

async fn get_choice_preview(
    svc: EmailService,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let range_raw = params
        .get("range")
        .map(String::as_str)
        .unwrap_or("30d")
        .trim();
    let Some(range) = RangePreset::parse(range_raw) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "range must be one of 30d, 6m, 1y, all, none",
        );
    };

    let types = match params.get("types") {
        None => vec![
            MessageType::Replies,
            MessageType::NewMessages,
            MessageType::Forwards,
        ],
        Some(raw) if raw.trim().is_empty() => Vec::new(),
        Some(raw) => {
            let mut parsed = Vec::new();
            for token in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                let Some(mt) = MessageType::parse(token) else {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid_input",
                        "types must contain only replies, new_messages, forwards",
                    );
                };
                if !parsed.contains(&mt) {
                    parsed.push(mt);
                }
            }
            parsed
        }
    };

    let root = svc.root.clone();
    match tokio::task::spawn_blocking(move || -> Result<Value, EmailError> {
        let zone = store::journal_timezone(&root);
        let now = Timestamp::now();
        let preview = sync::choice_preview(&root, range, &types, now, &zone)?;
        let existing_choice = store::read_choice(&root)?;
        let covered_ranges = existing_choice
            .as_ref()
            .map(|c| sync::covered_ranges_for(c.range))
            .unwrap_or_default();
        let snap = sync::status_snapshot(&root, &zone)?;
        let mut val =
            serde_json::to_value(preview).map_err(|e| EmailError::Storage(e.to_string()))?;
        if let Some(obj) = val.as_object_mut() {
            obj.insert(
                "covered_ranges".to_owned(),
                serde_json::to_value(covered_ranges).unwrap_or(Value::Array(Vec::new())),
            );
            if let Some(month) = snap.scan.earliest_month {
                obj.insert("earliest_month".to_owned(), Value::String(month));
            }
        }
        Ok(val)
    })
    .await
    {
        Ok(Ok(val)) => json_response(val),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to compute choice preview",
        ),
    }
}

async fn connect_email(svc: EmailService, body: Bytes) -> Response {
    let JsonBody::Value(value) = json_body(body) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "expected a JSON body",
        );
    };
    if let Some(provider) = value.get("provider").and_then(Value::as_str)
        && !provider.trim().is_empty()
        && !provider.trim().eq_ignore_ascii_case(PROVIDER_GMAIL)
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "unsupported_provider",
            "only Gmail is supported right now",
        );
    }
    let email = value
        .get("email")
        .or_else(|| value.get("account"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let app_password = value
        .get("app_password")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    let norm_email = match store::normalize_email_address(&email) {
        Ok(v) => v,
        Err(err) => return map_email_error(&err),
    };
    let norm_password = match store::normalize_app_password(&app_password) {
        Ok(v) => v,
        Err(err) => return map_email_error(&err),
    };

    if svc.is_busy() {
        return map_email_error(&EmailError::Busy);
    }

    let svc_bg = svc.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value, EmailError> {
        let mut source = (svc_bg.opener)(&norm_email, &norm_password)?;
        if let Err(err) = sync::connect_and_verify(
            &svc_bg.root,
            &norm_email,
            &norm_password,
            source.as_mut(),
            Timestamp::now(),
        ) {
            let _ = source.logout();
            return Err(err);
        }
        let started = spawn_background_scan_with_source(&svc_bg, source);
        let status = snapshot_value(&svc_bg)?;
        Ok(json!({
            "success": true,
            "scan_started": started,
            "status": status,
        }))
    })
    .await;

    match result {
        Ok(Ok(val)) => json_response(val),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to connect email",
        ),
    }
}

async fn resume_scan(svc: EmailService) -> Response {
    let svc_bg = svc.clone();
    match tokio::task::spawn_blocking(move || -> Result<Value, EmailError> {
        store::clear_cancel(&svc_bg.root);
        let started = spawn_background_scan(&svc_bg)?;
        let status = snapshot_value(&svc_bg)?;
        Ok(json!({
            "success": true,
            "started": started,
            "status": status,
        }))
    })
    .await
    {
        Ok(Ok(val)) => json_response(val),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to resume header scan",
        ),
    }
}

async fn cancel_scan(svc: EmailService) -> Response {
    let svc_bg = svc.clone();
    match tokio::task::spawn_blocking(move || -> Result<Value, EmailError> {
        store::request_cancel(&svc_bg.root)?;
        let status = snapshot_value(&svc_bg)?;
        Ok(json!({
            "success": true,
            "status": status,
        }))
    })
    .await
    {
        Ok(Ok(val)) => json_response(val),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to cancel header scan",
        ),
    }
}

async fn save_choice_route(svc: EmailService, body: Bytes) -> Response {
    let JsonBody::Value(value) = json_body(body) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "expected a JSON body",
        );
    };
    let range_str = value.get("range").and_then(Value::as_str).unwrap_or("");
    let Some(range) = RangePreset::parse(range_str.trim()) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "range must be one of 30d, 6m, 1y, all, none",
        );
    };
    let Some(types_arr) = value.get("types").and_then(Value::as_array) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "choose at least one type of message.",
        );
    };
    let mut types = Vec::new();
    for item in types_arr {
        let Some(token) = item.as_str() else {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "types must contain only replies, new_messages, forwards",
            );
        };
        let Some(mt) = MessageType::parse(token.trim()) else {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "types must contain only replies, new_messages, forwards",
            );
        };
        if !types.contains(&mt) {
            types.push(mt);
        }
    }
    if types.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "choose at least one type of message.",
        );
    }

    let svc_bg = svc.clone();
    match tokio::task::spawn_blocking(move || -> Result<Value, EmailError> {
        let zone = store::journal_timezone(&svc_bg.root);
        let now = Timestamp::now();
        let choice = sync::save_choice(&svc_bg.root, range, types, now, &zone)?;
        update_email_schedule(&svc_bg.root, true)?;
        let started = spawn_background_sync_with_optional_source(&svc_bg, None)?;
        let status = snapshot_value(&svc_bg)?;
        Ok(json!({
            "success": true,
            "sync_started": started,
            "choice": choice,
            "status": status,
        }))
    })
    .await
    {
        Ok(Ok(val)) => json_response(val),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to save email choice",
        ),
    }
}

async fn trigger_sync(svc: EmailService) -> Response {
    let svc_bg = svc.clone();
    match tokio::task::spawn_blocking(move || -> Result<Value, EmailError> {
        let started = spawn_background_sync_with_optional_source(&svc_bg, None)?;
        let status = snapshot_value(&svc_bg)?;
        Ok(json!({
            "success": true,
            "started": started,
            "status": status,
        }))
    })
    .await
    {
        Ok(Ok(val)) => json_response(val),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to trigger email sync",
        ),
    }
}

async fn replace_password(svc: EmailService, body: Bytes) -> Response {
    let JsonBody::Value(value) = json_body(body) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "expected a JSON body",
        );
    };
    let app_password = value
        .get("app_password")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let norm_password = match store::normalize_app_password(&app_password) {
        Ok(v) => v,
        Err(err) => return map_email_error(&err),
    };

    if svc.is_busy() {
        return map_email_error(&EmailError::Busy);
    }

    let svc_bg = svc.clone();
    match tokio::task::spawn_blocking(move || -> Result<Value, EmailError> {
        let account = store::read_account(&svc_bg.root)?
            .map(|a| a.account)
            .or_else(|| {
                store::read_credentials(&svc_bg.root)
                    .ok()
                    .flatten()
                    .map(|c| c.account)
            })
            .ok_or(EmailError::NotConnected)?;
        let mut source = (svc_bg.opener)(&account, &norm_password)?;
        if let Err(err) = sync::update_password(&svc_bg.root, &norm_password, source.as_mut()) {
            let _ = source.logout();
            return Err(err);
        }
        let has_choice = store::read_choice(&svc_bg.root)?.is_some();
        if has_choice {
            update_email_schedule(&svc_bg.root, true)?;
            let _ = spawn_background_sync_with_optional_source(&svc_bg, Some(source))?;
        } else {
            let _ = spawn_background_scan_with_source(&svc_bg, source);
        }
        let status = snapshot_value(&svc_bg)?;
        Ok(json!({
            "success": true,
            "status": status,
        }))
    })
    .await
    {
        Ok(Ok(val)) => json_response(val),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to update email password",
        ),
    }
}

async fn disconnect_email(svc: EmailService) -> Response {
    let svc_bg = svc.clone();
    match tokio::task::spawn_blocking(move || -> Result<Value, EmailError> {
        let _ = store::request_cancel(&svc_bg.root);
        update_email_schedule(&svc_bg.root, false)?;
        sync::disconnect(&svc_bg.root, Timestamp::now())?;
        let status = snapshot_value(&svc_bg)?;
        Ok(json!({
            "success": true,
            "status": status,
        }))
    })
    .await
    {
        Ok(Ok(val)) => json_response(val),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to disconnect email",
        ),
    }
}

// ---------------------------------------------------------------------------
// Per-run recoverable Trash (/app/sources/api/email/runs/{id}*)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct DeleteRunBody {
    #[serde(default)]
    erase: bool,
    #[serde(default)]
    backups: bool,
}

async fn run_delete_preview(svc: EmailService, AxumPath(id): AxumPath<String>) -> Response {
    if !is_valid_run_id(&id) {
        return error_response(StatusCode::NOT_FOUND, "not_found", "sync run not found");
    }
    let root = svc.root.clone();
    match tokio::task::spawn_blocking(move || -> Result<Option<Value>, EmailError> {
        let Some(run) = find_run(&root, &id)? else {
            return Ok(None);
        };
        let live_segs = run_live_segments(&root, &run);
        if live_segs.is_empty() {
            return Ok(None);
        }
        let live_msgs = run_live_message_count(&root, &id)?;
        let days = run_day_bounds(&live_segs);
        let erase_after_days = solstone_core_trash::erase_after_days(&root);
        Ok(Some(json!({
            "id": run.id,
            "synced_at": run.synced_at,
            "range_label": run.range_label,
            "messages": live_msgs,
            "segments": live_segs.len(),
            "days": days.map(|(first, last)| json!([first, last])),
            "erase_after_days": erase_after_days,
        })))
    })
    .await
    {
        Ok(Ok(Some(val))) => json_response(val),
        Ok(Ok(None)) => error_response(StatusCode::NOT_FOUND, "not_found", "sync run not found"),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to read sync run",
        ),
    }
}

async fn run_delete_execute(
    svc: EmailService,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_valid_run_id(&id) {
        return error_response(StatusCode::NOT_FOUND, "not_found", "sync run not found");
    }
    let request: DeleteRunBody = if body.iter().all(u8::is_ascii_whitespace) {
        DeleteRunBody::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(parsed) => parsed,
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "invalid delete body",
                );
            }
        }
    };
    let surface = Surface::of_request(
        Page::Settings,
        headers.get(USER_AGENT).and_then(|v| v.to_str().ok()),
    );
    let root = svc.root.clone();
    match tokio::task::spawn_blocking(move || -> Result<Option<Value>, EmailError> {
        let Some(run) = find_run(&root, &id)? else {
            return Ok(None);
        };
        let live_segs = run_live_segments(&root, &run);
        if live_segs.is_empty() {
            return Ok(None);
        }
        let live_msgs_before = run_live_message_count(&root, &id)?;
        let mut chosen = Vec::with_capacity(live_segs.len());
        for seg_rel in &live_segs {
            let parts: Vec<&str> = seg_rel.split('/').collect();
            if parts.len() != 3 || parts[1] != EMAIL_STREAM {
                continue;
            }
            chosen.push(solstone_core_trash::Chosen {
                unit: Unit::Segment {
                    day: parts[0].to_owned(),
                    stream: parts[1].to_owned(),
                    segment: parts[2].to_owned(),
                },
                paths: vec![format!("chronicle/{seg_rel}")],
                label: None,
            });
        }
        if chosen.is_empty() {
            return Ok(None);
        }
        let results = solstone_core_import::trash_kinds::with_kinds(&root, |kinds| {
            solstone_core_trash::detach_set_with(
                &root,
                kinds,
                chosen,
                Action::SegmentDelete,
                surface,
                request.erase,
                request.backups,
            )
        })
        .map_err(|e| EmailError::Storage(e.to_string()))?;

        let mut moved_segments = 0usize;
        let mut failed_segments = 0usize;
        let mut trash_ids = Vec::new();
        let mut erase_at: Option<String> = None;
        let mut all_erased = request.erase;
        for res in results {
            match res {
                solstone_core_trash::SetResult::Moved { item, erased } => {
                    moved_segments += 1;
                    if erase_at.is_none() && !item.erase_at.is_empty() {
                        erase_at = Some(item.erase_at.clone());
                    }
                    all_erased &= erased;
                    trash_ids.push(item.id);
                }
                solstone_core_trash::SetResult::Refused(_) => {
                    failed_segments += 1;
                    all_erased = false;
                }
            }
        }
        let _ = solstone_core_import::made_from::MadeFromIt::new().refresh_search(&root);
        let live_msgs_after = run_live_message_count(&root, &id)?;
        let moved_messages = live_msgs_before.saturating_sub(live_msgs_after);

        Ok(Some(json!({
            "success": true,
            "id": id,
            "moved_segments": moved_segments,
            "moved_messages": moved_messages,
            "failed_segments": failed_segments,
            "trash_ids": trash_ids,
            "erase_at": erase_at,
            "erased": all_erased,
        })))
    })
    .await
    {
        Ok(Ok(Some(val))) => json_response(val),
        Ok(Ok(None)) => error_response(StatusCode::NOT_FOUND, "not_found", "sync run not found"),
        Ok(Err(err)) => map_email_error(&err),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "failed to move sync run to trash",
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use solstone_core_email::error::{EmailError, Result as EmailResult};
    use solstone_core_email::imap::{ImapSource, MailboxInfo, WireAudit};
    use solstone_core_email::model::{Addr, Header, Location, Part};
    use tower::ServiceExt;

    use super::*;

    #[derive(Clone)]
    struct MockMessage {
        header: Header,
        plain: String,
    }

    #[derive(Clone)]
    struct MockMailboxState {
        expected_password: String,
        all_mail_hidden: bool,
        messages: Vec<MockMessage>,
        bodies_fetched: usize,
    }

    struct MockImapSource {
        state: Arc<Mutex<MockMailboxState>>,
    }

    impl ImapSource for MockImapSource {
        fn mailbox_info(&mut self) -> EmailResult<MailboxInfo> {
            let st = self.state.lock().expect("lock");
            if st.all_mail_hidden {
                return Err(EmailError::AllMailHidden);
            }
            let max_uid = st
                .messages
                .iter()
                .map(|m| m.header.location.uid)
                .max()
                .unwrap_or(0);
            Ok(MailboxInfo {
                name: "[Gmail]/All Mail".to_owned(),
                uid_validity: 7001,
                uid_next: max_uid + 1,
                exists: st.messages.len() as u32,
            })
        }

        fn scan_page(
            &mut self,
            _mb: &MailboxInfo,
            lo_uid: u32,
            hi_uid: u32,
        ) -> EmailResult<Vec<Header>> {
            let st = self.state.lock().expect("lock");
            if st.all_mail_hidden {
                return Err(EmailError::AllMailHidden);
            }
            Ok(st
                .messages
                .iter()
                .filter(|m| m.header.location.uid >= lo_uid && m.header.location.uid <= hi_uid)
                .map(|m| m.header.clone())
                .collect())
        }

        fn fetch_texts(
            &mut self,
            requests: &[(&Header, &Part)],
        ) -> EmailResult<Vec<Option<String>>> {
            let mut st = self.state.lock().expect("lock");
            if st.all_mail_hidden {
                return Err(EmailError::AllMailHidden);
            }
            st.bodies_fetched += requests.len();
            let by_uid: HashMap<u32, String> = st
                .messages
                .iter()
                .map(|m| (m.header.location.uid, m.plain.clone()))
                .collect();
            Ok(requests
                .iter()
                .map(|(h, _)| by_uid.get(&h.location.uid).cloned())
                .collect())
        }

        fn wire_audit(&self) -> WireAudit {
            WireAudit::default()
        }

        fn logout(&mut self) -> EmailResult<()> {
            Ok(())
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_header(
        uid: u32,
        gm_msgid: &str,
        from: &str,
        to: &str,
        subject: &str,
        sent_ms: i64,
        labels: &[&str],
        in_reply_to: Option<&str>,
    ) -> Header {
        Header {
            key: format!("gmail:{gm_msgid}"),
            location: Location {
                mailbox: "[Gmail]/All Mail".to_owned(),
                uid_validity: 7001,
                uid,
            },
            gm_msgid: Some(gm_msgid.to_owned()),
            gm_thrid: Some("t1".to_owned()),
            message_id: Some(format!("<{gm_msgid}@example.test>")),
            in_reply_to: in_reply_to.into_iter().map(str::to_owned).collect(),
            references: Vec::new(),
            subject: Some(subject.to_owned()),
            from: vec![Addr {
                name: None,
                address: Some(from.to_owned()),
            }],
            to: vec![Addr {
                name: None,
                address: Some(to.to_owned()),
            }],
            cc: Vec::new(),
            date_ms: Some(sent_ms),
            internal_date_ms: Some(sent_ms),
            flags: vec!["\\Seen".to_owned()],
            labels: labels.iter().map(|s| (*s).to_owned()).collect(),
            list_id: None,
            auto_submitted: None,
            tree: Part {
                path: vec![1],
                mime: "text/plain".to_owned(),
                size: 64,
                charset: Some("utf-8".to_owned()),
                encoding: Some("7bit".to_owned()),
                ..Default::default()
            },
            size: 128,
        }
    }

    async fn read_json(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let val = serde_json::from_slice(&bytes).expect("valid JSON");
        (status, val)
    }

    async fn wait_until_idle(svc: &EmailService) {
        for _ in 0..200 {
            if !svc.is_busy() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        panic!("email background task did not settle in time");
    }

    #[tokio::test]
    async fn full_http_lifecycle_connect_scan_choose_sync_trash_recover_rotate_disconnect() {
        let root = crate::test_support::established_root();
        let now_ms = Timestamp::now().as_millisecond();
        let recent_ms = now_ms - 3 * 86_400 * 1_000;
        let older_ms = now_ms - 120 * 86_400 * 1_000;

        let shared = Arc::new(Mutex::new(MockMailboxState {
            expected_password: "abcd efgh ijkl mnop".to_owned(),
            all_mail_hidden: false,
            bodies_fetched: 0,
            messages: vec![
                MockMessage {
                    header: make_header(
                        1,
                        "m-parent",
                        "sam@example.test",
                        "maya@example.test",
                        "Trailhead time",
                        recent_ms - 600_000,
                        &["\\Inbox"],
                        None,
                    ),
                    plain: "Meet at 7am by the south lot?".to_owned(),
                },
                MockMessage {
                    header: make_header(
                        2,
                        "m-reply",
                        "maya@example.test",
                        "sam@example.test",
                        "Re: Trailhead time",
                        recent_ms,
                        &["\\Sent"],
                        Some("<m-parent@example.test>"),
                    ),
                    plain: "Sounds good, see you at 7am.\n\nOn Mon Sam wrote:\n> Meet at 7am by the south lot?".to_owned(),
                },
                MockMessage {
                    header: make_header(
                        3,
                        "m-old-new",
                        "maya@example.test",
                        "pat@example.test",
                        "Summer cabin dates",
                        older_ms,
                        &["\\Sent"],
                        None,
                    ),
                    plain: "Let's hold the third weekend of July.".to_owned(),
                },
                MockMessage {
                    header: make_header(
                        4,
                        "m-draft",
                        "maya@example.test",
                        "pat@example.test",
                        "Half written",
                        recent_ms - 10_000,
                        &["\\Sent", "\\Draft"],
                        None,
                    ),
                    plain: "draft body never read".to_owned(),
                },
            ],
        }));

        let opener_state = Arc::clone(&shared);
        let opener: ImapOpener = Arc::new(move |_acct, pw| {
            let st = opener_state.lock().expect("lock");
            let clean_expected: String = st
                .expected_password
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            let clean_got: String = pw.chars().filter(|c| !c.is_whitespace()).collect();
            if clean_got != clean_expected {
                return Err(EmailError::AuthFailed);
            }
            if st.all_mail_hidden {
                return Err(EmailError::AllMailHidden);
            }
            Ok(Box::new(MockImapSource {
                state: Arc::clone(&opener_state),
            }))
        });

        let svc = EmailService::with_opener(root.path().to_path_buf(), opener);
        let router = crate::routes_with_email_service(
            root.path().to_path_buf(),
            solstone_core_journal_io::LockOptions::default(),
            svc.clone(),
        );

        // 1. Fresh state is ready_to_set_up.
        let (st, body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["source"], "email");
        assert_eq!(body["state"], "ready_to_set_up");
        assert_eq!(body["connected"], false);

        // 2. Wrong password fails with typed auth_failed and saves nothing.
        let (st, body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/sources/api/email/connect")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({
                                "provider": "gmail",
                                "email": "maya@example.test",
                                "app_password": "zzzz zzzz zzzz zzzz"
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "auth_failed");
        assert!(store::read_credentials(root.path()).unwrap().is_none());

        // 3. Connect succeeds and completes header-only scan with 0 bodies fetched.
        let (st, body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/sources/api/email/connect")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({
                                "provider": "gmail",
                                "email": "maya@example.test",
                                "app_password": "abcd efgh ijkl mnop"
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], true);
        wait_until_idle(&svc).await;
        assert_eq!(shared.lock().unwrap().bodies_fetched, 0);

        // Credentials must remain redacted from /app/settings/api/config.
        let (cfg_st, cfg_body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/settings/api/config")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(cfg_st, StatusCode::OK);
        assert!(cfg_body.get("email").is_none());
        assert!(!cfg_body.to_string().contains("abcdefghijklmnop"));

        // 4. Status is setting_up / awaiting_choice, and choice-preview reads 0 bodies.
        let (_, status_after_scan) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status_after_scan["state"], "setting_up");
        assert_eq!(status_after_scan["phase"], "awaiting_choice");
        assert_eq!(status_after_scan["scan"]["bodies_read"], 0);
        assert_eq!(status_after_scan["accounting"]["sent_total"], 3);
        assert_eq!(status_after_scan["accounting"]["left_out"]["total"], 1);
        assert_eq!(status_after_scan["accounting"]["left_out"]["drafts"], 1);

        let (prev_st, prev_body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email/choice-preview?range=30d&types=replies,new_messages,forwards")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(prev_st, StatusCode::OK);
        assert_eq!(prev_body["selected_messages"], 1);
        assert_eq!(prev_body["ranges"]["30d"], 1);
        assert_eq!(prev_body["ranges"]["6m"], 2);
        assert_eq!(shared.lock().unwrap().bodies_fetched, 0);

        // 5. Empty types is refused cleanly.
        let (bad_st, bad_body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/sources/api/email/choice")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "range": "30d", "types": [] }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(bad_st, StatusCode::BAD_REQUEST);
        assert_eq!(bad_body["code"], "invalid_input");

        // 6. Save choice (30d) installs hourly sync:email schedule and lands 1 exchange.
        let (ch_st, ch_body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/sources/api/email/choice")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "range": "30d", "types": ["replies", "new_messages", "forwards"] }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(ch_st, StatusCode::OK);
        assert_eq!(ch_body["success"], true);
        wait_until_idle(&svc).await;

        let schedules: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(schedules["sync:email"]["every"], "hourly");
        assert_eq!(
            schedules["sync:email"]["cmd"],
            json!([
                "solstone",
                "journal",
                "importer",
                "--sync",
                "email",
                "--save",
                "--scheduled"
            ])
        );

        let (_, steady) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(steady["state"], "on");
        assert_eq!(steady["accounting"]["in_journal"], 1);
        assert_eq!(steady["accounting"]["outside_choice"], 1);
        assert_eq!(steady["accounting"]["left_out"]["total"], 1);
        assert_eq!(steady["choice"]["covered_ranges"], json!(["30d"]));
        let run_id = steady["runs"][0]["id"].as_str().expect("run id").to_owned();
        assert!(root.path().join("streams/import.email.json").is_file());

        // 7. Per-run Trash preview + execute moves segment to Trash, then recover restores it.
        let (rp_st, rp_body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get(format!("/app/sources/api/email/runs/{run_id}/delete"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(rp_st, StatusCode::OK);
        assert_eq!(rp_body["messages"], 1);
        assert_eq!(rp_body["segments"], 1);

        let (rd_st, rd_body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::delete(format!("/app/sources/api/email/runs/{run_id}"))
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "erase": false, "backups": false }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(rd_st, StatusCode::OK);
        assert_eq!(rd_body["moved_segments"], 1);
        assert_eq!(rd_body["moved_messages"], 1);
        let trash_ids = rd_body["trash_ids"].as_array().expect("trash ids").clone();
        assert_eq!(trash_ids.len(), 1);

        // After Trash, accounting shows 0 in_journal and 1 removed_by_owner, and re-sync does NOT re-add it.
        let (_, after_trash) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(after_trash["accounting"]["in_journal"], 0);
        assert_eq!(after_trash["accounting"]["removed_by_owner"], 1);

        // Recover via standard Trash endpoint brings the exchange back.
        let (rec_st, _) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/settings/api/trash/recover")
                        .header("content-type", "application/json")
                        .body(Body::from(json!({ "ids": trash_ids }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(rec_st, StatusCode::OK);
        let (_, after_recover) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(after_recover["accounting"]["in_journal"], 1);
        assert_eq!(after_recover["accounting"]["removed_by_owner"], 0);

        // 8. Password revocation sets needs_attention; PUT /password clears it and resumes.
        shared.lock().unwrap().expected_password = "qwer tyui opas dfgh".to_owned();
        let (sync_st, _) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/sources/api/email/sync")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(sync_st, StatusCode::OK);
        wait_until_idle(&svc).await;
        assert!(needs_attention(root.path()));

        let (_, attn) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(attn["state"], "needs_attention");
        assert_eq!(attn["attention_reason"], "password_rejected");

        let (pw_st, pw_body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::put("/app/sources/api/email/password")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "app_password": "qwer tyui opas dfgh" }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(pw_st, StatusCode::OK);
        assert_eq!(pw_body["success"], true);
        wait_until_idle(&svc).await;
        assert!(!needs_attention(root.path()));

        // 9. Disconnect removes credentials + schedule while keeping landed exchange.
        let (dc_st, dc_body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::delete("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(dc_st, StatusCode::OK);
        assert_eq!(dc_body["status"]["state"], "off");
        assert_eq!(dc_body["status"]["accounting"]["in_journal"], 1);
        assert!(store::read_credentials(root.path()).unwrap().is_none());
        let schedules_after: Value = serde_json::from_slice(
            &std::fs::read(root.path().join("config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert!(schedules_after.get("sync:email").is_none());
    }

    #[tokio::test]
    async fn all_mail_hidden_recovers_via_sync_and_widen_untick_keeps_existing() {
        let root = crate::test_support::established_root();
        let now_ms = Timestamp::now().as_millisecond();
        let recent_ms = now_ms - 2 * 86_400 * 1_000;
        let older_ms = now_ms - 120 * 86_400 * 1_000;

        let shared = Arc::new(Mutex::new(MockMailboxState {
            expected_password: "abcd efgh ijkl mnop".to_owned(),
            all_mail_hidden: true,
            bodies_fetched: 0,
            messages: vec![
                MockMessage {
                    header: make_header(
                        1,
                        "m-recent-new",
                        "maya@example.test",
                        "sam@example.test",
                        "Recent update",
                        recent_ms,
                        &["\\Sent"],
                        None,
                    ),
                    plain: "Quick check-in from today.".to_owned(),
                },
                MockMessage {
                    header: make_header(
                        2,
                        "m-older-fwd",
                        "maya@example.test",
                        "pat@example.test",
                        "Fwd: Flight itinerary",
                        older_ms,
                        &["\\Sent"],
                        None,
                    ),
                    plain: "Sharing our booking.\n\n---------- Forwarded message ---------\nFrom: Airline\nFlight 42 departs at 9am.".to_owned(),
                },
            ],
        }));

        let opener_state = Arc::clone(&shared);
        let opener: ImapOpener = Arc::new(move |_acct, _pw| {
            let st = opener_state.lock().expect("lock");
            if st.all_mail_hidden {
                return Err(EmailError::AllMailHidden);
            }
            Ok(Box::new(MockImapSource {
                state: Arc::clone(&opener_state),
            }))
        });

        let svc = EmailService::with_opener(root.path().to_path_buf(), opener);
        let router = crate::routes_with_email_service(
            root.path().to_path_buf(),
            solstone_core_journal_io::LockOptions::default(),
            svc.clone(),
        );

        // Connect fails with all_mail_hidden when All Mail is hidden.
        let (st, body) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/sources/api/email/connect")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({
                                "email": "maya@example.test",
                                "app_password": "abcd efgh ijkl mnop"
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "all_mail_hidden");

        // Enable All Mail -> connect -> scan -> choose 30d (lands 1).
        shared.lock().unwrap().all_mail_hidden = false;
        let (st, _) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/sources/api/email/connect")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({
                                "email": "maya@example.test",
                                "app_password": "abcd efgh ijkl mnop"
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        wait_until_idle(&svc).await;

        let (st, _) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/sources/api/email/choice")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "range": "30d", "types": ["replies", "new_messages", "forwards"] }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        wait_until_idle(&svc).await;

        // Hide All Mail later -> manual sync sets needs_attention(all_mail_hidden).
        shared.lock().unwrap().all_mail_hidden = true;
        let _ = router
            .clone()
            .oneshot(
                Request::post("/app/sources/api/email/sync")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        wait_until_idle(&svc).await;
        let (_, hidden_st) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(hidden_st["state"], "needs_attention");
        assert_eq!(hidden_st["attention_reason"], "all_mail_hidden");

        // Re-enable All Mail -> POST /sync ("try again") clears needs_attention without password change.
        shared.lock().unwrap().all_mail_hidden = false;
        let _ = router
            .clone()
            .oneshot(
                Request::post("/app/sources/api/email/sync")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        wait_until_idle(&svc).await;
        let (_, recovered) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(recovered["state"], "on");
        assert!(recovered.get("attention_reason").is_none());

        // Widen to 6m while unticking new_messages: keeps already-landed recent new message AND adds older forward!
        let (st, _) = read_json(
            router
                .clone()
                .oneshot(
                    Request::post("/app/sources/api/email/choice")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({ "range": "6m", "types": ["replies", "forwards"] }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        wait_until_idle(&svc).await;
        let (_, widened) = read_json(
            router
                .clone()
                .oneshot(
                    Request::get("/app/sources/api/email")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(widened["accounting"]["in_journal"], 2);
        assert_eq!(widened["choice"]["covered_ranges"], json!(["30d", "6m"]));
    }
}
