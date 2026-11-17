// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Content kinds are descriptive. Only entries with `permission` are grantable.

use super::AdmittedCategory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Transcripts,
    Documents,
    Screen,
    Body,
    Calendar,
    Places,
    Photos,
    Facets,
    Entities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindSpec {
    pub kind: Kind,
    pub stored_word: &'static str,
    pub permission: Option<AdmittedCategory>,
    /// The capitalized spelling accepted by the query boundary, and nowhere else.
    pub query_alias: Option<&'static str>,
}

// Permission order is also the existing consent/default order.
pub const KINDS: &[KindSpec] = &[
    KindSpec {
        kind: Kind::Transcripts,
        stored_word: "transcripts",
        permission: Some(AdmittedCategory::Transcripts),
        query_alias: Some("Transcripts"),
    },
    KindSpec {
        kind: Kind::Entities,
        stored_word: "entities",
        permission: Some(AdmittedCategory::Entities),
        query_alias: Some("Entities"),
    },
    KindSpec {
        kind: Kind::Facets,
        stored_word: "facets",
        permission: Some(AdmittedCategory::Facets),
        query_alias: Some("Facets"),
    },
    KindSpec {
        kind: Kind::Screen,
        stored_word: "screen_text",
        permission: Some(AdmittedCategory::ScreenText),
        query_alias: Some("ScreenText"),
    },
    KindSpec {
        kind: Kind::Body,
        stored_word: "body",
        permission: Some(AdmittedCategory::Body),
        query_alias: Some("Body"),
    },
    KindSpec {
        kind: Kind::Documents,
        stored_word: "documents",
        permission: None,
        query_alias: None,
    },
    KindSpec {
        kind: Kind::Calendar,
        stored_word: "calendar",
        permission: None,
        query_alias: None,
    },
    KindSpec {
        kind: Kind::Places,
        stored_word: "places",
        permission: None,
        query_alias: None,
    },
    KindSpec {
        kind: Kind::Photos,
        stored_word: "photos",
        permission: None,
        query_alias: None,
    },
];

pub fn permission_words() -> impl Iterator<Item = &'static str> {
    KINDS
        .iter()
        .filter(|spec| spec.permission.is_some())
        .map(|spec| spec.stored_word)
}

impl AdmittedCategory {
    pub fn as_str(self) -> &'static str {
        KINDS
            .iter()
            .find(|spec| spec.permission == Some(self))
            .expect("every permission has a kind")
            .stored_word
    }

    pub fn from_stored_word(word: &str) -> Option<Self> {
        KINDS
            .iter()
            .find(|spec| spec.stored_word == word)
            .and_then(|spec| spec.permission)
    }

    pub fn from_query_token(word: &str) -> Option<Self> {
        KINDS
            .iter()
            .find(|spec| spec.stored_word == word || spec.query_alias == Some(word))
            .and_then(|spec| spec.permission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_existing_permissions_and_query_aliases_parse() {
        for spec in KINDS {
            assert_eq!(
                AdmittedCategory::from_stored_word(spec.stored_word),
                spec.permission
            );
            if let Some(permission) = spec.permission {
                assert_eq!(permission.as_str(), spec.stored_word);
                assert_eq!(
                    AdmittedCategory::from_query_token(spec.query_alias.unwrap()),
                    Some(permission)
                );
                assert_eq!(
                    AdmittedCategory::from_stored_word(spec.query_alias.unwrap()),
                    None
                );
            }
        }
        for (alias, expected) in [
            ("Transcripts", AdmittedCategory::Transcripts),
            ("Entities", AdmittedCategory::Entities),
            ("Facets", AdmittedCategory::Facets),
            ("ScreenText", AdmittedCategory::ScreenText),
            ("Body", AdmittedCategory::Body),
        ] {
            assert_eq!(AdmittedCategory::from_query_token(alias), Some(expected));
            assert_eq!(AdmittedCategory::from_stored_word(alias), None);
        }
        for word in [
            "documents",
            "calendar",
            "places",
            "photos",
            "BODY",
            "Screen",
            "screen",
            "unknown",
        ] {
            assert_eq!(AdmittedCategory::from_stored_word(word), None);
            assert_eq!(AdmittedCategory::from_query_token(word), None);
        }
        assert_eq!(
            permission_words().collect::<Vec<_>>(),
            ["transcripts", "entities", "facets", "screen_text", "body"]
        );
    }

    #[test]
    fn agents_picker_permission_arrays_match_the_registry() {
        let html = include_str!("../../../solstone-core-convey-shell/assets/agents/workspace.html");
        let picker = html
            .split("function picker(connection)")
            .nth(1)
            .unwrap()
            .split("function ")
            .next()
            .unwrap();
        let arrays = picker
            .split('[')
            .skip(1)
            .filter_map(|rest| {
                let array = rest.split(']').next()?;
                array.starts_with("'transcripts'").then_some(array)
            })
            .collect::<Vec<_>>();
        assert_eq!(arrays.len(), 2, "default and editor option arrays");
        for array in arrays {
            assert_eq!(
                array
                    .split(',')
                    .map(|word| word.trim().trim_matches('\''))
                    .collect::<Vec<_>>(),
                permission_words().collect::<Vec<_>>()
            );
        }
    }
}
