// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::json;
use solstone_core_convey_http::envelope::error_envelope;

pub fn internal_error() -> axum::response::Response {
    // `error_envelope` remains the one source for message/reason/status. This local
    // insertion order is stable with both serde map backends: preserve_order retains it,
    // while BTreeMap sorts detail, error, reason_code into the same byte order.
    let (status, Json(envelope)) = error_envelope(
        "internal_error",
        "that request couldn't be completed.",
        "",
        StatusCode::INTERNAL_SERVER_ERROR,
    );
    (
        status,
        Json(json!({
            "detail": envelope.detail,
            "error": envelope.error,
            "reason_code": envelope.reason_code,
        })),
    )
        .into_response()
}

pub fn error(
    reason_code: &str,
    message: &str,
    detail: String,
    status: StatusCode,
) -> axum::response::Response {
    error_envelope(reason_code, message, detail, status).into_response()
}
