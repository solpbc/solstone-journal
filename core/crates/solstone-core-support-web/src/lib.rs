// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read and page routes for the Support Convey surface.

use std::io::Write;
use std::path::{Path as FsPath, PathBuf};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{DefaultBodyLimit, FromRequest, Json, Multipart, Path, Query, Request},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{Local, Utc};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_support_drafts::{
    append_support_draft, load_draft_event, mark_draft_cancelled, mark_draft_submitted,
    record_draft_captured, resolve_draft_outcome,
};
use solstone_core_support_portal::{
    OperationError, PortalClient, PortalClientError, PortalOperationError, collect_all, is_enabled,
    native_platform, portal_url_from_settings,
};
use tempfile::Builder;
use uuid::Uuid;

const SHELL: &[u8] = include_bytes!("../../solstone-core-convey-shell/assets/static/shell.html");
const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const BACKGROUND: &[u8] = include_bytes!("../assets/background.html");
const SUPPORT_JS: &[u8] = include_bytes!("../assets/static/support.js");

/// Build the complete W2b Support route surface for one journal.
pub fn routes(journal_root: PathBuf) -> Router {
    let layer_root = journal_root.clone();
    let config_root = journal_root.clone();
    let tickets_root = journal_root.clone();
    let closed_root = journal_root.clone();
    let ticket_root = journal_root.clone();
    let articles_root = journal_root.clone();
    let article_root = journal_root.clone();
    let announcements_root = journal_root.clone();
    let diagnostics_root = journal_root.clone();
    let badge_root = journal_root;
    let draft_root = layer_root.clone();
    let register_root = layer_root.clone();
    let create_root = layer_root.clone();
    let reply_root = layer_root.clone();
    let attachment_root = layer_root.clone();
    let close_root = layer_root.clone();
    let confirm_root = layer_root.clone();
    let still_need_help_root = layer_root.clone();
    let feedback_root = layer_root.clone();
    let draft_confirm_root = layer_root.clone();
    let draft_cancel_root = layer_root.clone();
    Router::new()
        .route(
            "/app/support",
            get(|| async { Redirect::permanent("/app/support/") }),
        )
        .route("/app/support/", get(shell))
        .route("/app/support/workspace", get(workspace))
        .route("/app/support/background", get(background))
        .route("/app/support/static/support.js", get(support_js))
        .route(
            "/app/support/api/config",
            get(move || config(config_root.clone())),
        )
        .route(
            "/app/support/api/tickets",
            get(move |query| tickets(tickets_root.clone(), query))
                .post(move |headers, payload| create_ticket(create_root.clone(), headers, payload)),
        )
        .route(
            "/app/support/api/tickets/closed",
            get(move |query| closed(closed_root.clone(), query)),
        )
        .route(
            "/app/support/api/tickets/{id}",
            get(move |id| ticket(ticket_root.clone(), id)),
        )
        .route(
            "/app/support/api/articles",
            get(move |query| articles(articles_root.clone(), query)),
        )
        .route(
            "/app/support/api/articles/{slug}",
            get(move |slug| article(article_root.clone(), slug)),
        )
        .route(
            "/app/support/api/announcements",
            get(move || announcements(announcements_root.clone())),
        )
        .route(
            "/app/support/api/diagnostics",
            get(move || diagnostics(diagnostics_root.clone())),
        )
        .route(
            "/app/support/api/badge-count",
            get(move || badge_count(badge_root.clone())),
        )
        .route(
            "/app/support/api/draft",
            post(move |request| capture_draft(draft_root.clone(), request)),
        )
        .route(
            "/app/support/api/draft/confirm",
            post(move |payload| confirm_draft(draft_confirm_root.clone(), payload)),
        )
        .route(
            "/app/support/api/draft/cancel",
            post(move |payload| cancel_draft(draft_cancel_root.clone(), payload)),
        )
        .route(
            "/app/support/api/register",
            post(move || register(register_root.clone())),
        )
        .route(
            "/app/support/api/tickets/{id}/reply",
            post(move |id, headers, payload| {
                reply_to_ticket(reply_root.clone(), id, headers, payload)
            }),
        )
        .route(
            "/app/support/api/tickets/{id}/attachments",
            post(move |id, headers, multipart| {
                upload_attachment(attachment_root.clone(), id, headers, multipart)
            }),
        )
        .route(
            "/app/support/api/tickets/{id}/close",
            post(move |id, headers| close_ticket(close_root.clone(), id, headers)),
        )
        .route(
            "/app/support/api/tickets/{id}/resolution/confirm",
            post(move |id, headers| confirm_resolution(confirm_root.clone(), id, headers)),
        )
        .route(
            "/app/support/api/tickets/{id}/resolution/still-need-help",
            post(move |id, headers| still_need_help(still_need_help_root.clone(), id, headers)),
        )
        .route(
            "/app/support/api/feedback",
            post(move |headers, payload| submit_feedback(feedback_root.clone(), headers, payload)),
        )
        .layer(DefaultBodyLimit::max(
            solstone_core_convey_http::serve::STANDARD_BODY_LIMIT,
        ))
        .layer(middleware::from_fn(move |request, next| {
            drain_before_and_after(layer_root.clone(), request, next)
        }))
}

async fn shell() -> Response {
    bytes(SHELL, "text/html; charset=utf-8")
}
async fn workspace() -> Response {
    bytes(WORKSPACE, "text/html; charset=utf-8")
}
async fn background() -> Response {
    bytes(BACKGROUND, "text/html; charset=utf-8")
}
async fn support_js() -> Response {
    bytes(SUPPORT_JS, "text/javascript; charset=utf-8")
}

fn bytes(value: &'static [u8], content_type: &'static str) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(axum::body::Body::from(value))
        .expect("embedded support asset response")
}

async fn drain_before_and_after(
    root: PathBuf,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    drain(&root);
    let response = next.run(request).await;
    drain(&root);
    response
}

fn drain(root: &std::path::Path) {
    if let Ok(mut client) = build_client(root, false) {
        let _ = client.drain_pending_acknowledgements();
    }
}

fn build_client(
    root: &std::path::Path,
    anonymous: bool,
) -> Result<PortalClient, PortalClientError> {
    #[cfg(test)]
    if let Some(client) = try_test_client(root, anonymous) {
        return client;
    }
    PortalClient::from_journal_settings(root, None, anonymous)
}

#[cfg(test)]
type TestClientFactory = std::sync::Arc<
    dyn Fn(&std::path::Path, bool) -> Result<PortalClient, PortalClientError> + Send + Sync,
>;

#[cfg(test)]
thread_local! {
    static TEST_CLIENT_FACTORY: std::cell::RefCell<Option<TestClientFactory>> =
        const { std::cell::RefCell::new(None) };
}

/// Installs an OS-thread-local `PortalClient` factory for `--lib` tests.
///
/// This hook is thread-local. Corpus oneshot tests must stay on
/// `#[tokio::test]`'s default `current_thread` flavor. Do not add
/// `rt-multi-thread` or `flavor = "multi_thread"` without replacing this
/// with a task-local scope around each `oneshot()`.
#[cfg(test)]
pub(crate) fn install_test_client_factory(
    factory: impl Fn(&std::path::Path, bool) -> Result<PortalClient, PortalClientError>
    + Send
    + Sync
    + 'static,
) -> TestClientGuard {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        debug_assert_eq!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread,
            "install_test_client_factory is thread-local; oneshot must run on current_thread"
        );
    }
    let previous =
        TEST_CLIENT_FACTORY.with(|cell| cell.replace(Some(std::sync::Arc::new(factory))));
    TestClientGuard(previous)
}

#[cfg(test)]
pub(crate) struct TestClientGuard(Option<TestClientFactory>);

#[cfg(test)]
impl Drop for TestClientGuard {
    fn drop(&mut self) {
        TEST_CLIENT_FACTORY.with(|cell| {
            cell.replace(self.0.take());
        });
    }
}

#[cfg(test)]
fn try_test_client(
    root: &std::path::Path,
    anonymous: bool,
) -> Option<Result<PortalClient, PortalClientError>> {
    TEST_CLIENT_FACTORY.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|factory| factory(root, anonymous))
    })
}

async fn config(root: PathBuf) -> Response {
    json_response(
        json!({"enabled": is_enabled(&root), "portal_url": portal_url_from_settings(&root)}),
    )
}

async fn diagnostics(root: PathBuf) -> Response {
    json_response(Value::Object(collect_all(
        &root,
        Local::now(),
        native_platform(),
    )))
}

async fn tickets(root: PathBuf, Query(query): Query<TicketQuery>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let result = with_client(&root, |client| {
        client.list_tickets(
            query.status.as_deref(),
            query.product.as_deref(),
            query.severity.as_deref(),
        )
    });
    portal_response(result)
}

async fn closed(root: PathBuf, Query(query): Query<ClosedQuery>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| {
        client.list_closed_history(query.cursor.as_deref())
    }))
}

async fn ticket(root: PathBuf, Path(id): Path<String>) -> Response {
    let Ok(id) = id.parse::<i64>() else {
        return http_not_found();
    };
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| client.get_ticket(id)))
}

async fn articles(root: PathBuf, Query(query): Query<ArticleQuery>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| {
        client.search_articles(query.q.as_deref())
    }))
}

async fn article(root: PathBuf, Path(slug): Path<String>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| client.get_article(&slug)))
}

async fn announcements(root: PathBuf) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| client.list_announcements()))
}

async fn badge_count(root: PathBuf) -> Response {
    if let Some(response) = disabled(&root, true) {
        return response;
    }
    match with_client(&root, |client| {
        client.list_tickets(Some("open"), None, None)
    }) {
        Ok(Value::Array(tickets)) => json_response(
            json!({"count": tickets.iter().filter(|ticket| ticket.get("updated_at").and_then(Value::as_str) > ticket.get("created_at").and_then(Value::as_str)).count()}),
        ),
        Ok(_) => portal_failed("support portal ticket list is not an array"),
        Err(error) => portal_failed(&error.to_string()),
    }
}

fn with_client<T>(
    root: &std::path::Path,
    operation: impl FnOnce(&mut PortalClient) -> Result<T, PortalOperationError>,
) -> Result<T, PortalOperationError> {
    let mut client = build_client(root, false).map_err(PortalOperationError::Portal)?;
    operation(&mut client)
}

fn portal_response(result: Result<Value, PortalOperationError>) -> Response {
    match result {
        Ok(value) => json_response(value),
        Err(error) => portal_failed(&error.to_string()),
    }
}
fn json_response(value: Value) -> Response {
    axum::Json(value).into_response()
}
fn disabled(root: &std::path::Path, badge: bool) -> Option<Response> {
    (!is_enabled(root)).then(|| {
        error_envelope(
            "feature_unavailable",
            "that feature couldn't be used because it isn't enabled.",
            if badge {
                "Support is not enabled"
            } else {
                "Support is disabled"
            },
            StatusCode::FORBIDDEN,
        )
        .into_response()
    })
}
fn portal_failed(detail: &str) -> Response {
    error_envelope(
        "support_portal_failed",
        "support couldn't be reached right now.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}
fn http_not_found() -> Response {
    error_envelope(
        "http_error",
        "that request didn't finish.",
        "",
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

fn request_error(
    reason: &'static str,
    message: &'static str,
    detail: impl Into<String>,
) -> Response {
    error_envelope(reason, message, detail.into(), StatusCode::BAD_REQUEST).into_response()
}

fn missing_required(detail: impl Into<String>) -> Response {
    request_error(
        "missing_required_field",
        "a required field is missing.",
        detail,
    )
}

fn invalid_value(detail: impl Into<String>) -> Response {
    request_error(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
    )
}

fn payload_too_large(detail: impl Into<String>) -> Response {
    error_envelope(
        "payload_too_large",
        "that request couldn't be accepted because it's too large.",
        detail.into(),
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .into_response()
}

fn action_id(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn ticket_id(id: &str) -> Option<i64> {
    id.parse::<i64>().ok()
}

fn with_mutation_client<T>(
    root: &FsPath,
    anonymous: bool,
    operation: impl FnOnce(&mut PortalClient) -> Result<T, PortalOperationError>,
) -> Result<T, PortalOperationError> {
    let mut client = build_client(root, anonymous).map_err(PortalOperationError::Portal)?;
    operation(&mut client)
}

fn mutation_response(result: Result<Value, PortalOperationError>) -> Response {
    match result {
        Ok(value) => (StatusCode::CREATED, axum::Json(value)).into_response(),
        Err(PortalOperationError::Operation(error)) => operation_error_response(error),
        Err(PortalOperationError::Portal(error)) => portal_failed(&error.to_string()),
    }
}

fn operation_error_response(error: OperationError) -> Response {
    let reason = error
        .reason_code()
        .expect("all operation errors map to routes");
    let status = StatusCode::from_u16(
        error
            .http_status()
            .expect("all operation errors have a status"),
    )
    .expect("operation status is valid");
    error_envelope(
        reason,
        error
            .owner_message()
            .expect("all operation errors have a message"),
        error.to_string(),
        status,
    )
    .into_response()
}

fn object_text<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

fn draft_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn captured_day() -> String {
    Local::now().format("%Y%m%d").to_string()
}

fn capture_draft_event(
    root: &FsPath,
    verb: &str,
    payload: Value,
    diagnostics_snapshot: Value,
) -> Response {
    let ts = Utc::now().timestamp_millis();
    let draft_id = draft_id();
    let captured_day = captured_day();
    let event = Map::from_iter([
        ("ts".to_owned(), json!(ts)),
        ("draft_id".to_owned(), json!(draft_id)),
        ("captured_day".to_owned(), json!(captured_day)),
        ("verb".to_owned(), json!(verb)),
        ("payload".to_owned(), payload),
        ("diagnostics_snapshot".to_owned(), diagnostics_snapshot),
    ]);
    if let Err(error) = record_draft_captured(root, &draft_id, &captured_day) {
        return portal_failed(&error.to_string());
    }
    if let Err(error) = append_support_draft(root, event) {
        return portal_failed(&error.to_string());
    }
    json_response(json!({"draft_id": draft_id}))
}

async fn capture_draft(root: PathBuf, request: Request) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let (parts, body) = request.into_parts();
    let multipart = parts
        .headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("multipart/form-data"));
    let body = match to_bytes(body, solstone_core_convey_http::serve::STANDARD_BODY_LIMIT).await {
        Ok(body) => body,
        Err(error) => {
            let is_length_limit = std::error::Error::source(&error)
                .is_some_and(|source| source.is::<http_body_util::LengthLimitError>());
            if is_length_limit {
                return payload_too_large(error.to_string());
            }
            return invalid_value(error.to_string());
        }
    };
    if multipart {
        let request = Request::from_parts(parts, Body::from(body.clone()));
        let mut multipart = match Multipart::from_request(request, &()).await {
            Ok(multipart) => multipart,
            Err(error) => return invalid_value(error.to_string()),
        };
        let mut fields = std::collections::BTreeMap::new();
        let mut upload = None;
        loop {
            let field = match multipart.next_field().await {
                Ok(Some(field)) => field,
                Ok(None) => break,
                Err(error) => return invalid_value(error.to_string()),
            };
            let name = field.name().unwrap_or_default().to_owned();
            let filename = field.file_name().map(ToOwned::to_owned);
            let bytes = match field.bytes().await {
                Ok(bytes) => bytes,
                Err(error) => return invalid_value(error.to_string()),
            };
            if name == "file" {
                upload = Some((filename, bytes));
            } else if let Ok(value) = String::from_utf8(bytes.to_vec()) {
                fields.insert(name, value);
            }
        }
        if let Some((filename, bytes)) = upload {
            let verb = fields.get("verb").map(String::as_str);
            if verb != Some("attach") {
                return invalid_value("verb must be attach for multipart draft capture");
            }
            let ticket_id = match fields
                .get("ticket_id")
                .and_then(|value| value.parse::<i64>().ok())
            {
                Some(ticket_id) => ticket_id,
                None => return invalid_value("ticket_id must be an integer"),
            };
            let filename = match filename.filter(|filename| !filename.is_empty()) {
                Some(filename) => filename,
                None => return missing_required("No filename"),
            };
            let suffix = FsPath::new(&filename)
                .extension()
                .and_then(|suffix| suffix.to_str())
                .map_or_else(String::new, |suffix| {
                    format!(".{}", suffix.to_ascii_lowercase())
                });
            let Some((_, content_type)) = PortalClient::allowed_content_types()
                .iter()
                .find(|(allowed, _)| *allowed == suffix)
            else {
                return invalid_value(unsupported_suffix_detail(&suffix));
            };
            if bytes.len() as u64 > PortalClient::MAX_ATTACHMENT_SIZE {
                return invalid_value(format!(
                    "File too large: {:.1} MB (max {:.0} MB)",
                    bytes.len() as f64 / 1024.0 / 1024.0,
                    PortalClient::MAX_ATTACHMENT_SIZE as f64 / 1024.0 / 1024.0
                ));
            }
            return capture_draft_event(
                &root,
                "attach",
                json!({
                    "ticket_id": ticket_id,
                    "filename": filename,
                    "content_type": content_type,
                    "byte_size": bytes.len(),
                    "content_b64": STANDARD.encode(bytes),
                }),
                Value::Null,
            );
        }
    }
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => return invalid_value(error.to_string()),
    };
    let verb = payload.get("verb").and_then(Value::as_str);
    let draft_payload = payload.get("payload").filter(|value| !value.is_null());
    let (Some(verb), Some(draft_payload)) = (verb, draft_payload) else {
        return missing_required("verb and payload are required");
    };
    if !matches!(
        verb,
        "create" | "feedback" | "reply" | "close" | "resolved" | "still_need_help"
    ) || !draft_payload.is_object()
    {
        return invalid_value(
            "verb must be create|feedback|reply|close|resolved|still_need_help and payload must be an object",
        );
    }
    capture_draft_event(
        &root,
        verb,
        draft_payload.clone(),
        payload
            .get("diagnostics_snapshot")
            .cloned()
            .unwrap_or(Value::Null),
    )
}

enum DraftLookup {
    NotFound,
    AlreadyTerminal(String),
    Ready(Value),
}

fn resolve_draft(root: &FsPath, draft_id: &str) -> Result<DraftLookup, String> {
    match resolve_draft_outcome(root, draft_id) {
        Ok(Some(status)) => return Ok(DraftLookup::AlreadyTerminal(status)),
        Ok(None) => {}
        Err(error) => return Err(error.to_string()),
    }
    match load_draft_event(root, draft_id) {
        Ok(Some(event)) => Ok(DraftLookup::Ready(event)),
        Ok(None) => Ok(DraftLookup::NotFound),
        Err(error) => Err(error.to_string()),
    }
}

fn draft_outcome_response(outcome: &str, ticket_id: Option<i64>) -> Response {
    let mut body = json!({"ok": true, "outcome": outcome});
    if let Some(id) = ticket_id {
        body["ticket_id"] = json!(id);
    }
    json_response(body)
}

fn as_ticket_id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn required_draft_id(payload: &Value) -> Option<&str> {
    object_text(payload, "draft_id").filter(|value| !value.is_empty())
}

fn terminal_draft_response(status: &str) -> Response {
    draft_outcome_response(
        if status == "submitted" {
            "already_submitted"
        } else {
            "cancelled"
        },
        None,
    )
}

fn dispatch_error(error: PortalOperationError) -> Response {
    match error {
        PortalOperationError::Operation(error) => operation_error_response(error),
        PortalOperationError::Portal(error) => portal_failed(&error.to_string()),
    }
}

async fn confirm_draft(root: PathBuf, Json(payload): Json<Value>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let Some(draft_id) = required_draft_id(&payload) else {
        return missing_required("draft_id is required");
    };
    let draft_id = draft_id.to_owned();
    match resolve_draft(&root, &draft_id) {
        Err(error) => portal_failed(&error),
        Ok(DraftLookup::NotFound) => draft_outcome_response("not_found", None),
        Ok(DraftLookup::AlreadyTerminal(status)) => terminal_draft_response(&status),
        Ok(DraftLookup::Ready(event)) => confirm_ready_draft(&root, &draft_id, event),
    }
}

fn confirm_ready_draft(root: &FsPath, draft_id: &str, event: Value) -> Response {
    let event_payload = event.get("payload").cloned().unwrap_or(Value::Null);
    let user_context = event
        .get("diagnostics_snapshot")
        .filter(|value| !value.is_null())
        .cloned()
        .or_else(|| event_payload.get("user_context").cloned());
    let anonymous = event_payload
        .get("anonymous")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let verb = event.get("verb").and_then(Value::as_str).unwrap_or("");
    let dispatched = match verb {
        "create" => with_mutation_client(root, anonymous, |client| {
            client.create_ticket(
                object_text(&event_payload, "product").unwrap_or("solstone"),
                object_text(&event_payload, "subject").unwrap_or_default(),
                object_text(&event_payload, "description").unwrap_or_default(),
                object_text(&event_payload, "severity").unwrap_or("medium"),
                object_text(&event_payload, "category"),
                object_text(&event_payload, "user_email"),
                user_context.clone(),
                draft_id,
            )
        }),
        "feedback" => with_mutation_client(root, anonymous, |client| {
            client.submit_feedback(
                object_text(&event_payload, "body").unwrap_or_default(),
                object_text(&event_payload, "product").unwrap_or("solstone"),
                object_text(&event_payload, "user_email"),
                user_context.clone(),
                draft_id,
            )
        }),
        "reply" => {
            let Some(ticket_id) = event_payload.get("ticket_id").and_then(as_ticket_id) else {
                return invalid_value("ticket_id must be an integer");
            };
            with_mutation_client(root, anonymous, |client| {
                client.reply_to_ticket(
                    ticket_id,
                    object_text(&event_payload, "content").unwrap_or_default(),
                    draft_id,
                )
            })
        }
        "attach" => {
            let Some(ticket_id) = event_payload.get("ticket_id").and_then(as_ticket_id) else {
                return invalid_value("ticket_id must be an integer");
            };
            let Some(bytes) = event_payload
                .get("content_b64")
                .and_then(Value::as_str)
                .and_then(|value| STANDARD.decode(value).ok())
            else {
                return invalid_value("content_b64 must be valid base64");
            };
            let filename = object_text(&event_payload, "filename").unwrap_or_default();
            let suffix = FsPath::new(filename)
                .extension()
                .and_then(|suffix| suffix.to_str())
                .map_or_else(String::new, |suffix| {
                    format!(".{}", suffix.to_ascii_lowercase())
                });
            let mut temp = match Builder::new().suffix(&suffix).tempfile() {
                Ok(temp) => temp,
                Err(error) => return portal_failed(&error.to_string()),
            };
            #[cfg(unix)]
            if let Err(error) = {
                use std::os::unix::fs::PermissionsExt;
                temp.as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o600))
            } {
                return portal_failed(&error.to_string());
            }
            if let Err(error) = temp.write_all(&bytes).and_then(|()| temp.flush()) {
                return portal_failed(&error.to_string());
            }
            with_mutation_client(root, anonymous, |client| {
                client.attach_file(
                    ticket_id,
                    temp.path(),
                    draft_id,
                    0,
                    object_text(&event_payload, "filename"),
                    object_text(&event_payload, "content_type"),
                )
            })
        }
        "close" | "resolved" | "still_need_help" => {
            let Some(ticket_id) = event_payload.get("ticket_id").and_then(as_ticket_id) else {
                return invalid_value("ticket_id must be an integer");
            };
            with_mutation_client(root, anonymous, |client| match verb {
                "close" => client.close_ticket(ticket_id, draft_id),
                "resolved" => client.confirm_resolution(ticket_id, draft_id),
                _ => client.still_need_help(ticket_id, draft_id),
            })
        }
        _ => return invalid_value("unknown draft verb"),
    };
    let value = match dispatched {
        Ok(value) => value,
        Err(error) => return dispatch_error(error),
    };
    let ticket_id = value
        .get("ticket_id")
        .and_then(as_ticket_id)
        .or_else(|| event_payload.get("ticket_id").and_then(as_ticket_id));
    if let Err(error) = mark_draft_submitted(root, draft_id) {
        return portal_failed(&error.to_string());
    }
    match resolve_draft_outcome(root, draft_id) {
        Ok(Some(status)) if status == "submitted" => draft_outcome_response("submitted", ticket_id),
        Ok(Some(status)) => draft_outcome_response(&status, None),
        Ok(None) => portal_failed("support draft mark is missing after write"),
        Err(error) => portal_failed(&error.to_string()),
    }
}

async fn cancel_draft(root: PathBuf, Json(payload): Json<Value>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let Some(draft_id) = required_draft_id(&payload) else {
        return missing_required("draft_id is required");
    };
    let draft_id = draft_id.to_owned();
    match resolve_draft(&root, &draft_id) {
        Err(error) => portal_failed(&error),
        Ok(DraftLookup::NotFound) => draft_outcome_response("not_found", None),
        Ok(DraftLookup::AlreadyTerminal(status)) => terminal_draft_response(&status),
        Ok(DraftLookup::Ready(_)) => {
            if let Err(error) = mark_draft_cancelled(&root, &draft_id) {
                return portal_failed(&error.to_string());
            }
            match resolve_draft_outcome(&root, &draft_id) {
                Ok(Some(status)) if status == "submitted" => {
                    draft_outcome_response("already_submitted", None)
                }
                Ok(Some(status)) => draft_outcome_response(&status, None),
                Ok(None) => portal_failed("support draft mark is missing after write"),
                Err(error) => portal_failed(&error.to_string()),
            }
        }
    }
}

async fn register(root: PathBuf) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let mut client = match build_client(&root, false) {
        Ok(client) => client,
        Err(_) => return portal_failed("Registration with the support portal failed."),
    };
    if client.register().is_err() {
        return portal_failed("Registration with the support portal failed.");
    }
    json_response(json!({"handle": client.handle()}))
}

async fn create_ticket(
    root: PathBuf,
    headers: axum::http::HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let action_id = match action_id(&headers) {
        Some(action_id) => action_id,
        None => return missing_required("Idempotency-Key header is required"),
    };
    let (Some(subject), Some(description)) = (
        object_text(&payload, "subject").filter(|value| !value.is_empty()),
        object_text(&payload, "description").filter(|value| !value.is_empty()),
    ) else {
        return missing_required("subject and description are required");
    };
    let auto_context = payload
        .get("auto_context")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let user_context = if auto_context {
        let mut diagnostics = collect_all(&root, Local::now(), native_platform());
        if let Some(context) = payload.get("user_context").and_then(Value::as_object) {
            diagnostics.extend(context.clone());
        }
        Some(Value::Object(diagnostics))
    } else {
        payload.get("user_context").cloned()
    };
    mutation_response(with_mutation_client(
        &root,
        payload
            .get("anonymous")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        |client| {
            client.create_ticket(
                object_text(&payload, "product").unwrap_or("solstone"),
                subject,
                description,
                object_text(&payload, "severity").unwrap_or("medium"),
                object_text(&payload, "category"),
                object_text(&payload, "user_email"),
                user_context,
                &action_id,
            )
        },
    ))
}

async fn reply_to_ticket(
    root: PathBuf,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    let ticket_id = match ticket_id(&id) {
        Some(ticket_id) => ticket_id,
        None => return http_not_found(),
    };
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let action_id = match action_id(&headers) {
        Some(action_id) => action_id,
        None => return missing_required("Idempotency-Key header is required"),
    };
    let Some(content) = object_text(&payload, "content").filter(|value| !value.is_empty()) else {
        return missing_required("content is required");
    };
    mutation_response(with_mutation_client(&root, false, |client| {
        client.reply_to_ticket(ticket_id, content, &action_id)
    }))
}

fn unsupported_suffix_detail(suffix: &str) -> String {
    let mut suffixes = PortalClient::allowed_content_types()
        .iter()
        .map(|(suffix, _)| *suffix)
        .collect::<Vec<_>>();
    suffixes.sort_unstable();
    format!(
        "Unsupported file type: {suffix}. Allowed: {}",
        suffixes.join(", ")
    )
}

async fn upload_attachment(
    root: PathBuf,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let ticket_id = match ticket_id(&id) {
        Some(ticket_id) => ticket_id,
        None => return http_not_found(),
    };
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let action_id = match action_id(&headers) {
        Some(action_id) => action_id,
        None => return missing_required("Idempotency-Key header is required"),
    };
    let mut index = None;
    let mut file = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return invalid_value(error.to_string()),
        };
        let name = field.name().unwrap_or_default().to_owned();
        let filename = field.file_name().map(ToOwned::to_owned);
        let bytes = match field.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => return invalid_value(error.to_string()),
        };
        if name == "file" {
            file = Some((filename, bytes));
        } else if name == "index" {
            index = String::from_utf8(bytes.to_vec()).ok();
        }
    }
    let (filename, bytes) = match file {
        Some((filename, bytes)) => (filename, bytes),
        None => return missing_required("No file provided"),
    };
    let filename = match filename.filter(|filename| !filename.is_empty()) {
        Some(filename) => filename,
        None => return missing_required("No filename"),
    };
    let index = match index.as_deref().unwrap_or("0").parse::<i64>() {
        Ok(index) => index,
        Err(_) => return invalid_value("index must be an integer"),
    };
    if index < 0 {
        return invalid_value("index must be non-negative");
    }
    let suffix = FsPath::new(&filename)
        .extension()
        .and_then(|suffix| suffix.to_str())
        .map_or_else(String::new, |suffix| {
            format!(".{}", suffix.to_ascii_lowercase())
        });
    if !PortalClient::allowed_content_types()
        .iter()
        .any(|(allowed, _)| *allowed == suffix)
    {
        return invalid_value(unsupported_suffix_detail(&suffix));
    }
    let mut temp = match Builder::new().suffix(&suffix).tempfile() {
        Ok(temp) => temp,
        Err(error) => return portal_failed(&error.to_string()),
    };
    #[cfg(unix)]
    if let Err(error) = {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
    } {
        return portal_failed(&error.to_string());
    }
    if let Err(error) = temp.write_all(&bytes).and_then(|()| temp.flush()) {
        return portal_failed(&error.to_string());
    }
    mutation_response(with_mutation_client(&root, false, |client| {
        client.attach_file(
            ticket_id,
            temp.path(),
            &action_id,
            index as u64,
            Some(&filename),
            None,
        )
    }))
}

async fn close_ticket(
    root: PathBuf,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    lifecycle_mutation(root, id, headers, |client, ticket_id, action_id| {
        client.close_ticket(ticket_id, action_id)
    })
}

async fn confirm_resolution(
    root: PathBuf,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    lifecycle_mutation(root, id, headers, |client, ticket_id, action_id| {
        client.confirm_resolution(ticket_id, action_id)
    })
}

async fn still_need_help(
    root: PathBuf,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    lifecycle_mutation(root, id, headers, |client, ticket_id, action_id| {
        client.still_need_help(ticket_id, action_id)
    })
}

fn lifecycle_mutation(
    root: PathBuf,
    id: String,
    headers: axum::http::HeaderMap,
    operation: impl FnOnce(&mut PortalClient, i64, &str) -> Result<Value, PortalOperationError>,
) -> Response {
    let ticket_id = match ticket_id(&id) {
        Some(ticket_id) => ticket_id,
        None => return http_not_found(),
    };
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let action_id = match action_id(&headers) {
        Some(action_id) => action_id,
        None => return missing_required("Idempotency-Key header is required"),
    };
    mutation_response(with_mutation_client(&root, false, |client| {
        operation(client, ticket_id, &action_id)
    }))
}

async fn submit_feedback(
    root: PathBuf,
    headers: axum::http::HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let action_id = match action_id(&headers) {
        Some(action_id) => action_id,
        None => return missing_required("Idempotency-Key header is required"),
    };
    let Some(body) = object_text(&payload, "body").filter(|value| !value.is_empty()) else {
        return missing_required("body is required");
    };
    let anonymous = payload
        .get("anonymous")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let user_email = (!anonymous)
        .then(|| object_text(&payload, "user_email").map(str::trim))
        .flatten()
        .filter(|email| !email.is_empty());
    mutation_response(with_mutation_client(&root, anonymous, |client| {
        client.submit_feedback(
            body,
            object_text(&payload, "product").unwrap_or("solstone"),
            user_email,
            None,
            &action_id,
        )
    }))
}

#[derive(serde::Deserialize)]
struct TicketQuery {
    status: Option<String>,
    product: Option<String>,
    severity: Option<String>,
}
#[derive(serde::Deserialize)]
struct ClosedQuery {
    cursor: Option<String>,
}
#[derive(serde::Deserialize)]
struct ArticleQuery {
    q: Option<String>,
}

#[cfg(test)]
mod corpus;
