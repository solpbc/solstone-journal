// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{Json, extract::State, http::StatusCode, response::Response};
use serde::Deserialize;
use serde_json::json;
use solstone_core_import::originals;

use crate::{AppState, http::json as json_response};

pub(crate) async fn read(State(state): State<AppState>) -> Response {
    match originals::preferences(&state.root) {
        Ok(preferences) => json_response(StatusCode::OK, json!({ "keep_originals": preferences })),
        Err(error) => storage_error(&error, "journal.window.import.response.originals.read"),
    }
}

#[derive(Deserialize)]
pub(crate) struct Choice {
    source: String,
    keep: bool,
}

pub(crate) async fn write(State(state): State<AppState>, Json(choice): Json<Choice>) -> Response {
    if !originals::SOURCES.contains(&choice.source.as_str()) {
        return crate::http::refusal(
            StatusCode::BAD_REQUEST,
            json!({ "error": "" }),
            "journal.window.import.response.originals.unknown.source",
        );
    }
    match tokio::task::spawn_blocking(move || {
        originals::set_preference(&state.root, &choice.source, choice.keep)
    })
    .await
    {
        Ok(Ok(())) => json_response(StatusCode::OK, json!({ "keep": choice.keep })),
        Ok(Err(error)) => storage_error(&error, "journal.window.import.response.originals.write"),
        Err(error) => storage_error(
            &error.to_string(),
            "journal.window.import.response.originals.write",
        ),
    }
}

fn storage_error(error: &str, message: &'static str) -> Response {
    log::warn!("import originals settings: {error}");
    crate::http::refusal(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": "" }),
        message,
    )
}
