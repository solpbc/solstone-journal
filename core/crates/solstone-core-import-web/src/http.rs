// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::Body,
    http::{Response, StatusCode, header},
    response::IntoResponse,
};
use serde_json::{Value, json};

pub(crate) const WERKZEUG_NOT_FOUND: &str = "<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n<h1>Not Found</h1>\n<p>The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.</p>\n";

pub(crate) const WERKZEUG_UNAUTHORIZED: &str = "<!doctype html>\n<html lang=en>\n<title>401 Unauthorized</title>\n<h1>Unauthorized</h1>\n<p>Missing or invalid authentication</p>\n";

pub(crate) fn bytes(bytes: &'static [u8], content_type: &'static str) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("embedded import asset response")
}

pub(crate) fn html_not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(WERKZEUG_NOT_FOUND))
        .expect("import not-found response")
}

pub(crate) fn html_auth_failure(status: StatusCode, description: &str) -> Response<Body> {
    let body = match status {
        StatusCode::UNAUTHORIZED if description == "Missing or invalid authentication" => {
            WERKZEUG_UNAUTHORIZED.to_owned()
        }
        StatusCode::UNAUTHORIZED => format!(
            "<!doctype html>\n<html lang=en>\n<title>401 Unauthorized</title>\n<h1>Unauthorized</h1>\n<p>{description}</p>\n"
        ),
        StatusCode::FORBIDDEN => format!(
            "<!doctype html>\n<html lang=en>\n<title>403 Forbidden</title>\n<h1>Forbidden</h1>\n<p>{description}</p>\n"
        ),
        _ => unreachable!("journal-source authentication is 401 or 403"),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(body))
        .expect("import authentication response")
}

pub(crate) fn json(status: StatusCode, value: Value) -> axum::response::Response {
    (status, axum::Json(value)).into_response()
}

pub(crate) fn error(
    status: StatusCode,
    error: &str,
    reason_code: &str,
    detail: String,
) -> axum::response::Response {
    json(
        status,
        json!({"error": error, "reason_code": reason_code, "detail": detail}),
    )
}

pub(crate) fn import_not_found(detail: &str) -> axum::response::Response {
    error(
        StatusCode::NOT_FOUND,
        "that import isn't in your journal.",
        "import_not_found",
        detail.to_owned(),
    )
}
