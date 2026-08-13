// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    Json,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

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
