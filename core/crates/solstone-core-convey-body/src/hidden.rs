// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's hide for a day of body data: the one write route, and what the
//! read routes take from the hidden days.
//!
//! A hidden day shows nothing on the Body page, and the part of a night or
//! workout that started on it leaves the days after it. Every read decides from
//! the hidden days as it reads; while they can't be read, every day shows.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_format::content::body_hidden_days::{RowDays, is_day};
use solstone_core_format::content::body_records::BodyRow;
use solstone_core_location::body_hidden::{BodyHiddenError, set_body_days_hidden};

use crate::status::invalid_day_response;

/// `POST /app/body/api/hidden`, `{"days": ["YYYYMMDD", ...], "hidden": bool}`:
/// hide or unhide those days. Answers every day hidden after it, and whether
/// search was told.
pub(crate) async fn hidden_route(State(root): State<Arc<PathBuf>>, body: Bytes) -> Response {
    let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(&body) else {
        return invalid_request("Body must be a JSON object");
    };
    let Some(Value::Bool(hidden)) = fields.get("hidden") else {
        return invalid_request("hidden must be true or false");
    };
    let Some(Value::Array(listed)) = fields.get("days") else {
        return invalid_request("days must be a list of days");
    };
    let mut days = BTreeSet::new();
    for day in listed {
        match day.as_str() {
            Some(day) if is_day(day) => {
                days.insert(day.to_owned());
            }
            _ => return invalid_day_response(),
        }
    }
    if days.is_empty() {
        return invalid_request("days must name at least one day");
    }
    match set_body_days_hidden(&root, &days, *hidden) {
        Ok(saved) => Json(json!({
            "days": saved.days,
            "search_queued": saved.search_queued,
        }))
        .into_response(),
        Err(BodyHiddenError::NotADay(_)) => invalid_day_response(),
        Err(BodyHiddenError::Unreadable) => error_envelope(
            "body_hidden_unreadable",
            "your journal can't read which days you hid.",
            "the hidden body days can't be read",
            StatusCode::CONFLICT,
        )
        .into_response(),
        Err(error @ BodyHiddenError::Write(_)) => error_envelope(
            "body_hidden_unsaved",
            "that change couldn't be saved.",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

fn invalid_request(detail: &str) -> Response {
    error_envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

/// Drop from one source's rows on `window_day` every row that goes with a
/// hidden day, and add each such day to `took`. `rows` are all that source
/// holds that day, so a later piece of a night finds where it began.
pub(crate) fn drop_hidden_rows(
    root: &Path,
    source: &str,
    window_day: &str,
    rows: &mut Vec<BodyRow>,
    hidden: &BTreeSet<String>,
    took: &mut BTreeSet<String>,
) {
    if hidden.is_empty() {
        return;
    }
    let zone = solstone_core_journal_config::placed_zone(root, source);
    let days = RowDays::new(window_day, zone, rows.iter());
    rows.retain(|row| {
        let day = days.day_of_row(row);
        if hidden.contains(day.as_ref()) {
            took.insert(day.into_owned());
            false
        } else {
            true
        }
    });
}
