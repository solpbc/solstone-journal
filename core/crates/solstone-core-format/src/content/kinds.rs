// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Content kinds are descriptive. Only entries with `permission` are grantable.

use std::path::Path;

use super::{AdmittedCategory, IndexDisposition, KindAssignment, body_material, file_spec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Transcripts,
    Notes,
    Documents,
    Screen,
    Body,
    Calendar,
    Places,
    Photos,
    Video,
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
    pub label: &'static str,
    pub description: &'static str,
    pub checked_by_default: bool,
}

// Permission order is also the existing consent/default order.
pub const KINDS: &[KindSpec] = &[
    KindSpec {
        kind: Kind::Transcripts,
        stored_word: "transcripts",
        permission: Some(AdmittedCategory::Transcripts),
        query_alias: Some("Transcripts"),
        label: "transcripts",
        description: "words spoken or typed in conversations, and details drawn from those words.",
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::Entities,
        stored_word: "entities",
        permission: Some(AdmittedCategory::Entities),
        query_alias: Some("Entities"),
        label: "entities",
        description: "people, places and projects in your journal, and what it has noted about them.",
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::Facets,
        stored_word: "facets",
        permission: Some(AdmittedCategory::Facets),
        query_alias: Some("Facets"),
        label: "summaries",
        description: "facet descriptions, activities, facet events and summaries made from other kinds in your journal. reading the original details requires their own permissions.",
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::Screen,
        stored_word: "screen_text",
        permission: Some(AdmittedCategory::ScreenText),
        query_alias: Some("ScreenText"),
        label: "screen text",
        description: "text and summaries from your screens and terminal, with times when available. visible text can include passwords or other secrets. your redaction rules do not apply to that text.",
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::Body,
        stored_word: "body",
        permission: Some(AdmittedCategory::Body),
        query_alias: Some("Body"),
        label: "body",
        description: "readings, sleep, workouts, mindfulness sessions and what you logged, such as food or symptoms, imported from apps and devices like Apple Health, Oura or Strava, with their values and any notes you wrote on them.",
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::Notes,
        stored_word: "notes",
        permission: Some(AdmittedCategory::Notes),
        query_alias: Some("Notes"),
        label: "notes",
        description: "notes you import, highlights and notes saved by other agents.",
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Documents,
        stored_word: "documents",
        permission: Some(AdmittedCategory::Documents),
        query_alias: Some("Documents"),
        label: "documents",
        description: "text from imported documents such as PDFs and Word files.",
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Calendar,
        stored_word: "calendar",
        permission: Some(AdmittedCategory::Calendar),
        query_alias: Some("Calendar"),
        label: "calendar",
        description: "events from calendars you've connected or imported.",
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Places,
        stored_word: "places",
        permission: Some(AdmittedCategory::Places),
        query_alias: Some("Places"),
        label: "places",
        description: "place names and times from your journal's location history.",
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Photos,
        stored_word: "photos",
        permission: Some(AdmittedCategory::Photos),
        query_alias: Some("Photos"),
        label: "photos",
        description: "text descriptions of photos and screenshots you import as images.",
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Video,
        stored_word: "video",
        permission: None,
        query_alias: None,
        label: "video",
        description: "",
        checked_by_default: false,
    },
];

pub fn permission_kinds() -> impl Iterator<Item = &'static KindSpec> {
    KINDS.iter().filter(|spec| spec.permission.is_some())
}

pub fn default_permission_words() -> impl Iterator<Item = &'static str> {
    permission_kinds()
        .filter(|spec| spec.checked_by_default)
        .map(|spec| spec.stored_word)
}

pub fn permission_words() -> impl Iterator<Item = &'static str> {
    permission_kinds().map(|spec| spec.stored_word)
}

impl AdmittedCategory {
    pub fn as_str(self) -> &'static str {
        self.kind_spec().stored_word
    }

    pub fn kind_spec(self) -> &'static KindSpec {
        permission_kinds()
            .find(|spec| spec.permission == Some(self))
            .expect("every permission has a kind")
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

impl Kind {
    pub fn permission(self) -> Option<AdmittedCategory> {
        KINDS.iter().find(|spec| spec.kind == self)?.permission
    }
}

/// Effective permission for material that already has an index admission.
pub fn indexed_permission(
    journal: Option<&Path>,
    path: &str,
    stored: AdmittedCategory,
) -> AdmittedCategory {
    if body_material::is_body_path(path) {
        return AdmittedCategory::Body;
    }
    if super::screen_text::is_screen_text_path(path) {
        return AdmittedCategory::ScreenText;
    }
    if additional_indexed_permission(path).is_some() {
        return AdmittedCategory::Facets;
    }
    let Some(spec) = file_spec(path) else {
        return stored;
    };
    if !matches!(
        spec.disposition,
        IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            ..
        }
    ) {
        return stored;
    }
    match spec.kind {
        KindAssignment::Settled(kind) | KindAssignment::Legacy(kind) => {
            kind.permission().unwrap_or(stored)
        }
        KindAssignment::ImportSummary => journal
            .and_then(|journal| body_material::import_summary_source(journal, path))
            .map(|source| match source.as_str() {
                "apple_health" | "oura" | "strava" => AdmittedCategory::Body,
                "obsidian" | "evernote" | "kindle" | "text" => AdmittedCategory::Notes,
                "document" => AdmittedCategory::Documents,
                "ics" => AdmittedCategory::Calendar,
                "image" => AdmittedCategory::Photos,
                _ => stored,
            })
            .unwrap_or(stored),
        KindAssignment::NotContent => stored,
    }
}

/// The weekly reflection includes verbatim passages from transcripts.
pub fn additional_indexed_permission(path: &str) -> Option<AdmittedCategory> {
    path.strip_prefix("reflections/weekly/")
        .filter(|name| name.ends_with(".md") && !name.contains('/'))
        .map(|_| AdmittedCategory::Transcripts)
}

/// The segment reader calls this only for a file already eligible on that
/// route. A producer-marked speech file can have an arbitrary basename.
pub fn segment_file_permission(path: &str) -> AdmittedCategory {
    if body_material::is_body_path(path)
        || path.ends_with("/body.jsonl")
        || path.ends_with("/workout.json")
    {
        return AdmittedCategory::Body;
    }
    if path.ends_with(".jsonl") {
        return AdmittedCategory::Transcripts;
    }
    match path.rsplit('/').next().unwrap_or(path) {
        "note_transcript.md" => AdmittedCategory::Notes,
        "document_transcript.md" => AdmittedCategory::Documents,
        "event_transcript.md" => AdmittedCategory::Calendar,
        "image_transcript.md" => AdmittedCategory::Photos,
        _ => indexed_permission(None, path, AdmittedCategory::Transcripts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_the_expected_stored_permission() {
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
        assert_eq!(
            KINDS
                .iter()
                .map(|spec| (spec.kind, spec.permission.map(AdmittedCategory::as_str)))
                .collect::<Vec<_>>(),
            [
                (Kind::Transcripts, Some("transcripts")),
                (Kind::Entities, Some("entities")),
                (Kind::Facets, Some("facets")),
                (Kind::Screen, Some("screen_text")),
                (Kind::Body, Some("body")),
                (Kind::Notes, Some("notes")),
                (Kind::Documents, Some("documents")),
                (Kind::Calendar, Some("calendar")),
                (Kind::Places, Some("places")),
                (Kind::Photos, Some("photos")),
                (Kind::Video, None),
            ]
        );
        assert_eq!(
            default_permission_words().collect::<Vec<_>>(),
            ["transcripts", "entities", "facets", "screen_text", "body"]
        );
        for word in ["video", "Video", "BODY", "Screen", "screen", "unknown"] {
            assert_eq!(AdmittedCategory::from_stored_word(word), None);
            assert_eq!(AdmittedCategory::from_query_token(word), None);
        }
    }
}
