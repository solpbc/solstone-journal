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
            "error": "that facet isn't in your journal.",
            "reason_code": "facet_not_found",
            "detail": "Facet not found",
        })),
    )
        .into_response()
}

pub fn settings_operation_failed() -> Response {
    settings_operation_failed_with_detail(
        "something went wrong — try again, and if it persists, check the health dashboard",
    )
}

pub fn settings_operation_failed_with_detail(detail: impl Into<String>) -> Response {
    error_envelope(
        "settings_operation_failed",
        "those settings couldn't be saved.",
        detail.into(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}

pub fn plaud_validation_unavailable() -> Response {
    error_envelope(
        "plaud_validation_unavailable",
        "that Plaud token can't be validated in this version.",
        "live Plaud validation is not available in the native settings surface",
        StatusCode::NOT_IMPLEMENTED,
    )
    .into_response()
}

pub fn invalid_config_value(detail: impl Into<String>) -> Response {
    error_envelope(
        "invalid_config_value",
        "that setting couldn't be saved because one value was invalid.",
        detail.into(),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn invalid_request_value(detail: impl Into<String>) -> Response {
    error_envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail.into(),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn missing_request_body() -> Response {
    error_envelope(
        "missing_request_body",
        "that request had no data in it.",
        "No data provided",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn missing_required_field(detail: impl Into<String>) -> Response {
    error_envelope(
        "missing_required_field",
        "a required field is missing.",
        detail.into(),
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn config_busy() -> Response {
    error_envelope(
        "config_busy",
        "those settings couldn't be saved right now because they were busy. try again in a moment.",
        "settings configuration is busy",
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

pub fn activity_protected() -> Response {
    error_envelope(
        "activity_protected",
        "that activity couldn't be removed.",
        "Cannot remove always-on activity",
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

pub fn activity_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": "that activity isn't in the facet.",
            "reason_code": "activity_not_found",
            "detail": "Activity not found in facet",
        })),
    )
        .into_response()
}
