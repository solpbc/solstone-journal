// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The facet question on the Strava card.
//!
//! The first body import asks whether its workouts go in an existing `body`
//! (or `health`) facet or in a new `body` one, and says what a no means before
//! the owner answers. A yes remembers that facet, making `body` (or turning a
//! muted one back on) when that was the offer; a no remembers none and isn't
//! asked again. Once answered, the remembered facet is what a Strava import
//! starts with, and each import can still pick another facet, or none, for
//! itself without changing what is remembered: that choice travels in the
//! staged import's metadata, never here.
//!
//! The card resolves its copy from the window catalog.

use std::path::Path;

use axum::{Extension, Json, extract::State, http::StatusCode, response::Response};
use serde_json::{Value, json};
use solstone_core_convey_http::{
    coalesce::ResponseLocale,
    window_catalog::{response_locale, text as catalog_text},
};
use solstone_core_journal_config::PREFERRED_BODY_FACET;

use crate::{AppState, http::json as json_response};

/// What the card says about the facet, by key.
pub(crate) const COPY: &[(&str, &str)] = &[
    ("heading", "journal.window.import.body.facet.shared.heading"),
    (
        "ask_existing",
        "!journal.window.import.body.facet.shared.ask.existing",
    ),
    (
        "ask_make_body",
        "!journal.window.import.body.facet.shared.ask.make.body",
    ),
    (
        "ask_unmute",
        "!journal.window.import.body.facet.shared.ask.unmute",
    ),
    (
        "no_means",
        "!journal.window.import.body.facet.shared.no.means",
    ),
    ("yes", "!journal.window.import.body.facet.shared.yes"),
    ("no", "!journal.window.import.body.facet.shared.no"),
    (
        "this_import",
        "!journal.window.import.body.facet.shared.this.import",
    ),
    (
        "none_option",
        "!journal.window.import.body.facet.shared.none.option",
    ),
    (
        "files_earlier",
        "!journal.window.import.body.facet.shared.files.earlier",
    ),
    (
        "none_hint",
        "!journal.window.import.body.facet.shared.none.hint",
    ),
    (
        "unanswered",
        "journal.window.import.body.facet.shared.unanswered",
    ),
    (
        "answer_failed",
        "journal.window.import.body.facet.shared.answer.failed",
    ),
    (
        "offer_changed",
        "journal.window.import.body.facet.shared.offer.changed",
    ),
];

/// Where the Apple Health card's words differ: its import holds workouts,
/// sleep and readings, and no owner-written words, so no people or things come
/// from it. These are the words the Apple Health terminal question uses.
pub(crate) const APPLE_COPY: &[(&str, &str)] = &[
    (
        "ask_existing",
        "!journal.window.import.body.facet.apple.ask.existing",
    ),
    (
        "ask_make_body",
        "!journal.window.import.body.facet.apple.ask.make.body",
    ),
    (
        "ask_unmute",
        "!journal.window.import.body.facet.apple.ask.unmute",
    ),
    (
        "no_means",
        "!journal.window.import.body.facet.apple.no.means",
    ),
    (
        "files_earlier",
        "!journal.window.import.body.facet.apple.files.earlier",
    ),
    (
        "none_hint",
        "!journal.window.import.body.facet.apple.none.hint",
    ),
];

/// Whether a request is from the Apple Health card.
fn for_apple(source: Option<&str>) -> bool {
    source == Some("apple_health")
}

fn copy(locale: &str, apple: bool) -> Value {
    let mut words: serde_json::Map<String, Value> = COPY
        .iter()
        .map(|(key, id)| ((*key).to_owned(), json!(catalog_text(locale, id))))
        .collect();
    if apple {
        for (key, id) in APPLE_COPY {
            words.insert((*key).to_owned(), json!(catalog_text(locale, id)));
        }
    }
    Value::Object(words)
}

use solstone_core_facets::BodyFacetAsk as CardOffer;

fn card_offer(root: &Path) -> Result<(CardOffer, Vec<(String, String)>), String> {
    solstone_core_facets::body_facet_ask(root)
}

/// The offer as the card shows it, and as an answer sends it back.
fn offer_value(offer: &CardOffer) -> Value {
    let (state, facet) = match offer {
        CardOffer::Chosen(facet) => ("chosen", Some(facet.as_str())),
        CardOffer::Declined => ("declined", None),
        CardOffer::AskExisting(facet) => ("ask_existing", Some(facet.as_str())),
        CardOffer::AskMakeBody => ("ask_make_body", Some(PREFERRED_BODY_FACET)),
        CardOffer::AskUnmute => ("ask_unmute", Some(PREFERRED_BODY_FACET)),
    };
    json!({"state": state, "facet": facet})
}

fn body(locale: &str, root: &Path, apple: bool) -> Result<Value, String> {
    let (offer, facets) = card_offer(root)?;
    Ok(json!({
        "offer": offer_value(&offer),
        "facets": facets
            .into_iter()
            .map(|(name, title)| json!({"name": name, "title": title}))
            .collect::<Vec<_>>(),
        "copy": copy(locale, apple),
    }))
}

fn failed(status: StatusCode, detail: String) -> Response {
    crate::http::refusal(
        status,
        json!({"error": "", "reason_code": "body_facet_failed", "detail": detail}),
        copy_id("answer_failed"),
    )
}

fn copy_id(key: &str) -> &'static str {
    COPY.iter()
        .find_map(|(name, text)| (*name == key).then_some(*text))
        .unwrap_or_default()
}

#[cfg(test)]
fn text(key: &str) -> String {
    catalog_text(
        solstone_core_convey_http::window_catalog::ENGLISH,
        copy_id(key),
    )
}

/// `GET /app/import/api/body-facet[?source=apple_health]`: what the Strava or
/// Apple Health card asks or remembers. The offer is one for every body
/// importer; only the words differ.
pub(crate) async fn offer(
    selected_locale: Option<Extension<ResponseLocale>>,
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let root = state.root.clone();
    let apple = for_apple(query.get("source").map(String::as_str));
    match tokio::task::spawn_blocking(move || body(response_locale(selected_locale), &root, apple))
        .await
    {
        Ok(Ok(body)) => json_response(StatusCode::OK, body),
        Ok(Err(error)) => failed(StatusCode::INTERNAL_SERVER_ERROR, error),
        Err(error) => failed(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// Why an answer wasn't recorded.
#[derive(Debug)]
enum AnswerRefused {
    /// The card's question is no longer the one asked now: the owner
    /// answered something they weren't shown.
    OfferChanged,
    Failed(String),
}

impl From<String> for AnswerRefused {
    fn from(detail: String) -> Self {
        Self::Failed(detail)
    }
}

/// `POST /app/import/api/body-facet` with `{"answer": "yes" | "no", "offer":
/// <the offer the card showed>}`: record the owner's answer to the card's
/// question, if it is still the question asked now.
pub(crate) async fn answer(
    selected_locale: Option<Extension<ResponseLocale>>,
    State(state): State<AppState>,
    Json(data): Json<Value>,
) -> Response {
    let yes = match data.get("answer").and_then(Value::as_str) {
        Some("yes") => true,
        Some("no") => false,
        _ => {
            return failed(
                StatusCode::BAD_REQUEST,
                "answer must be yes or no".to_owned(),
            );
        }
    };
    let shown = data.get("offer").cloned().unwrap_or(Value::Null);
    let apple = for_apple(data.get("source").and_then(Value::as_str));
    let root = state.root.clone();
    match tokio::task::spawn_blocking(move || {
        record_answer(&root, yes, &shown)
            .and_then(|()| Ok(body(response_locale(selected_locale), &root, apple)?))
    })
    .await
    {
        Ok(Ok(body)) => json_response(StatusCode::OK, body),
        Ok(Err(AnswerRefused::OfferChanged)) => crate::http::refusal(
            StatusCode::CONFLICT,
            json!({"error": "", "reason_code": "body_facet_offer_changed",
                "detail": "the facet question changed since it was shown"}),
            copy_id("offer_changed"),
        ),
        Ok(Err(AnswerRefused::Failed(error))) => failed(StatusCode::CONFLICT, error),
        Err(error) => failed(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// Record `yes` or no to the offer the card showed, `shown`. An offer that
/// has changed since, or none sent, is refused with nothing written.
fn record_answer(root: &Path, yes: bool, shown: &Value) -> Result<(), AnswerRefused> {
    let (offer, _) = card_offer(root)?;
    if *shown != offer_value(&offer) {
        return Err(AnswerRefused::OfferChanged);
    }
    // Already answered: the card shows the remembered facet, and a repeat
    // answer changes nothing.
    let Some(choice) = solstone_core_facets::apply_body_facet_answer(root, &offer, yes)? else {
        return Ok(());
    };
    solstone_core_journal_config_write::record_body_facet(root, &choice)
        .map(|_| ())
        .map_err(|error| AnswerRefused::Failed(error.to_string()))
}

/// Validate a card's per-import facet as the staged import keeps it: none,
/// or one facet name. `null` clears it.
pub(crate) fn valid_import_facet(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(object) => match object.get("choice").and_then(Value::as_str) {
            Some("none") => object.len() == 1,
            Some("facet") => {
                object.len() == 2
                    && object
                        .get("facet")
                        .and_then(Value::as_str)
                        .is_some_and(solstone_core_journal_config::is_body_facet_name)
            }
            _ => false,
        },
        _ => false,
    }
}

/// The importer arguments for a staged import's per-import facet.
pub(crate) fn command_arguments(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Object(object)) => match object.get("choice").and_then(Value::as_str) {
            Some("none") => vec!["--no-facet".to_owned()],
            Some("facet") => object
                .get("facet")
                .and_then(Value::as_str)
                .map(|facet| vec!["--facet".to_owned(), facet.to_owned()])
                .unwrap_or_default(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "body_facet_tests.rs"]
mod tests;
