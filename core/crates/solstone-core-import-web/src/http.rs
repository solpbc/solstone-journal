// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    extract::Request,
    http::{Response, StatusCode, header},
    middleware::Next,
    response::IntoResponse,
};
use serde_json::{Value, json};
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    window_catalog::{ENGLISH, text},
};

use crate::mobile_import_contract::{ImportErrorKind, ImportReasonCode};

pub(crate) fn bytes(bytes: &'static [u8], content_type: &'static str) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded import asset response")
}

pub(crate) fn html_not_found() -> Response<Body> {
    solstone_core_convey_http::window_catalog::not_found_response()
}

pub(crate) fn json(status: StatusCode, value: Value) -> axum::response::Response {
    (status, axum::Json(value)).into_response()
}

pub(crate) fn error_body(
    error: &str,
    reason_code: &str,
    detail: String,
    kind: Option<ImportErrorKind>,
) -> Value {
    let mut value = json!({"error": error, "reason_code": reason_code, "detail": detail});
    if let Some(kind) = kind {
        value["kind"] = json!(kind.as_wire());
    }
    value
}

pub(crate) fn error(
    status: StatusCode,
    message: &'static str,
    reason_code: &str,
    detail: String,
    kind: Option<ImportErrorKind>,
) -> axum::response::Response {
    refusal(status, error_body("", reason_code, detail, kind), message)
}

#[derive(Clone)]
struct RefusalCopy {
    body: Value,
    message: &'static str,
}

pub(crate) fn refusal(
    status: StatusCode,
    mut body: Value,
    message: &'static str,
) -> axum::response::Response {
    if body["error"].as_str().is_none_or(str::is_empty) {
        body["error"] = json!(text(ENGLISH, message));
    }
    let mut response = json(status, body.clone());
    response
        .extensions_mut()
        .insert(RefusalCopy { body, message });
    response
}

/// The importer and retained workers produce the same errors for every caller.
/// Resolve only their declared presentation field inside the requesting route.
pub(crate) async fn render_refusal(request: Request, next: Next) -> axum::response::Response {
    let locale = request
        .extensions()
        .get::<ResponseLocale>()
        .map_or(ENGLISH, |locale| locale.0);
    let mut response = next.run(request).await;
    if let Some(mut copy) = response.extensions_mut().remove::<RefusalCopy>()
        && locale != ENGLISH
    {
        copy.body["error"] = json!(text(locale, copy.message));
        let rendered = axum::Json(copy.body).into_response();
        *response.body_mut() = rendered.into_body();
        response.headers_mut().remove(header::CONTENT_LENGTH);
    }
    response
}

pub(crate) fn import_not_found(detail: &str) -> axum::response::Response {
    let code = ImportReasonCode::ImportNotFound;
    error(
        code.status(),
        "journal.window.import.response.error.that.import.isn.t.in.your",
        code.as_wire(),
        detail.to_owned(),
        None,
    )
}
