// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;

pub fn json_response(value: Value) -> Response {
    Json(value).into_response()
}

pub fn facet_not_found() -> Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(json!({
            "error": "I couldn't find that facet.",
            "reason_code": "facet_not_found",
            "detail": "Facet not found",
        })),
    )
        .into_response()
}

pub fn settings_operation_failed() -> Response {
    error_envelope(
        "settings_operation_failed",
        "I couldn't save those settings.",
        "something went wrong — try again, and if it persists, check the health dashboard",
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}
