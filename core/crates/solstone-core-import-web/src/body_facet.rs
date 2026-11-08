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
use solstone_core_journal_config::{BodyFacetChoice, BodyFacetOffer, PREFERRED_BODY_FACET};

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
        "if you say no, new workouts aren't listed with your activities, no people or things come from them, and agents you've set to only these facets won't see them. search and the body app still show them.",
    ),
    ("yes", "yes"),
    ("no", "no"),
    ("this_import", "facet for this import"),
    ("none_option", "no facet"),
    (
        "files_earlier",
        "this also files workouts from earlier imports of this download under {facet}, apart from ones already listed with your activities.",
    ),
    (
        "none_hint",
        "new workouts in this import aren't listed with your activities, no people or things come from them, and agents you've set to only these facets won't see them. search and the body app still show them.",
    ),
    (
        "unanswered",
        "answer the facet question first, then preview again.",
    ),
    ("answer_failed", "your answer couldn't be saved. try again."),
];

fn copy() -> Value {
    Value::Object(
        COPY.iter()
            .map(|(key, text)| ((*key).to_owned(), json!(text)))
            .collect(),
    )
}

/// The offer as the card reads it: what to ask, or what is remembered.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CardOffer {
    Chosen(String),
    Declined,
    AskExisting(String),
    AskMakeBody,
    /// A `body` facet exists but is muted: offer to turn it back on.
    AskUnmute,
}

fn card_offer(root: &Path) -> Result<(CardOffer, Vec<(String, String)>), String> {
    let read = solstone_core_facets::body_facet_offer(root).map_err(|error| error.to_string())?;
    let inventory = solstone_core_facets::observe_declared_facet_inventory(root)
        .map_err(|error| error.to_string())?;
    let offer = match read.offer {
        BodyFacetOffer::Chosen(facet) => CardOffer::Chosen(facet),
        BodyFacetOffer::Declined => CardOffer::Declined,
        BodyFacetOffer::AskExisting(facet) => CardOffer::AskExisting(facet),
        BodyFacetOffer::AskMakeBody
            if inventory
                .muted
                .iter()
                .any(|facet| facet == PREFERRED_BODY_FACET) =>
        {
            CardOffer::AskUnmute
        }
        BodyFacetOffer::AskMakeBody => CardOffer::AskMakeBody,
    };
    let facets = inventory
        .enabled
        .iter()
        .map(|facet| {
            let title = solstone_core_facets::read_facet_declaration(root, facet)
                .ok()
                .flatten()
                .and_then(|snapshot| {
                    snapshot
                        .value()
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| facet.clone());
            (facet.clone(), title)
        })
        .collect();
    Ok((offer, facets))
}

fn body(root: &Path) -> Result<Value, String> {
    let (offer, facets) = card_offer(root)?;
    let (state, facet) = match &offer {
        CardOffer::Chosen(facet) => ("chosen", Some(facet.as_str())),
        CardOffer::Declined => ("declined", None),
        CardOffer::AskExisting(facet) => ("ask_existing", Some(facet.as_str())),
        CardOffer::AskMakeBody => ("ask_make_body", Some(PREFERRED_BODY_FACET)),
        CardOffer::AskUnmute => ("ask_unmute", Some(PREFERRED_BODY_FACET)),
    };
    Ok(json!({
        "offer": {"state": state, "facet": facet},
        "facets": facets
            .into_iter()
            .map(|(name, title)| json!({"name": name, "title": title}))
            .collect::<Vec<_>>(),
        "copy": copy(),
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

/// `GET /app/import/api/body-facet`: what the Strava card asks or remembers.
pub(crate) async fn offer(State(state): State<AppState>) -> Response {
    let root = state.root.clone();
    match tokio::task::spawn_blocking(move || body(&root)).await {
        Ok(Ok(body)) => json_response(StatusCode::OK, body),
        Ok(Err(error)) => failed(StatusCode::INTERNAL_SERVER_ERROR, error),
        Err(error) => failed(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// `POST /app/import/api/body-facet` with `{"answer": "yes" | "no"}`: record
/// the owner's answer to the card's question.
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
    let root = state.root.clone();
    match tokio::task::spawn_blocking(move || record_answer(&root, yes).and_then(|()| body(&root)))
        .await
    {
        Ok(Ok(body)) => json_response(StatusCode::OK, body),
        Ok(Err(error)) => failed(StatusCode::CONFLICT, error),
        Err(error) => failed(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

fn record_answer(root: &Path, yes: bool) -> Result<(), String> {
    let (offer, _) = card_offer(root)?;
    let choice = match (offer, yes) {
        // Already answered: the card shows the remembered facet, and a repeat
        // answer changes nothing.
        (CardOffer::Chosen(_) | CardOffer::Declined, _) => return Ok(()),
        (_, false) => BodyFacetChoice::None,
        (CardOffer::AskExisting(facet), true) => BodyFacetChoice::Facet(facet),
        (CardOffer::AskUnmute, true) => {
            solstone_core_facets::set_facet_muted(root, PREFERRED_BODY_FACET, false)
                .map_err(|error| error.to_string())?;
            log_facet_action(
                root,
                PREFERRED_BODY_FACET,
                "facet_unmute",
                json!({"muted": false}),
            )?;
            BodyFacetChoice::Facet(PREFERRED_BODY_FACET.to_owned())
        }
        (CardOffer::AskMakeBody, true) => BodyFacetChoice::Facet(make_body(root)?),
    };
    solstone_core_journal_config_write::record_body_facet(root, &choice)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

const BODY_TITLE: &str = "Body";
const BODY_COLOR: &str = "#667eea";
const BODY_EMOJI: &str = "📦";

/// Make the `body` facet, or the next free name when `body` belonged to a
/// facet that was deleted or merged, as the settings page does.
fn make_body(root: &Path) -> Result<String, String> {
    let name = match solstone_core_facets::retired_facet_entry(root, PREFERRED_BODY_FACET)
        .map_err(|error| error.to_string())?
    {
        None => PREFERRED_BODY_FACET.to_owned(),
        Some(_) => solstone_core_facets::first_free_facet_name(root, PREFERRED_BODY_FACET)
            .map_err(|error| error.to_string())?,
    };
    solstone_core_facets::create_facet(root, &name, BODY_TITLE, "", BODY_COLOR, BODY_EMOJI, None)
        .map_err(|error| error.to_string())?;
    log_facet_action(
        root,
        &name,
        "facet_create",
        json!({"title": BODY_TITLE, "emoji": BODY_EMOJI, "color": BODY_COLOR, "description": ""}),
    )?;
    Ok(name)
}

fn log_facet_action(root: &Path, facet: &str, action: &str, params: Value) -> Result<(), String> {
    solstone_core_facets::append_action_log(root, Some(facet), "import", "owner", action, params)
        .map(|_| ())
        .map_err(|error| error.to_string())
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
