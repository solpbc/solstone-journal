// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Content kinds are descriptive. Only entries with `permission` are grantable.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    AdmittedCategory, ContentUnit, Family, FileFormat, FilePattern, IndexDisposition,
    KindAssignment, body_material, file_spec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Transcripts,
    Messages,
    Transactions,
    Notes,
    Documents,
    Screen,
    Body,
    WomensHealth,
    Calendar,
    Places,
    Photos,
    Video,
    Facets,
    Entities,
    ProfilePics,
    Screenshots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindSpec {
    pub kind: Kind,
    pub stored_word: &'static str,
    pub permission: Option<AdmittedCategory>,
    /// The capitalized spelling accepted by the query boundary, and nowhere else.
    pub query_alias: Option<&'static str>,
    pub permission_copy: Option<PermissionCopy>,
    pub checked_by_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionCopy {
    pub label_id: &'static str,
    pub description_id: &'static str,
    pub fallback_label: Option<&'static str>,
    pub fallback_description: Option<&'static str>,
}

impl PermissionCopy {
    pub const fn catalog(label_id: &'static str, description_id: &'static str) -> Self {
        Self {
            label_id,
            description_id,
            fallback_label: None,
            fallback_description: None,
        }
    }

    pub const fn with_fallback(
        label_id: &'static str,
        description_id: &'static str,
        fallback_label: &'static str,
        fallback_description: &'static str,
    ) -> Self {
        Self {
            label_id,
            description_id,
            fallback_label: Some(fallback_label),
            fallback_description: Some(fallback_description),
        }
    }

    pub fn render_label(&self, lookup: impl FnOnce(&str) -> String) -> String {
        match self.fallback_label {
            Some(label) => label.to_owned(),
            None => lookup(self.label_id),
        }
    }

    pub fn render_description(&self, lookup: impl FnOnce(&str) -> String) -> String {
        match self.fallback_description {
            Some(description) => description.to_owned(),
            None => lookup(self.description_id),
        }
    }
}

// Permission order is also the existing consent/default order.
pub const KINDS: &[KindSpec] = &[
    KindSpec {
        kind: Kind::Transcripts,
        stored_word: "transcripts",
        permission: Some(AdmittedCategory::Transcripts),
        query_alias: Some("Transcripts"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.transcripts.label",
            "journal.window.agents.permission.kind.transcripts.description",
        )),
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::Messages,
        stored_word: "messages",
        permission: Some(AdmittedCategory::Messages),
        query_alias: Some("Messages"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.messages.label",
            "journal.window.agents.permission.kind.messages.description",
        )),
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Transactions,
        stored_word: "transactions",
        permission: Some(AdmittedCategory::Transactions),
        query_alias: Some("Transactions"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.transactions.label",
            "journal.window.agents.permission.kind.transactions.description",
        )),
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Entities,
        stored_word: "entities",
        permission: Some(AdmittedCategory::Entities),
        query_alias: Some("Entities"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.entities.label",
            "journal.window.agents.permission.kind.entities.description",
        )),
        checked_by_default: true,
    },
    // An option of entities: the picture the journal shows for one. It reads
    // only with entities. ⚠ A grant that holds entities and predates this row
    // keeps profile pics: only `NO_PROFILE_PICS`, written
    // where the owner chooses, takes them away. See `profile_pics_admitted`.
    KindSpec {
        kind: Kind::ProfilePics,
        stored_word: "profile_pics",
        permission: Some(AdmittedCategory::ProfilePics),
        query_alias: None,
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.profile.pics.label",
            "journal.window.agents.permission.kind.profile.pics.description",
        )),
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::Facets,
        stored_word: "facets",
        permission: Some(AdmittedCategory::Facets),
        query_alias: Some("Facets"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.facets.label",
            "journal.window.agents.permission.kind.facets.description",
        )),
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::Screen,
        stored_word: "screen_text",
        permission: Some(AdmittedCategory::ScreenText),
        query_alias: Some("ScreenText"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.screen.text.label",
            "journal.window.agents.permission.kind.screen.text.description",
        )),
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::Screenshots,
        stored_word: "screenshots",
        permission: Some(AdmittedCategory::Screenshots),
        query_alias: None,
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.screenshots.label",
            "journal.window.agents.permission.kind.screenshots.description",
        )),
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Body,
        stored_word: "body",
        permission: Some(AdmittedCategory::Body),
        query_alias: Some("Body"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.body.label",
            "journal.window.agents.permission.kind.body.description",
        )),
        checked_by_default: true,
    },
    KindSpec {
        kind: Kind::WomensHealth,
        stored_word: "womens_health",
        permission: Some(AdmittedCategory::WomensHealth),
        query_alias: None,
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.womens.health.label",
            "journal.window.agents.permission.kind.womens.health.description",
        )),
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Notes,
        stored_word: "notes",
        permission: Some(AdmittedCategory::Notes),
        query_alias: Some("Notes"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.notes.label",
            "journal.window.agents.permission.kind.notes.description",
        )),
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Documents,
        stored_word: "documents",
        permission: Some(AdmittedCategory::Documents),
        query_alias: Some("Documents"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.documents.label",
            "journal.window.agents.permission.kind.documents.description",
        )),
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Calendar,
        stored_word: "calendar",
        permission: Some(AdmittedCategory::Calendar),
        query_alias: Some("Calendar"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.calendar.label",
            "journal.window.agents.permission.kind.calendar.description",
        )),
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Places,
        stored_word: "places",
        permission: Some(AdmittedCategory::Places),
        query_alias: Some("Places"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.places.label",
            "journal.window.agents.permission.kind.places.description",
        )),
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Photos,
        stored_word: "photos",
        permission: Some(AdmittedCategory::Photos),
        query_alias: Some("Photos"),
        permission_copy: Some(PermissionCopy::catalog(
            "journal.window.agents.permission.kind.photos.label",
            "journal.window.agents.permission.kind.photos.description",
        )),
        checked_by_default: false,
    },
    KindSpec {
        kind: Kind::Video,
        stored_word: "video",
        permission: None,
        query_alias: None,
        permission_copy: None,
        checked_by_default: false,
    },
];

/// The stored word an owner's choice writes when entities is allowed and
/// profile pics are not. ⛔ Not a kind and not a permission: it only takes
/// profile pics away from an entities grant.
pub const NO_PROFILE_PICS: &str = "no_profile_pics";

/// Record the owner's profile pics choice in the words they chose. Call it
/// only where the owner chooses (the consent page, the agents app, the CLI),
/// never on a rewrite of a stored grant, so a grant that predates the choice
/// keeps what it was approved with.
pub fn record_profile_pics_choice(words: &mut Vec<String>) {
    words.retain(|word| word != NO_PROFILE_PICS);
    let entities = words.iter().any(|word| word == "entities");
    let pics = words.iter().any(|word| word == "profile_pics");
    if entities && !pics {
        words.push(NO_PROFILE_PICS.to_owned());
    }
}

/// Whether stored grant words admit profile pics: chosen, or held with
/// entities by a grant the choice never reached.
pub fn profile_pics_admitted(words: &[String]) -> bool {
    let has = |wanted: &str| words.iter().any(|word| word == wanted);
    has("profile_pics") || (has("entities") && !has(NO_PROFILE_PICS))
}

/// Stored precision modifiers for the `places` permission.
/// ⛔ Not kinds and not standalone permissions: they sit inside
/// `ReadPermission.categories` alongside `"places"` so older runtimes
/// deserialize `permissions.json` cleanly and ignore unknown words.
pub const PLACES_PRECISION_FULL: &str = "places_full";
pub const PLACES_PRECISION_APPROX: &str = "places_approx";
pub const PLACES_PRECISION_NAMES: &str = "places_names";

/// How much spatial detail a connection with `places` may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacesPrecision {
    /// Place names, area names and times only — zero coordinates, zero cells.
    NamesOnly,
    /// H3 resolution-8 (~1 km) cells and cell centres — zero exact coordinates.
    Approx,
    /// Exact coordinates for named places, visits/stays and GPS fixes.
    Full,
}

impl PlacesPrecision {
    #[must_use]
    pub const fn stored_word(self) -> &'static str {
        match self {
            Self::NamesOnly => PLACES_PRECISION_NAMES,
            Self::Approx => PLACES_PRECISION_APPROX,
            Self::Full => PLACES_PRECISION_FULL,
        }
    }

    #[must_use]
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::NamesOnly => "names",
            Self::Approx => "approx",
            Self::Full => "full",
        }
    }

    #[must_use]
    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "approx" => Some(Self::Approx),
            "names" => Some(Self::NamesOnly),
            _ => None,
        }
    }
}

#[must_use]
pub fn is_places_precision_word(word: &str) -> bool {
    matches!(
        word,
        PLACES_PRECISION_FULL | PLACES_PRECISION_APPROX | PLACES_PRECISION_NAMES
    )
}

/// Record the owner's places precision choice in the words they chose. Call it
/// only where the owner chooses (the consent page, the agents app, the CLI),
/// never on a passive rewrite of a stored grant, so a grant that predates the
/// choice keeps its safe historical words-only meaning.
pub fn record_places_precision_choice(words: &mut Vec<String>, chosen: Option<PlacesPrecision>) {
    words.retain(|word| !is_places_precision_word(word));
    if words.iter().any(|word| word == "places") {
        words.push(
            chosen
                .unwrap_or(PlacesPrecision::Full)
                .stored_word()
                .to_owned(),
        );
    }
}

/// Effective location precision admitted by stored grant words.
///
/// - Without `"places"`, returns `None` even if stray precision words appear.
/// - Conflicting modifiers fail safer (`NamesOnly` > `Approx` > `Full`).
/// - A legacy bare `"places"` grant (no modifier) stays at `NamesOnly`.
#[must_use]
pub fn places_precision_admitted(words: &[String]) -> Option<PlacesPrecision> {
    let has = |wanted: &str| words.iter().any(|word| word == wanted);
    if !has("places") {
        return None;
    }
    if has(PLACES_PRECISION_NAMES) {
        Some(PlacesPrecision::NamesOnly)
    } else if has(PLACES_PRECISION_APPROX) {
        Some(PlacesPrecision::Approx)
    } else if has(PLACES_PRECISION_FULL) {
        Some(PlacesPrecision::Full)
    } else {
        Some(PlacesPrecision::NamesOnly)
    }
}

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

impl KindSpec {
    /// The kind this one is an option of, shown under it where an owner chooses.
    pub fn option_of(&self) -> Option<&'static str> {
        match self.kind {
            Kind::ProfilePics => Some("entities"),
            Kind::Screenshots => Some("screen_text"),
            Kind::WomensHealth => Some("body"),
            _ => None,
        }
    }
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

/// The item shown on the owner dashboard for a supported filed import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DashboardImportItem {
    pub kind: &'static str,
    pub source: &'static str,
}

impl DashboardImportItem {
    pub fn from_file(path: &str) -> Option<Self> {
        Self::from_spec(file_spec(path)?)
    }

    fn from_spec(spec: &FilePattern) -> Option<Self> {
        // Use the matched registry row, not the input stream, to select the
        // imports this view covers. A chat export can have the same kind and
        // file format as a manual conversation transcript.
        let lane = spec.pattern.split('/').nth(1)?;
        let KindAssignment::Settled(kind) = spec.kind else {
            return None;
        };
        if spec.unit != ContentUnit::Segment {
            return None;
        }
        let source = match (lane, kind, spec.format) {
            (
                "import.text",
                Kind::Notes | Kind::Documents,
                FileFormat::Content(Family::Markdown),
            )
            | ("import.text", Kind::Transcripts, FileFormat::Content(Family::AiChat))
            | ("import.image", Kind::Screen | Kind::Photos, _) => "imported",
            ("import.obsidian", Kind::Notes, _) => "obsidian",
            ("import.evernote", Kind::Notes, _) => "evernote",
            // The document importer writes this row for PDF imports.
            ("import.document", Kind::Documents, _) => "pdf",
            _ => return None,
        };
        Some(Self {
            kind: kind.permission()?.as_str(),
            source,
        })
    }

    /// A descriptive file is the primary identity. An original can supply it
    /// while the import has no description yet, but a conflicting description
    /// cannot be resolved by directory order.
    pub fn from_segment_files<'a>(
        segment: &str,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Option<Self> {
        let mut descriptions = Vec::new();
        let mut originals = Vec::new();
        for name in names {
            let path = format!("{segment}/{name}");
            let Some(spec) = file_spec(&path) else {
                continue;
            };
            let Some(item) = Self::from_spec(spec) else {
                continue;
            };
            if spec.format == FileFormat::Original {
                originals.push(item);
            } else {
                descriptions.push(item);
            }
        }
        let items = if descriptions.is_empty() {
            originals
        } else {
            descriptions
        };
        let item = *items.first()?;
        items.iter().all(|other| *other == item).then_some(item)
    }
}

fn permission_for_spec(
    journal: Option<&Path>,
    path: &str,
    spec: &FilePattern,
) -> Option<AdmittedCategory> {
    let IndexDisposition::Admitted { .. } = spec.disposition else {
        return None;
    };
    if super::conversation::unresolved_markdown(path) {
        return Some(AdmittedCategory::Transcripts);
    }
    if body_material::is_body_path(path) {
        return Some(AdmittedCategory::Body);
    }
    if super::screen_text::is_screen_text_path(path) {
        return Some(AdmittedCategory::ScreenText);
    }
    if additional_indexed_permission(path).is_some() {
        return Some(AdmittedCategory::Facets);
    }
    match spec.kind {
        KindAssignment::Settled(kind) | KindAssignment::Legacy(kind) => kind.permission(),
        KindAssignment::ImportSummary => import_summary_permission(journal, path),
        KindAssignment::NotContent => None,
    }
}

/// Effective permission for material that already has an index admission.
///
/// Legacy non-agent fallback when an already-eligible path does not settle a
/// permission on its own. Every agent connection path must call
/// [`agent_indexed_permission`], which fails closed (`None`).
pub fn indexed_permission(
    journal: Option<&Path>,
    path: &str,
    stored: AdmittedCategory,
) -> AdmittedCategory {
    file_spec(path)
        .and_then(|spec| permission_for_spec(journal, path, spec))
        .unwrap_or(stored)
}

/// Effective permission for an indexed path on every agent connection path.
///
/// Fails closed (`None`) for segment topics (which have a separate multi-kind
/// boundary), paths that match no registry row, non-admitted rows, kinds with
/// no permission switch (such as video), non-content rows, and import
/// summaries whose manifest is missing, unreadable or unrecognised.
pub fn agent_indexed_permission(journal: Option<&Path>, path: &str) -> Option<AdmittedCategory> {
    if super::topics::segment_path(path).is_some() {
        return None; // Segment topics have a separate multi-kind boundary.
    }
    let spec = file_spec(path)?;
    permission_for_spec(journal, path, spec)
}

fn import_summary_permission(journal: Option<&Path>, path: &str) -> Option<AdmittedCategory> {
    let journal = journal?;
    let manifest = body_material::import_summary_manifest(journal, path)?;
    use AdmittedCategory as C;
    Some(match manifest["source_type"].as_str()? {
        "apple_health" | "oura" | "garmin" | "strava" => C::Body,
        "obsidian" | "evernote" | "kindle" => C::Notes,
        "document" => C::Documents,
        "ics" => C::Calendar,
        "image" => C::Photos,
        "chatgpt" | "claude" | "gemini" => C::Facets,
        "text" => body_material::text_summary_permission(journal, path, &manifest)?,
        _ => return None,
    })
}

/// Match an indexed passage to the current summary before applying the
/// current import's grant to it. The index may still contain an earlier job.
pub fn current_import_summary_chunks(
    journal: &Path,
    path: &str,
) -> Option<Vec<crate::chunker::MarkdownChunk>> {
    body_material::summary_import_id(path)?;
    let root = solstone_core_journal_io::JournalRoot::open(journal).ok()?;
    // Use the same 4 MiB bound as live activity prose reads.
    let source = solstone_core_journal_io::read_relative_file_bounded(
        &root,
        Path::new(path),
        4 * 1024 * 1024,
    )
    .ok()??;
    Some(crate::chunker::format_markdown(std::str::from_utf8(&source.bytes).ok()?).chunks)
}

/// Extra access needed in addition to the primary indexed kind.
pub fn additional_indexed_permission(path: &str) -> Option<AdmittedCategory> {
    if super::conversation::unresolved_markdown(path) {
        return Some(AdmittedCategory::Messages);
    }
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
        "transaction_transcript.md" => AdmittedCategory::Transactions,
        "event_transcript.md" => AdmittedCategory::Calendar,
        "image_transcript.md" => AdmittedCategory::Photos,
        _ => indexed_permission(None, path, AdmittedCategory::Transcripts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_imports_use_the_registered_kind_and_only_the_selected_sources() {
        for (stream, name, permission, source) in [
            (
                "import.text",
                "note_transcript.md",
                AdmittedCategory::Notes,
                "imported",
            ),
            (
                "import.text",
                "document_transcript.md",
                AdmittedCategory::Documents,
                "imported",
            ),
            (
                "import.text",
                "conversation_transcript.jsonl",
                AdmittedCategory::Transcripts,
                "imported",
            ),
            (
                "import.image",
                "screenshot_transcript.md",
                AdmittedCategory::ScreenText,
                "imported",
            ),
            (
                "import.image",
                "image_transcript.md",
                AdmittedCategory::Photos,
                "imported",
            ),
            (
                "import.evernote",
                "note_transcript.md",
                AdmittedCategory::Notes,
                "evernote",
            ),
            (
                "import.obsidian",
                "note_transcript.md",
                AdmittedCategory::Notes,
                "obsidian",
            ),
            (
                "import.document",
                "document_transcript.md",
                AdmittedCategory::Documents,
                "pdf",
            ),
        ] {
            let segment = format!("20260407/{stream}/123456_300");
            let path = format!("{segment}/{name}");
            let expected = Some(DashboardImportItem {
                kind: permission.as_str(),
                source,
            });
            assert_eq!(DashboardImportItem::from_file(&path), expected);
            assert_eq!(
                DashboardImportItem::from_segment_files(&segment, [name]),
                expected
            );
        }

        for (stream, name) in [
            ("import.chatgpt", "conversation_transcript.jsonl"),
            ("import.kindle", "book_transcript.md"),
            ("import.ics", "event_transcript.md"),
            ("import.apple_health", "reading_transcript.md"),
            ("phone", "audio.jsonl"),
            ("phone_obsidian", "note_transcript.md"),
            ("import.text", "imported.md"),
            ("import.text", "unknown_transcript.md"),
            ("import.text", "import_kind.json"),
        ] {
            let path = format!("chronicle/20260407/{stream}/123456_300/{name}");
            assert_eq!(DashboardImportItem::from_file(&path), None, "{path}");
        }
    }

    #[test]
    fn dashboard_segment_prefers_descriptive_material_to_an_original() {
        let segment = "20260407/import.image/123456_0";
        let screenshot =
            DashboardImportItem::from_file(&format!("{segment}/screenshot_transcript.md"));
        assert!(screenshot.is_some());
        assert_eq!(
            DashboardImportItem::from_segment_files(
                segment,
                ["original.png", "screenshot_transcript.md"]
            ),
            screenshot
        );
        assert_eq!(
            DashboardImportItem::from_segment_files(segment, ["original.screenshot.png"]),
            screenshot
        );
        assert_eq!(
            DashboardImportItem::from_segment_files(
                segment,
                ["image_transcript.md", "screenshot_transcript.md"]
            ),
            None
        );
    }

    #[test]
    fn manual_imports_keep_index_and_segment_permissions_agreeing() {
        use super::super::{ContentUnit, OfferedTo};
        for (stream, file, permission) in [
            ("import.text", "note_transcript.md", AdmittedCategory::Notes),
            (
                "import.text",
                "document_transcript.md",
                AdmittedCategory::Documents,
            ),
            (
                "import.image",
                "screenshot_transcript.md",
                AdmittedCategory::ScreenText,
            ),
            (
                "import.image",
                "image_transcript.md",
                AdmittedCategory::Photos,
            ),
        ] {
            let path = format!("20260407/{stream}/123456_300/{file}");
            let spec = file_spec(&path).unwrap();
            assert_eq!(spec.unit, ContentUnit::Segment);
            assert_eq!(spec.offered_to, OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS);
            assert_eq!(
                indexed_permission(None, &path, AdmittedCategory::Transcripts),
                permission
            );
            assert_eq!(segment_file_permission(&path), permission);
        }
    }

    #[test]
    fn every_kind_has_the_expected_stored_permission() {
        for spec in KINDS {
            assert_eq!(
                AdmittedCategory::from_stored_word(spec.stored_word),
                spec.permission
            );
            if let Some(permission) = spec.permission {
                assert_eq!(permission.as_str(), spec.stored_word);
                // An option is no material of its own, so search names none.
                if spec.option_of().is_some() {
                    assert_eq!(spec.query_alias, None);
                    continue;
                }
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
                (Kind::Messages, Some("messages")),
                (Kind::Transactions, Some("transactions")),
                (Kind::Entities, Some("entities")),
                (Kind::ProfilePics, Some("profile_pics")),
                (Kind::Facets, Some("facets")),
                (Kind::Screen, Some("screen_text")),
                (Kind::Screenshots, Some("screenshots")),
                (Kind::Body, Some("body")),
                (Kind::WomensHealth, Some("womens_health")),
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
            [
                "transcripts",
                "entities",
                "profile_pics",
                "facets",
                "screen_text",
                "body"
            ]
        );
        for word in ["video", "Video", "BODY", "Screen", "screen", "unknown"] {
            assert_eq!(AdmittedCategory::from_stored_word(word), None);
            assert_eq!(AdmittedCategory::from_query_token(word), None);
        }
    }

    #[test]
    fn profile_pics_ride_entities_and_only_the_owners_no_takes_them_away() {
        let words = |list: &[&str]| list.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        // A grant from before the choice keeps them.
        assert!(profile_pics_admitted(&words(&["entities"])));
        assert!(!profile_pics_admitted(&words(&[
            "entities",
            NO_PROFILE_PICS
        ])));
        assert!(profile_pics_admitted(&words(&["entities", "profile_pics"])));
        assert!(!profile_pics_admitted(&words(&["transcripts"])));

        let mut chosen = words(&["entities"]);
        record_profile_pics_choice(&mut chosen);
        assert_eq!(chosen, words(&["entities", NO_PROFILE_PICS]));
        let mut chosen = words(&["entities", "profile_pics", NO_PROFILE_PICS]);
        record_profile_pics_choice(&mut chosen);
        assert_eq!(chosen, words(&["entities", "profile_pics"]));
        let mut chosen = words(&["transcripts", NO_PROFILE_PICS]);
        record_profile_pics_choice(&mut chosen);
        assert_eq!(chosen, words(&["transcripts"]));
        // New connections start with them ticked, under entities.
        assert!(default_permission_words().any(|word| word == "profile_pics"));
        let spec = AdmittedCategory::ProfilePics.kind_spec();
        assert_eq!(spec.option_of(), Some("entities"));
        assert_eq!(AdmittedCategory::from_stored_word(NO_PROFILE_PICS), None);
    }

    #[test]
    fn agent_indexed_permission_fails_closed_for_unknown_excluded_and_switchless_kinds() {
        use super::super::{ContentResolution, FileCondition, OfferedTo, PatternRoot, ScopeBasis};

        // Paths with no registry row, excluded rows (including video and non-content),
        // and excluded rows under a body stream all fail closed.
        for closed in [
            "20260407/default/120000_60/unknown_mystery.md",
            "20260407/default/120000_60/clip.mp4",
            "20260407/import.strava/120000_60/clip.mp4",
            "20260407/import.image/120000_60/import_kind.json",
            "20260407/phone/120000_60/imported.md",
            "20260407/default/120000_60/talents/topics.md",
            "imports/20260407_090000/summary.md",
        ] {
            assert_eq!(agent_indexed_permission(None, closed), None, "{closed}");
        }

        // Even if a registry row were admitted, a kind with no permission switch
        // (such as Kind::Video) or KindAssignment::NotContent fails closed.
        let admitted_video = FilePattern {
            pattern: "*/*/*/*.mp4",
            root: PatternRoot::DayRooted,
            format: FileFormat::Content(Family::Markdown),
            classification: ContentResolution::Indexed(Family::Markdown),
            disposition: IndexDisposition::Admitted {
                category: AdmittedCategory::Transcripts,
                basis: ScopeBasis::SegmentAssigned,
            },
            kind: KindAssignment::Settled(Kind::Video),
            unit: ContentUnit::Segment,
            offered_to: OfferedTo::NONE,
            condition: FileCondition::Path,
            segment: None,
        };
        assert_eq!(
            permission_for_spec(None, "20260407/default/120000_60/clip.mp4", &admitted_video),
            None
        );

        let admitted_not_content = FilePattern {
            kind: KindAssignment::NotContent,
            ..admitted_video
        };
        assert_eq!(
            permission_for_spec(
                None,
                "20260407/import.image/120000_60/import_kind.json",
                &admitted_not_content
            ),
            None
        );

        // Existing admitted paths keep their exact permissions.
        for (path, expected) in [
            (
                "20260407/import.obsidian/120000_60/note_transcript.md",
                AdmittedCategory::Notes,
            ),
            (
                "20260407/agent-memory-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/120000_60/note.txt",
                AdmittedCategory::Notes,
            ),
            ("20260407/body.md", AdmittedCategory::Body),
            (
                "20260407/import.strava/070000_300/talents/activity.md",
                AdmittedCategory::Body,
            ),
            ("20260407/location.md", AdmittedCategory::Places),
            (
                "20260407/default/120000_60/talents/screen_text.md",
                AdmittedCategory::ScreenText,
            ),
            ("20260407/talents/daily.md", AdmittedCategory::Facets),
            (
                "facets/work/activities/20260407.jsonl",
                AdmittedCategory::Facets,
            ),
            (
                "facets/work/entities/20260407.jsonl",
                AdmittedCategory::Entities,
            ),
            ("reflections/weekly/2026-W15.md", AdmittedCategory::Facets),
        ] {
            assert_eq!(
                agent_indexed_permission(None, path),
                Some(expected),
                "{path}"
            );
        }
    }

    #[test]
    fn places_precision_modifiers_fail_safer_and_preserve_legacy_bare_places() {
        let words = |list: &[&str]| list.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        // Without "places", stray modifiers grant nothing.
        assert_eq!(places_precision_admitted(&words(&["transcripts"])), None);
        assert_eq!(
            places_precision_admitted(&words(&["transcripts", PLACES_PRECISION_FULL])),
            None
        );
        // Legacy bare "places" stays at NamesOnly.
        assert_eq!(
            places_precision_admitted(&words(&["places"])),
            Some(PlacesPrecision::NamesOnly)
        );
        assert_eq!(
            places_precision_admitted(&words(&["places", PLACES_PRECISION_NAMES])),
            Some(PlacesPrecision::NamesOnly)
        );
        assert_eq!(
            places_precision_admitted(&words(&["places", PLACES_PRECISION_APPROX])),
            Some(PlacesPrecision::Approx)
        );
        assert_eq!(
            places_precision_admitted(&words(&["places", PLACES_PRECISION_FULL])),
            Some(PlacesPrecision::Full)
        );
        // Conflicting modifiers resolve to the safer/coarser tier.
        assert_eq!(
            places_precision_admitted(&words(&[
                "places",
                PLACES_PRECISION_FULL,
                PLACES_PRECISION_APPROX
            ])),
            Some(PlacesPrecision::Approx)
        );
        assert_eq!(
            places_precision_admitted(&words(&[
                "places",
                PLACES_PRECISION_FULL,
                PLACES_PRECISION_NAMES
            ])),
            Some(PlacesPrecision::NamesOnly)
        );
        // Recording an owner choice defaults to Full when places is ticked and strips when unticked.
        let mut chosen = words(&["places"]);
        record_places_precision_choice(&mut chosen, None);
        assert_eq!(chosen, words(&["places", PLACES_PRECISION_FULL]));
        record_places_precision_choice(&mut chosen, Some(PlacesPrecision::Approx));
        assert_eq!(chosen, words(&["places", PLACES_PRECISION_APPROX]));
        let mut unticked = words(&["transcripts", PLACES_PRECISION_FULL]);
        record_places_precision_choice(&mut unticked, Some(PlacesPrecision::Full));
        assert_eq!(unticked, words(&["transcripts"]));
        // Precision modifiers are never standalone kinds or permissions.
        for word in [
            PLACES_PRECISION_FULL,
            PLACES_PRECISION_APPROX,
            PLACES_PRECISION_NAMES,
        ] {
            assert!(is_places_precision_word(word));
            assert_eq!(AdmittedCategory::from_stored_word(word), None);
            assert_eq!(AdmittedCategory::from_query_token(word), None);
        }
        // Places stays unchecked by default.
        assert!(!default_permission_words().any(|word| word == "places"));
    }
}
