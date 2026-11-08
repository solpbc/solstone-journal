// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's default facet for body imports, stored at
//! `config/journal.json` → `imports.body_facet`.
//!
//! The stored value is either `{"choice": "facet", "facet": "<name>"}` or
//! `{"choice": "none"}`. This module validates it and decides what an import
//! offers from it and the journal's enabled facets. It never writes: the one
//! writer is `record_body_facet` in the config write crate, and it records
//! only the owner's answer to the default question. A facet picked for one
//! import is that import's argument, never this value.

use serde_json::{Map, Value, json};

use crate::is_path_shaped_name;

/// Top-level `config/journal.json` section holding import preferences.
pub const IMPORTS_SECTION: &str = "imports";
/// Key under [`IMPORTS_SECTION`] holding the remembered default facet.
pub const BODY_FACET_KEY: &str = "body_facet";
/// The facet an import offers first, and offers to create when none fits.
pub const PREFERRED_BODY_FACET: &str = "body";
/// The existing facet an import offers when there is no `body` facet.
pub const FALLBACK_BODY_FACET: &str = "health";

/// The owner's answer to "which facet should body imports go in by default".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyFacetChoice {
    /// File body imports under this facet.
    Facet(String),
    /// The owner said no: body imports get no facet by default.
    None,
}

/// What `imports.body_facet` holds, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredBodyFacet {
    /// No value: the owner has never answered.
    Unanswered,
    /// A valid stored answer.
    Answered(BodyFacetChoice),
    /// A value is present but is not a valid answer. It is never treated as
    /// a choice; the detail says what is wrong so a caller can report it.
    Malformed(String),
}

/// What an import offers for its default facet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyFacetOffer {
    /// The remembered facet, still enabled.
    Chosen(String),
    /// The owner said no.
    Declined,
    /// Not answered, or the remembered facet is muted or gone, and this
    /// enabled facet (`body` before `health`) is the one to offer.
    AskExisting(String),
    /// Not answered, or the remembered facet is muted or gone, and neither a
    /// `body` nor a `health` facet is enabled: offer to make `body`.
    AskMakeBody,
}

/// Validate `imports.body_facet` from a strictly read configuration map.
pub fn parse_body_facet(config: &Map<String, Value>) -> StoredBodyFacet {
    let Some(imports) = config.get(IMPORTS_SECTION) else {
        return StoredBodyFacet::Unanswered;
    };
    let Some(imports) = imports.as_object() else {
        return StoredBodyFacet::Malformed(format!("{IMPORTS_SECTION} is not an object"));
    };
    let Some(value) = imports.get(BODY_FACET_KEY) else {
        return StoredBodyFacet::Unanswered;
    };
    match parse_choice(value) {
        Ok(choice) => StoredBodyFacet::Answered(choice),
        Err(detail) => {
            StoredBodyFacet::Malformed(format!("{IMPORTS_SECTION}.{BODY_FACET_KEY} {detail}"))
        }
    }
}

fn parse_choice(value: &Value) -> Result<BodyFacetChoice, &'static str> {
    let object = value.as_object().ok_or("is not an object")?;
    match object.get("choice").and_then(Value::as_str) {
        Some("none") if object.len() == 1 => Ok(BodyFacetChoice::None),
        Some("none") => Err("with choice none carries other fields"),
        Some("facet") if object.len() == 2 => {
            let facet = object
                .get("facet")
                .and_then(Value::as_str)
                .ok_or("with choice facet has no facet name")?;
            if is_body_facet_name(facet) {
                Ok(BodyFacetChoice::Facet(facet.to_owned()))
            } else {
                Err("names a facet that is not a facet name")
            }
        }
        Some("facet") => Err("with choice facet must hold exactly choice and facet"),
        _ => Err("has no choice of facet or none"),
    }
}

/// Whether `name` can be stored as a remembered facet: one non-empty path
/// component with no surrounding whitespace.
pub fn is_body_facet_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name != "."
        && name != ".."
        && !is_path_shaped_name(name)
        && !name.chars().any(char::is_control)
}

/// The stored JSON form of a choice.
pub fn body_facet_value(choice: &BodyFacetChoice) -> Value {
    match choice {
        BodyFacetChoice::Facet(facet) => json!({"choice": "facet", "facet": facet}),
        BodyFacetChoice::None => json!({"choice": "none"}),
    }
}

impl StoredBodyFacet {
    /// Decide the offer from this value and the journal's enabled facet names.
    pub fn offer(&self, enabled: &[String]) -> BodyFacetOffer {
        match self {
            Self::Answered(BodyFacetChoice::None) => BodyFacetOffer::Declined,
            Self::Answered(BodyFacetChoice::Facet(facet)) if is_enabled(enabled, facet) => {
                BodyFacetOffer::Chosen(facet.clone())
            }
            _ => ask(enabled),
        }
    }

    /// The facet an import with nobody to answer uses: the remembered facet
    /// while it is still enabled, otherwise none.
    pub fn unattended_default(&self, enabled: &[String]) -> Option<String> {
        match self {
            Self::Answered(BodyFacetChoice::Facet(facet)) if is_enabled(enabled, facet) => {
                Some(facet.clone())
            }
            _ => None,
        }
    }

    /// The malformed detail, when the stored value is malformed.
    pub fn malformed(&self) -> Option<&str> {
        match self {
            Self::Malformed(detail) => Some(detail),
            _ => None,
        }
    }
}

fn is_enabled(enabled: &[String], facet: &str) -> bool {
    enabled.iter().any(|name| name == facet)
}

fn ask(enabled: &[String]) -> BodyFacetOffer {
    [PREFERRED_BODY_FACET, FALLBACK_BODY_FACET]
        .into_iter()
        .find(|facet| is_enabled(enabled, facet))
        .map_or(BodyFacetOffer::AskMakeBody, |facet| {
            BodyFacetOffer::AskExisting(facet.to_owned())
        })
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::*;

    fn config(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn facet(name: &str) -> StoredBodyFacet {
        StoredBodyFacet::Answered(BodyFacetChoice::Facet(name.to_owned()))
    }

    #[test]
    fn valid_values_parse_to_their_choice() {
        assert_eq!(
            parse_body_facet(&config(json!({}))),
            StoredBodyFacet::Unanswered
        );
        assert_eq!(
            parse_body_facet(&config(json!({"imports": {"other": 1}}))),
            StoredBodyFacet::Unanswered
        );
        assert_eq!(
            parse_body_facet(&config(
                json!({"imports": {"body_facet": {"choice": "facet", "facet": "body"}}})
            )),
            facet("body")
        );
        assert_eq!(
            parse_body_facet(&config(
                json!({"imports": {"body_facet": {"choice": "none"}}})
            )),
            StoredBodyFacet::Answered(BodyFacetChoice::None)
        );
    }

    #[test]
    fn malformed_values_are_never_a_choice() {
        for value in [
            json!({"imports": 5}),
            json!({"imports": {"body_facet": "body"}}),
            json!({"imports": {"body_facet": null}}),
            json!({"imports": {"body_facet": {}}}),
            json!({"imports": {"body_facet": {"choice": "maybe"}}}),
            json!({"imports": {"body_facet": {"choice": "facet"}}}),
            json!({"imports": {"body_facet": {"choice": "facet", "facet": 5}}}),
            json!({"imports": {"body_facet": {"choice": "facet", "facet": ""}}}),
            json!({"imports": {"body_facet": {"choice": "facet", "facet": " body"}}}),
            json!({"imports": {"body_facet": {"choice": "facet", "facet": "a/b"}}}),
            json!({"imports": {"body_facet": {"choice": "facet", "facet": ".."}}}),
            json!({"imports": {"body_facet": {"choice": "facet", "facet": "body", "x": 1}}}),
            json!({"imports": {"body_facet": {"choice": "none", "facet": "body"}}}),
        ] {
            let stored = parse_body_facet(&config(value.clone()));
            assert!(stored.malformed().is_some(), "{value}: {stored:?}");
            let enabled = names(&["body", "work"]);
            assert_eq!(
                stored.offer(&enabled),
                BodyFacetOffer::AskExisting("body".to_owned()),
                "{value}"
            );
            assert_eq!(stored.offer(&names(&["work"])), BodyFacetOffer::AskMakeBody);
            assert_eq!(stored.unattended_default(&enabled), None, "{value}");
        }
    }

    #[test]
    fn every_offer_state_follows_the_stored_value_and_enabled_facets() {
        let enabled = names(&["body", "health", "work"]);
        assert_eq!(
            facet("work").offer(&enabled),
            BodyFacetOffer::Chosen("work".to_owned())
        );
        assert_eq!(
            StoredBodyFacet::Answered(BodyFacetChoice::None).offer(&enabled),
            BodyFacetOffer::Declined
        );
        assert_eq!(
            StoredBodyFacet::Unanswered.offer(&enabled),
            BodyFacetOffer::AskExisting("body".to_owned())
        );
        assert_eq!(
            StoredBodyFacet::Unanswered.offer(&names(&["health", "work"])),
            BodyFacetOffer::AskExisting("health".to_owned())
        );
        assert_eq!(
            StoredBodyFacet::Unanswered.offer(&names(&["work"])),
            BodyFacetOffer::AskMakeBody
        );
        assert_eq!(
            facet("gone").offer(&enabled),
            BodyFacetOffer::AskExisting("body".to_owned())
        );
        assert_eq!(
            facet("gone").offer(&names(&["work"])),
            BodyFacetOffer::AskMakeBody
        );
    }

    #[test]
    fn unattended_default_is_only_an_enabled_remembered_facet() {
        let enabled = names(&["body", "work"]);
        assert_eq!(
            facet("work").unattended_default(&enabled),
            Some("work".to_owned())
        );
        assert_eq!(facet("gone").unattended_default(&enabled), None);
        assert_eq!(
            StoredBodyFacet::Answered(BodyFacetChoice::None).unattended_default(&enabled),
            None
        );
        assert_eq!(
            StoredBodyFacet::Unanswered.unattended_default(&enabled),
            None
        );
    }

    #[test]
    fn stored_form_parses_back_to_the_same_choice() {
        for choice in [
            BodyFacetChoice::Facet("body".to_owned()),
            BodyFacetChoice::None,
        ] {
            let value = json!({"imports": {"body_facet": body_facet_value(&choice)}});
            assert_eq!(
                parse_body_facet(&config(value)),
                StoredBodyFacet::Answered(choice)
            );
        }
    }
}
