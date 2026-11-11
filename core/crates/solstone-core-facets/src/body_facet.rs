// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a body import offers for its facet, from the owner's remembered
//! default and the journal's declared facets, and what an answer does. The
//! answer itself is recorded by `record_body_facet` in the config write crate.

use std::error::Error;
use std::fmt;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_journal_config::{
    BodyFacetChoice, BodyFacetOffer, ConfigLoadError, PREFERRED_BODY_FACET, StoredBodyFacet,
    parse_body_facet, read_journal_config,
};

use crate::{FacetStoreError, observe_declared_facet_inventory};

/// The offer, and whether a stored value had to be ignored to make it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyFacetOfferRead {
    pub offer: BodyFacetOffer,
    /// What was wrong with a stored value that is present but not valid. It
    /// was read as never answered, and the file was left as it is.
    pub malformed: Option<String>,
}

/// The configuration or the facet declarations could not be read.
#[derive(Debug)]
pub enum BodyFacetReadError {
    Config(ConfigLoadError),
    Facets(FacetStoreError),
}

impl fmt::Display for BodyFacetReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Facets(error) => error.fmt(formatter),
        }
    }
}

impl Error for BodyFacetReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Facets(error) => Some(error),
        }
    }
}

/// What a body import asks an owner, or what is already remembered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyFacetAsk {
    Chosen(String),
    Declined,
    AskExisting(String),
    AskMakeBody,
    /// A `body` facet exists but is muted: offer to turn it back on.
    AskUnmute,
}

/// What to ask, and the enabled facets by name and title.
///
/// # Errors
///
/// The configuration or the facet declarations can't be read.
pub fn body_facet_ask(root: &Path) -> Result<(BodyFacetAsk, Vec<(String, String)>), String> {
    let read = body_facet_offer(root).map_err(|error| error.to_string())?;
    let inventory = observe_declared_facet_inventory(root).map_err(|error| error.to_string())?;
    let offer = match read.offer {
        BodyFacetOffer::Chosen(facet) => BodyFacetAsk::Chosen(facet),
        BodyFacetOffer::Declined => BodyFacetAsk::Declined,
        BodyFacetOffer::AskExisting(facet) => BodyFacetAsk::AskExisting(facet),
        BodyFacetOffer::AskMakeBody
            if inventory
                .muted
                .iter()
                .any(|facet| facet == PREFERRED_BODY_FACET) =>
        {
            BodyFacetAsk::AskUnmute
        }
        BodyFacetOffer::AskMakeBody => BodyFacetAsk::AskMakeBody,
    };
    let facets = inventory
        .enabled
        .iter()
        .map(|facet| {
            let title = crate::read_facet_declaration(root, facet)
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

const BODY_TITLE: &str = "Body";
const BODY_COLOR: &str = "#667eea";
const BODY_EMOJI: &str = "\u{1f4e6}";

/// Do what an answer to `ask` means, making or unmuting the facet a yes names,
/// and return what to remember. `None`: it was already answered.
///
/// # Errors
///
/// The facet can't be made, unmuted or logged.
pub fn apply_body_facet_answer(
    root: &Path,
    ask: &BodyFacetAsk,
    yes: bool,
) -> Result<Option<BodyFacetChoice>, String> {
    Ok(Some(match (ask, yes) {
        (BodyFacetAsk::Chosen(_) | BodyFacetAsk::Declined, _) => return Ok(None),
        (_, false) => BodyFacetChoice::None,
        (BodyFacetAsk::AskExisting(facet), true) => BodyFacetChoice::Facet(facet.clone()),
        (BodyFacetAsk::AskUnmute, true) => {
            crate::set_facet_muted(root, PREFERRED_BODY_FACET, false)
                .map_err(|error| error.to_string())?;
            log_facet_action(
                root,
                PREFERRED_BODY_FACET,
                "facet_unmute",
                json!({"muted": false}),
            )?;
            BodyFacetChoice::Facet(PREFERRED_BODY_FACET.to_owned())
        }
        (BodyFacetAsk::AskMakeBody, true) => BodyFacetChoice::Facet(make_body(root)?),
    }))
}

/// Make the `body` facet, or the next free name when `body` belonged to a
/// facet that was merged or renamed away, as the settings page does.
fn make_body(root: &Path) -> Result<String, String> {
    let name = match crate::retired_facet_entry(root, PREFERRED_BODY_FACET)
        .map_err(|error| error.to_string())?
        .filter(|entry| entry.redirects())
    {
        None => PREFERRED_BODY_FACET.to_owned(),
        Some(_) => crate::first_free_facet_name(root, PREFERRED_BODY_FACET)
            .map_err(|error| error.to_string())?,
    };
    crate::create_facet(root, &name, BODY_TITLE, "", BODY_COLOR, BODY_EMOJI, None)
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
    crate::append_action_log(root, Some(facet), "import", "owner", action, params)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// What a body import with the owner present offers for its facet.
pub fn body_facet_offer(journal: &Path) -> Result<BodyFacetOfferRead, BodyFacetReadError> {
    let (stored, enabled) = read_body_facet_basis(journal)?;
    Ok(BodyFacetOfferRead {
        offer: stored.offer(&enabled),
        malformed: stored.malformed().map(str::to_owned),
    })
}

/// The facet a body import with nobody to answer uses: the remembered facet
/// while it is still enabled, otherwise none. Never creates a facet.
pub fn default_for_unattended(journal: &Path) -> Result<Option<String>, BodyFacetReadError> {
    let (stored, enabled) = read_body_facet_basis(journal)?;
    Ok(stored.unattended_default(&enabled))
}

fn read_body_facet_basis(
    journal: &Path,
) -> Result<(StoredBodyFacet, Vec<String>), BodyFacetReadError> {
    let config = read_journal_config(journal)
        .map_err(BodyFacetReadError::Config)?
        .config
        .unwrap_or_default();
    let stored = parse_body_facet(&config);
    if let Some(detail) = stored.malformed() {
        log::warn!("body import facet: ignoring the stored default: {detail}");
    }
    let enabled = observe_declared_facet_inventory(journal)
        .map_err(BodyFacetReadError::Facets)?
        .enabled;
    Ok((stored, enabled))
}
