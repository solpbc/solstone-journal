// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deleting a stretch of time from the transcripts page: what it would take,
//! for the confirmation, and the delete itself, which moves everything
//! recorded then to the trash as one thing
//! (`solstone_core_import::stretch_trash`).

use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::USER_AGENT;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    window_catalog::{response_locale, text},
};
use solstone_core_import::stretch_trash::{self, Refused};
use solstone_core_removals::{Page, Surface};
use solstone_core_trash::TrashError;

use crate::{AppState, legacy_error_response};

/// What a stretch request names: its bounds in the owner's wall time.
#[derive(Debug, serde::Deserialize)]
struct StretchBody {
    from: String,
    to: String,
    /// Erase it now rather than leave it in the trash.
    #[serde(default)]
    erase: bool,
    /// And remove it from the backups too, as the owner chose with erase now.
    #[serde(default)]
    backups: bool,
}

#[allow(clippy::result_large_err)]
fn read_body(
    locale: &str,
    body: &Bytes,
) -> Result<(jiff::civil::DateTime, jiff::civil::DateTime, bool, bool), Response> {
    let unreadable = || {
        legacy_error_response(
            "invalid_request",
            text(
                locale,
                "journal.window.transcripts.response.error.that.stretch.couldn.t.be.read",
            ),
            text(
                locale,
                "journal.window.transcripts.response.detail.invalid.stretch.body",
            ),
            StatusCode::BAD_REQUEST,
        )
    };
    let request: StretchBody = serde_json::from_slice(body).map_err(|_| unreadable())?;
    let from = stretch_trash::parse_wall(&request.from).ok_or_else(unreadable)?;
    let to = stretch_trash::parse_wall(&request.to).ok_or_else(unreadable)?;
    Ok((from, to, request.erase, request.backups))
}

fn refused(locale: &str, refused: Refused) -> Response {
    let (code, message) = match refused {
        Refused::Backwards => (
            "stretch_backwards",
            text(
                locale,
                "journal.window.transcripts.response.error.the.end.has.to.be.after",
            ),
        ),
        Refused::TooLong => (
            "stretch_too_long",
            text(
                locale,
                "journal.window.transcripts.response.error.choose.days.or.less",
            ),
        ),
    };
    legacy_error_response(code, message.clone(), message, StatusCode::BAD_REQUEST)
}

fn stopped(locale: &str) -> Response {
    legacy_error_response(
        "stretch_delete_failed",
        text(
            locale,
            "journal.window.transcripts.response.error.your.journal.couldn.t.finish.that",
        ),
        text(
            locale,
            "journal.window.transcripts.response.detail.the.stretch.task.stopped",
        ),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}

/// What deleting a stretch would take: its bounds once whole segments are
/// counted, the kinds of recording in it, what stays because it is still in
/// flight, and what happens to each import. Reads only; counts no recordings.
pub(crate) async fn preview_stretch(
    selected_locale: Option<Extension<ResponseLocale>>,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let locale = response_locale(selected_locale);
    let (from, to, _, _) = match read_body(locale, &body) {
        Ok(read) => read,
        Err(response) => return response,
    };
    let root = state.journal_root.clone();
    match tokio::task::spawn_blocking(move || stretch_trash::plan(&root, from, to)).await {
        Ok(Ok(plan)) => Json(json!({
            "from": plan.from,
            "to": plan.to,
            "asked_from": plan.asked_from,
            "asked_to": plan.asked_to,
            "kinds": plan.kinds,
            "empty": plan.segments.is_empty(),
            "left_out": plan.left_out,
            "imports": plan.imports,
            "in_trash": plan.in_trash,
        }))
        .into_response(),
        Ok(Err(why)) => refused(locale, why),
        Err(_) => stopped(locale),
    }
}

/// Move a stretch of time to the trash as one thing, or erase it now.
pub(crate) async fn delete_stretch(
    selected_locale: Option<Extension<ResponseLocale>>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let locale = response_locale(selected_locale);
    let (from, to, erase, backups) = match read_body(locale, &body) {
        Ok(read) => read,
        Err(response) => return response,
    };
    let surface = Surface::of_request(
        Page::Transcripts,
        headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok()),
    );
    let root = state.journal_root.clone();
    match tokio::task::spawn_blocking(move || {
        stretch_trash::trash_with(&root, from, to, surface, erase, backups)
    })
    .await
    {
        Ok(Ok(Ok(outcome))) => Json(json!({
            "success": true,
            "stretch": outcome.stretch,
            "erase_at": outcome.erase_at,
            "moved": outcome.moved,
            "erased": erase && outcome.erased == outcome.moved,
            "imports_taken": outcome.imports_taken,
            "refused": outcome.refused,
            "left_out": outcome.left_out,
            "imports": outcome.imports,
        }))
        .into_response(),
        Ok(Ok(Err(why))) => refused(locale, why),
        Ok(Err(TrashError::Missing(_))) => legacy_error_response(
            "stretch_empty",
            text(
                locale,
                "journal.window.transcripts.response.error.nothing.from.that.time.could.go",
            ),
            text(
                locale,
                "journal.window.transcripts.response.detail.the.stretch.holds.no.segment",
            ),
            StatusCode::NOT_FOUND,
        ),
        Ok(Err(error)) => legacy_error_response(
            "stretch_delete_failed",
            text(
                locale,
                "journal.window.transcripts.response.error.your.journal.couldn.t.move.that.variant2",
            ),
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        Err(_) => stopped(locale),
    }
}
