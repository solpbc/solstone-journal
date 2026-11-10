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
//! Every word the card shows for this lives in [`COPY`], so it can be read and
//! changed in one place.

use std::path::Path;

use axum::{Json, extract::State, http::StatusCode, response::Response};
use serde_json::{Value, json};
use solstone_core_journal_config::PREFERRED_BODY_FACET;

use crate::{AppState, http::json as json_response};

/// What the card says about the facet, by key.
pub(crate) const COPY: &[(&str, &str)] = &[
    ("heading", "facet"),
    (
        "ask_existing",
        "file your workouts under the facet you named {facet}?",
    ),
    ("ask_make_body", "make a body facet for your workouts?"),
    (
        "ask_unmute",
        "your body facet is muted. turn it back on for your workouts?",
    ),
    (
        "no_means",
        "if you say no, new workouts aren't listed on the activities page, no people or things come from them, and agents limited to some facets won't see them. search and the body app still show them.",
    ),
    ("yes", "yes"),
    ("no", "no"),
    ("this_import", "facet for this import"),
    ("none_option", "no facet"),
    (
        "files_earlier",
        "this files every workout in this download under {facet}, including any already listed on the activities page. the preview says how many of those would move.",
    ),
    (
        "none_hint",
        "new workouts in this import aren't listed on the activities page, no people or things come from them, and agents limited to some facets won't see them. search and the body app still show them.",
    ),
    (
        "unanswered",
        "answer the facet question first, then preview again.",
    ),
    ("answer_failed", "your answer couldn't be saved. try again."),
    (
        "offer_changed",
        "the facet question changed after you saw it, so your answer wasn't saved. the card now shows where it stands.",
    ),
];

/// Where the Apple Health card's words differ: its import holds workouts,
/// sleep and readings, and no owner-written words, so no people or things come
/// from it. These are the words the Apple Health terminal question uses.
pub(crate) const APPLE_COPY: &[(&str, &str)] = &[
    (
        "ask_existing",
        "file your workouts, sleep and other body data under the facet you named {facet}?",
    ),
    (
        "ask_make_body",
        "make a body facet for your workouts, sleep and other body data?",
    ),
    (
        "ask_unmute",
        "your body facet is muted. turn it back on for your workouts, sleep and other body data?",
    ),
    (
        "no_means",
        "if you say no, new workouts and sleep aren't listed on the activities page, and agents limited to some facets won't see them. search and the body app still show them.",
    ),
    (
        "files_earlier",
        "this files everything in this export under {facet}, including what's already listed on the activities page. the preview says how many of those could move, at most.",
    ),
    (
        "none_hint",
        "new workouts and sleep in this import aren't listed on the activities page, and agents limited to some facets won't see them. search and the body app still show them.",
    ),
];

/// Whether a request is from the Apple Health card.
fn for_apple(source: Option<&str>) -> bool {
    source == Some("apple_health")
}

fn copy(apple: bool) -> Value {
    let mut words: serde_json::Map<String, Value> = COPY
        .iter()
        .map(|(key, text)| ((*key).to_owned(), json!(text)))
        .collect();
    if apple {
        for (key, text) in APPLE_COPY {
            words.insert((*key).to_owned(), json!(text));
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

fn body(root: &Path, apple: bool) -> Result<Value, String> {
    let (offer, facets) = card_offer(root)?;
    Ok(json!({
        "offer": offer_value(&offer),
        "facets": facets
            .into_iter()
            .map(|(name, title)| json!({"name": name, "title": title}))
            .collect::<Vec<_>>(),
        "copy": copy(apple),
    }))
}

fn failed(status: StatusCode, detail: String) -> Response {
    json_response(
        status,
        json!({"error": text("answer_failed"), "reason_code": "body_facet_failed", "detail": detail}),
    )
}

fn text(key: &str) -> &'static str {
    COPY.iter()
        .find_map(|(name, text)| (*name == key).then_some(*text))
        .unwrap_or_default()
}

/// `GET /app/import/api/body-facet[?source=apple_health]`: what the Strava or
/// Apple Health card asks or remembers. The offer is one for every body
/// importer; only the words differ.
pub(crate) async fn offer(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let root = state.root.clone();
    let apple = for_apple(query.get("source").map(String::as_str));
    match tokio::task::spawn_blocking(move || body(&root, apple)).await {
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
pub(crate) async fn answer(State(state): State<AppState>, Json(data): Json<Value>) -> Response {
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
        record_answer(&root, yes, &shown).and_then(|()| Ok(body(&root, apple)?))
    })
    .await
    {
        Ok(Ok(body)) => json_response(StatusCode::OK, body),
        Ok(Err(AnswerRefused::OfferChanged)) => json_response(
            StatusCode::CONFLICT,
            json!({"error": text("offer_changed"), "reason_code": "body_facet_offer_changed",
                "detail": "the facet question changed since it was shown"}),
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
