// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod action_logs;
mod activities;
pub use activities::{
    BODY_ENTRY_LINE, BODY_RECORD_SOURCE, EDIT_REMOVED, EDIT_STRETCH, STRETCH_ID, STRETCH_SEGMENTS,
    activity_field_held, body_record_at, body_record_indexed, correction_segments, edit_names,
    edit_removed, is_body_record, is_hidden_activity, is_retired_plan, is_strava_reading,
    non_body_entries, ordinary_entries, owner_edit,
};
mod ai_chat;
mod audio;
pub mod body_records;
pub mod granola;
pub use audio::{OTHER_VOICE, audio_speaker_label};
mod browser;
pub mod conversation;
mod day_accumulator;
mod desktop_activity;
mod documents;
pub mod experiences;
pub use experiences::{
    ActivityRef, ExperienceMoment, ExperienceRecord, MembershipSource, OwnerHolds,
    ShareDetailPolicy, experience_field_held, experience_moment_held, is_hidden_experience,
};
mod events;
pub use events::is_retired_event_plan;
mod facet_entities;
mod formatters;
mod imports;
pub use formatters::{FormatterView, PlaceFormatter, format_file};
pub mod kinds;
mod registry;
pub mod topics;
pub use registry::*;
mod morning_briefing;
pub use morning_briefing::morning_briefing_presentations;
mod observations;
mod projections;
mod protected_video;
mod raw_screen;
mod screen;
mod sense;
pub use sense::{owner_sense_text, sense_presentations};
mod shape;
pub mod strava_body;
mod talent_projections;
mod workout;

use kinds::Kind;
use serde_json::{Map, Value};

use crate::chunker::format_markdown;
use crate::matcher::{PatternSpec, Resolver, patterns_for_root as filter_patterns_for_root};

pub mod body_hidden_days;
pub mod body_material;
pub mod possible_names;
pub mod screen_text;
pub mod week_left_out;

pub use crate::matcher::PatternRoot;
pub use projections::{
    render_browser_text, render_browser_text_for_locale, render_morning_briefing_text,
    render_morning_briefing_text_for_locale, render_raw_screen_text,
    render_raw_screen_text_for_locale,
};
pub use shape::{SHAPE_SIDECAR_BASENAME, parse_shape_name, resolve_content_shape};
pub use talent_projections::{
    ConsumedOriginal, TalentTextProjection, iter_talent_text_projections,
    iter_talent_text_projections_for_locale, talent_projection_map,
};

/// Private import provenance. This file is not a content input.
pub const IMPORT_KIND_FILENAME: &str = "import_kind.json";

/// Imported image metadata (capture zone, time source, owner caption). Not a content input.
pub const IMAGE_META_FILENAME: &str = "image_meta.json";

/// Canonical per-photo source-bound observation sidecar. Not a direct content input;
/// indexed prose stays in `image_transcript.md`.
pub const PHOTO_OBSERVATION_FILENAME: &str = "photo_observation.json";

/// Optional tmux capture metadata, retained on disk but not yet used by talents.
pub const TMUX_PANE_CAPTURE_FIELDS: [&str; 5] = [
    "cursor_x",
    "cursor_y",
    "cursor_visible",
    "alternate_on",
    "pane_in_mode",
];

/// Remove capture-only metadata from an in-memory tmux talent input view.
pub fn omit_tmux_capture_fields(tmux: &mut Value) {
    if let Some(panes) = tmux.get_mut("panes").and_then(Value::as_array_mut) {
        for pane in panes.iter_mut().filter_map(Value::as_object_mut) {
            for key in TMUX_PANE_CAPTURE_FIELDS {
                pane.shift_remove(key);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Markdown,
    Event,
    Activity,
    /// Facet-scoped derived Experience records. This family is path-selected only.
    Experience,
    ActionLog,
    StructuredImport,
    AiChat,
    Browser,
    DayAccumulator,
    FacetEntity,
    Observation,
    Documents,
    Screen,
    Sense,
    MorningBriefing,
    /// A validated private memory original. This family is path-selected only.
    AgentMemory,
    /// One piece of a Strava workout. This family is path-selected only.
    Workout,
    /// A body window's records. This family is path-selected only.
    BodyRecords,
    /// A pause while a protected-video window was on screen. This family is path-selected only.
    ProtectedVideo,
    /// Desktop activity notes. This family is path-selected only.
    DesktopActivity,
    /// Text recognized from screens. This family is path-selected only.
    ScreenText,
    /// Calendar notes from a linked device. This family is path-selected only.
    Calendar,
    /// Possible names a segment's audio may refer to. This family is path-selected only.
    PossibleNames,
    /// Speech transcript lines. This family is path-selected only.
    Audio,
}

/// Content that can be rendered for direct consumption but is deliberately
/// excluded from the search index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPerceptFamily {
    Audio,
    RawScreen,
}

#[cfg(test)]
const ALL_FAMILIES: [Family; 24] = [
    Family::Markdown,
    Family::Event,
    Family::Activity,
    Family::Experience,
    Family::ActionLog,
    Family::StructuredImport,
    Family::AiChat,
    Family::Browser,
    Family::DayAccumulator,
    Family::FacetEntity,
    Family::Observation,
    Family::Documents,
    Family::Screen,
    Family::Sense,
    Family::MorningBriefing,
    Family::AgentMemory,
    Family::Workout,
    Family::BodyRecords,
    Family::ProtectedVideo,
    Family::DesktopActivity,
    Family::ScreenText,
    Family::Calendar,
    Family::PossibleNames,
    Family::Audio,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentResolution {
    Indexed(Family),
    Unindexed(RawPerceptFamily),
    IndexedElsewhere,
    Unrecognized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnindexedReason {
    RawPercept(RawPerceptFamily),
    IndexedElsewhere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OccurrenceTimeMs(pub i64);

impl From<i64> for OccurrenceTimeMs {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexChunk {
    pub content: String,
    pub occurrence_time_ms: Option<OccurrenceTimeMs>,
    pub source: Option<Map<String, Value>>,
    pub group_offset_ms: Option<i64>,
}

/// Whether a chunk goes into the search index. An empty chunk doesn't, and
/// neither does an activity file's entry for a hidden record: both keep their
/// position, so every other entry keeps its own. Only [`Family::Activity`]
/// chunks are judged by `source.hidden`.
#[must_use]
pub fn indexed(family: Family, chunk: &IndexChunk) -> bool {
    !chunk.content.trim().is_empty()
        && !(matches!(family, Family::Activity | Family::Experience)
            && chunk
                .source
                .as_ref()
                .and_then(|source| source.get("hidden"))
                .and_then(Value::as_bool)
                .unwrap_or(false))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducedChunks {
    pub chunks: Vec<IndexChunk>,
    pub agent_override: Option<String>,
    pub header: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<solstone_core_cli_copy::MessageDetail>,
}

/// Screen-talent rendering plus projector-owned tmux chunk identity.
///
/// `tmux_chunk_indices` is a side channel: owner content can contain any text,
/// so rendered Markdown must never be parsed to rediscover these boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenTalentRawScreen {
    pub chunks: Vec<IndexChunk>,
    pub agent_override: Option<String>,
    pub header: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<solstone_core_cli_copy::MessageDetail>,
    pub tmux_chunk_indices: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdmittedCategory {
    Transcripts,
    /// Audio recordings backing admitted transcript segments. An opt-in
    /// sub-option of `transcripts`, never standalone content.
    AudioMedia,
    Messages,
    Transactions,
    Entities,
    Facets,
    ScreenText,
    /// What body imports brought in. ⛔ Never a stored category: a reader
    /// decides it from the path (`body_material`), so a row stored as
    /// transcripts before it existed is gated as body too.
    Body,
    /// Women's Health body records (cycle, reproductive and pregnancy rows).
    /// An opt-in sub-option of `body`, never standalone content.
    WomensHealth,
    Notes,
    Documents,
    Calendar,
    Places,
    Photos,
    /// The picture shown for an entity. An option of entities, never content.
    ProfilePics,
    Screenshots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeBasis {
    FacetOwned,
    SegmentAssigned,
    JournalWide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexDisposition {
    Admitted {
        category: AdmittedCategory,
        basis: ScopeBasis,
    },
    Excluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FamilyPattern {
    pub pattern: &'static str,
    pub family: Family,
    pub root: PatternRoot,
    pub disposition: IndexDisposition,
}

pub(crate) const INDEX_FAMILY_PATTERNS: &[FilePattern] = &[
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.evernote/*/note_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Notes),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.obsidian/*/note_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Notes),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.apple_health/*/*_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.oura/*/*_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.strava/*/*_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.document/*/document_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Documents),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.email/*/document_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Documents),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.image/*/screenshot_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.image/*/image_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Photos),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/photos/*/image_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Photos),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.ics/*/event_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Calendar),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/screen.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Legacy(Kind::Screen),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/agent-memory-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]/*/note.txt",
        format: FileFormat::Content(Family::AgentMemory),
        classification: ContentResolution::Indexed(Family::AgentMemory),
        kind: KindAssignment::Settled(Kind::Notes),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::JournalWide,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/talents/*.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Day,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Facets,
            basis: ScopeBasis::JournalWide,
        },
        segment: None,
    },
    // A day's location in words (`solstone-core-location` writes it): area
    // names and times, never coordinates.
    FilePattern {
        pattern: "*/location.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Places),
        unit: ContentUnit::Day,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::JournalWide,
        },
        segment: None,
    },
    // A day's body record in words (`solstone-core-location` writes it): what
    // was recorded and by what, never a reading's value.
    FilePattern {
        pattern: "*/body.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Day,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::JournalWide,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/sense.json",
        format: FileFormat::Content(Family::Sense),
        classification: ContentResolution::Indexed(Family::Sense),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/documents.json",
        format: FileFormat::Content(Family::Documents),
        classification: ContentResolution::Indexed(Family::Documents),
        kind: KindAssignment::Settled(Kind::Documents),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/screen.json",
        format: FileFormat::Content(Family::Screen),
        classification: ContentResolution::Indexed(Family::Screen),
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::SEARCH_DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/talents/morning_briefing.json",
        format: FileFormat::Content(Family::MorningBriefing),
        classification: ContentResolution::Indexed(Family::MorningBriefing),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/talents/*.jsonl",
        format: FileFormat::Content(Family::DayAccumulator),
        classification: ContentResolution::Indexed(Family::DayAccumulator),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/screen_text.md",
        format: FileFormat::Content(Family::ScreenText),
        classification: ContentResolution::Indexed(Family::ScreenText),
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::ScreenText,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    // Possible names for a segment's audio: only the labelled canonical name
    // is indexed. Readers re-check every entry before a hit is shown
    // (`solstone-core-possible-names`).
    FilePattern {
        pattern: "*/*/*/talents/possible_names.json",
        format: FileFormat::Content(Family::PossibleNames),
        classification: ContentResolution::Indexed(Family::PossibleNames),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/audio.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Legacy(Kind::Transcripts),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/topics.provenance.json",
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/topics.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/*.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/*/*.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Facets,
            basis: ScopeBasis::FacetOwned,
        },
        segment: None,
    },
    // Deterministic transaction records (receipts, invoices, statements, order
    // confirmations). Searchable and fetchable under the `transactions` grant,
    // never offered to talents, facets, entities, or places.
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/*/*/transaction_transcript.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Transactions),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::JournalWide,
        },
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_AGENT)),
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.kindle/*/*_transcript.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Notes),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.text/*/document_transcript.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Documents),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.text/*/note_transcript.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Notes),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.text/*/*_transcript.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Notes),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.*/*/*_transcript.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.*/*/imported.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Legacy(Kind::Transcripts),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/import.apple_health/*/body.jsonl",
        format: FileFormat::Content(Family::BodyRecords),
        classification: ContentResolution::Indexed(Family::BodyRecords),
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/import.oura/*/body.jsonl",
        format: FileFormat::Content(Family::BodyRecords),
        classification: ContentResolution::Indexed(Family::BodyRecords),
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/import.garmin/*/body.jsonl",
        format: FileFormat::Content(Family::BodyRecords),
        classification: ContentResolution::Indexed(Family::BodyRecords),
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/import.strava/*/workout.json",
        format: FileFormat::Content(Family::Workout),
        classification: ContentResolution::Indexed(Family::Workout),
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Event,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/browser_*.jsonl",
        format: FileFormat::Content(Family::Browser),
        classification: ContentResolution::Indexed(Family::Browser),
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::SEARCH_DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_TALENTS)),
    },
    // Note changes a linked device reports from an Obsidian vault, written by
    // ingest as one transcript per window.
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/*/*/note_transcript.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Notes),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    // Notes about the owner's calendar. Readable by a connection that sees the
    // whole journal's transcripts; they belong to no facet.
    FilePattern {
        pattern: "*/*/*/calendar_*.jsonl",
        format: FileFormat::Content(Family::Calendar),
        classification: ContentResolution::Indexed(Family::Calendar),
        kind: KindAssignment::Settled(Kind::Calendar),
        unit: ContentUnit::Event,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::JournalWide,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*_protected_video.jsonl",
        format: FileFormat::Content(Family::ProtectedVideo),
        classification: ContentResolution::Indexed(Family::ProtectedVideo),
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::SEARCH_DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/*_desktop_activity.jsonl",
        format: FileFormat::Content(Family::DesktopActivity),
        classification: ContentResolution::Indexed(Family::DesktopActivity),
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::SEARCH_DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_TALENTS)),
    },
    FilePattern {
        pattern: "*/import.*/imported.jsonl",
        format: FileFormat::Content(Family::StructuredImport),
        classification: ContentResolution::Indexed(Family::StructuredImport),
        kind: KindAssignment::Legacy(Kind::Notes),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/import.chatgpt/*/conversation_transcript.jsonl",
        format: FileFormat::Content(Family::AiChat),
        classification: ContentResolution::Indexed(Family::AiChat),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/import.claude/*/conversation_transcript.jsonl",
        format: FileFormat::Content(Family::AiChat),
        classification: ContentResolution::Indexed(Family::AiChat),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/import.gemini/*/conversation_transcript.jsonl",
        format: FileFormat::Content(Family::AiChat),
        classification: ContentResolution::Indexed(Family::AiChat),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/import.text/*/conversation_transcript.jsonl",
        format: FileFormat::Content(Family::AiChat),
        classification: ContentResolution::Indexed(Family::AiChat),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/import.email/*/conversation_transcript.jsonl",
        format: FileFormat::Content(Family::AiChat),
        classification: ContentResolution::Indexed(Family::AiChat),
        kind: KindAssignment::Settled(Kind::Messages),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "*/import.chatgpt/*/imported_audio.jsonl",
        format: FileFormat::Content(Family::AiChat),
        classification: ContentResolution::Indexed(Family::AiChat),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/import.claude/*/imported_audio.jsonl",
        format: FileFormat::Content(Family::AiChat),
        classification: ContentResolution::Indexed(Family::AiChat),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "*/import.gemini/*/imported_audio.jsonl",
        format: FileFormat::Content(Family::AiChat),
        classification: ContentResolution::Indexed(Family::AiChat),
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        root: PatternRoot::DayRooted,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "config/actions/*.jsonl",
        format: FileFormat::Content(Family::ActionLog),
        classification: ContentResolution::Indexed(Family::ActionLog),
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/events/*.jsonl",
        format: FileFormat::Content(Family::Event),
        classification: ContentResolution::Indexed(Family::Event),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Event,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Facets,
            basis: ScopeBasis::FacetOwned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/entities/*/observations.jsonl",
        format: FileFormat::Content(Family::Observation),
        classification: ContentResolution::Indexed(Family::Observation),
        kind: KindAssignment::Settled(Kind::Entities),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Entities,
            basis: ScopeBasis::FacetOwned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/entities/*.jsonl",
        format: FileFormat::Content(Family::FacetEntity),
        classification: ContentResolution::Indexed(Family::FacetEntity),
        kind: KindAssignment::Settled(Kind::Entities),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Entities,
            basis: ScopeBasis::FacetOwned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/activities/*.jsonl",
        format: FileFormat::Content(Family::Activity),
        classification: ContentResolution::Indexed(Family::Activity),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Event,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Facets,
            basis: ScopeBasis::FacetOwned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/experiences/*.jsonl",
        format: FileFormat::Content(Family::Experience),
        classification: ContentResolution::Indexed(Family::Experience),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Event,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Facets,
            basis: ScopeBasis::FacetOwned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/logs/*.jsonl",
        format: FileFormat::Content(Family::ActionLog),
        classification: ContentResolution::Indexed(Family::ActionLog),
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/activities/*/*/*.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Facets,
            basis: ScopeBasis::FacetOwned,
        },
        segment: None,
    },
    FilePattern {
        pattern: "reflections/weekly/*.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Facets,
            basis: ScopeBasis::JournalWide,
        },
        segment: None,
    },
    FilePattern {
        pattern: "imports/*/summary.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::ImportSummary,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::JournalWide,
        },
        segment: None,
    },
    FilePattern {
        pattern: "apps/*/talents/*.md",
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Indexed(Family::Markdown),
        kind: KindAssignment::Legacy(Kind::Facets),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD,
        condition: FileCondition::Path,
        root: PatternRoot::Structural,
        disposition: IndexDisposition::Excluded,
        segment: None,
    },
    FilePattern {
        pattern: "entities/*/entity.json",
        root: PatternRoot::Structural,
        format: FileFormat::IndexedElsewhere,
        classification: ContentResolution::IndexedElsewhere,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Entities),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "reflections/weekly/*.json",
        root: PatternRoot::Structural,
        format: FileFormat::IndexedElsewhere,
        classification: ContentResolution::IndexedElsewhere,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Facets),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::DASHBOARD_AGENT,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/audio.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Raw(RawPerceptFamily::Audio),
        classification: ContentResolution::Indexed(Family::Audio),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_AGENT_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/*_audio.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Raw(RawPerceptFamily::Audio),
        classification: ContentResolution::Indexed(Family::Audio),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_AGENT_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/*_transcript.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Raw(RawPerceptFamily::Audio),
        classification: ContentResolution::Indexed(Family::Audio),
        disposition: IndexDisposition::Admitted {
            category: AdmittedCategory::Transcripts,
            basis: ScopeBasis::SegmentAssigned,
        },
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::SEARCH_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::AGENT_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/screen.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Raw(RawPerceptFamily::RawScreen),
        classification: ContentResolution::Unindexed(RawPerceptFamily::RawScreen),
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/*_screen.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Raw(RawPerceptFamily::RawScreen),
        classification: ContentResolution::Unindexed(RawPerceptFamily::RawScreen),
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/location.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Place,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Places),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD)),
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.image/*/original.screenshot.*",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.image/*/original.*",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Photos),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/photos/*/original.*",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Photos),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.document/*/original.*",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Documents),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/import.document/*/pages/*.png",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Documents),
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/import.*/*/import_kind.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/import.image/*/image_meta.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/photos/*/image_meta.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/*/*/photo_observation.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/screenshot_screen.*",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*_transcript.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_AGENT_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/imported.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Markdown),
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Legacy(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD_AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_AGENT_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/body.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::BodyRecords),
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Window,
        offered_to: OfferedTo::AGENT,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::AGENT)),
    },
    FilePattern {
        pattern: "*/*/*/workout.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Workout),
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Body),
        unit: ContentUnit::Event,
        offered_to: OfferedTo::AGENT_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::AGENT_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/*.png",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Photos),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*.jpg",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Photos),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*.jpeg",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Photos),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*.webp",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Photos),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*.flac",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*.webm",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*.m4a",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*screen.mp4",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*.mp4",
        root: PatternRoot::DayRooted,
        format: FileFormat::Original,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Video),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "config/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "health/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        // Social memory: activities other people shared with this journal,
        // and the bookkeeping for activities it shared. A received memory is
        // someone else's account, so no talent, agent, search or dashboard
        // reader is offered it.
        pattern: "social/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "awareness/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "link/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "streams/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "mcp-endpoint/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "indexer/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "logs/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "solstone/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "apps/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "imports/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "entities/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/facet.json",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/activities.jsonl",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/news/*.md",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/agent-runs/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/journal/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/state/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "AGENTS.md",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "CLAUDE.md",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "GEMINI.md",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "README.md",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: ".gitignore",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: ".gitkeep",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/health/**",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/stream.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/ingest.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/events.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/placement.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/shape.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/*.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/talents/*/*.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/health/**",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/.*",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*.sqlite",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "maint/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "talents/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "tokens/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/review-candidates.jsonl",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/.gitkeep",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/entities.md",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "facets/*/entities/*/entity.json",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "entities.md",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "summary.md",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "entity_review.log",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/indexer/**",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/README.md",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/.gitkeep",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/audio.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*_diff.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*_diff_box.json",
        root: PatternRoot::DayRooted,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/*",
        root: PatternRoot::DayRooted,
        format: FileFormat::PhotoDescription,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Photos),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD_TALENTS,
        condition: FileCondition::ImageHeader,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/*",
        root: PatternRoot::DayRooted,
        format: FileFormat::Raw(RawPerceptFamily::Audio),
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::AGENT_TALENTS,
        condition: FileCondition::TranscribeHeader,
        segment: Some(SegmentRule::new(OfferedTo::AGENT_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/*.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Raw(RawPerceptFamily::Audio),
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::AGENT_TALENTS,
        condition: FileCondition::ArchiveName,
        segment: Some(SegmentRule::new(OfferedTo::AGENT_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/*audio.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Raw(RawPerceptFamily::Audio),
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Transcripts),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_TALENTS)),
    },
    FilePattern {
        pattern: "*/*/*/*screen.jsonl",
        root: PatternRoot::DayRooted,
        format: FileFormat::Raw(RawPerceptFamily::RawScreen),
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Screen),
        unit: ContentUnit::Segment,
        offered_to: OfferedTo::DASHBOARD_TALENTS,
        condition: FileCondition::Path,
        segment: Some(SegmentRule::new(OfferedTo::DASHBOARD_TALENTS)),
    },
    FilePattern {
        pattern: "cache/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "backup/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: ".agents/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: ".claude/**",
        root: PatternRoot::Structural,
        format: FileFormat::NotContent,
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::NotContent,
        unit: ContentUnit::Record,
        offered_to: OfferedTo::NONE,
        condition: FileCondition::Path,
        segment: None,
    },
    FilePattern {
        pattern: "*/*/*/calendar_*",
        root: PatternRoot::DayRooted,
        format: FileFormat::Content(Family::Calendar),
        classification: ContentResolution::Unrecognized,
        disposition: IndexDisposition::Excluded,
        kind: KindAssignment::Settled(Kind::Calendar),
        unit: ContentUnit::Event,
        offered_to: OfferedTo::DASHBOARD,
        condition: FileCondition::Path,
        segment: Some(SegmentRule {
            readers: OfferedTo::DASHBOARD,
            calendar_stream: true,
        }),
    },
];

impl PatternSpec<ContentResolution> for FilePattern {
    fn pattern(&self) -> &'static str {
        self.pattern
    }
    fn root(&self) -> PatternRoot {
        self.root
    }
    fn value(&self) -> ContentResolution {
        self.classification
    }
}

static CONTENT_RESOLVER: Resolver<ContentResolution, FilePattern> =
    Resolver::new(INDEX_FAMILY_PATTERNS);

/// Resolve a stored source by path, then recognize transcribe's own header.
/// Header recognition adds only an unindexed raw-percept family; it never
/// grants an indexed category or overrides a path-owned family.
pub fn classify_file(path: &std::path::Path, rel: &str) -> ContentResolution {
    let resolution = classify(rel);
    if resolution != ContentResolution::Unrecognized {
        return resolution;
    }
    let rel = rel.strip_prefix("chronicle/").unwrap_or(rel);
    let parts: Vec<_> = rel.split('/').collect();
    if parts.len() == 4
        && crate::segment::is_date_key(parts[0])
        && crate::segment::segment_parse(parts[2]).is_some()
        && solstone_core_processing_record::is_transcribe_output(path)
    {
        ContentResolution::Unindexed(RawPerceptFamily::Audio)
    } else {
        resolution
    }
}

pub fn classify(rel: &str) -> ContentResolution {
    if let Some(row) = CONTENT_RESOLVER.resolve_spec_where(rel, |row| {
        row.classification != ContentResolution::Unrecognized
    }) {
        return row.classification;
    }
    let mut components = rel.split('/');
    if let (Some(day), Some("import.archive"), Some(segment), Some(name), None) = (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) && crate::segment::is_date_key(day)
        && crate::segment::segment_parse(segment).is_some()
        && crate::audio_archive::is_audio_archive_transcript_name(name)
    {
        return ContentResolution::Unindexed(RawPerceptFamily::Audio);
    }
    ContentResolution::Unrecognized
}

/// Return the matched family specification for authorization classification.
///
/// This deliberately ignores a sibling `shape.json`: sidecars select chunking,
/// not who may see an already-indexed source.
pub fn resolve_spec(rel: &str) -> Option<FamilyPattern> {
    CONTENT_RESOLVER
        .resolve_spec_where(rel, |row| {
            matches!(row.classification, ContentResolution::Indexed(_))
        })
        .and_then(FilePattern::index_spec)
}

/// Whether `path` resolves to an indexed speech transcript file (`Family::Audio`).
#[must_use]
pub fn is_audio_transcript_path(path: &str) -> bool {
    let normalized = path.strip_prefix("chronicle/").unwrap_or(path);
    resolve_spec(normalized).is_some_and(|spec| spec.family == Family::Audio)
}

pub fn patterns_for_root(root: PatternRoot) -> impl Iterator<Item = FamilyPattern> {
    filter_patterns_for_root(INDEX_FAMILY_PATTERNS, root).filter_map(FilePattern::index_spec)
}

/// Full file registry, including files deliberately invisible to content readers.
/// Search compatibility views above retain their original domain.
pub fn file_spec(rel: &str) -> Option<&'static FilePattern> {
    file_spec_with_facts(rel, FileFacts::default())
}

pub fn file_spec_with_facts(rel: &str, facts: FileFacts) -> Option<&'static FilePattern> {
    let rel = rel.strip_prefix("chronicle/").unwrap_or(rel);
    let parts = rel.split('/').collect::<Vec<_>>();
    let direct;
    let rel = if parts.len() == 3
        && crate::segment::is_date_key(parts[0])
        && crate::segment::segment_parse(parts[1]).is_some()
    {
        direct = format!("{}/_default/{}/{}", parts[0], parts[1], parts[2]);
        direct.as_str()
    } else {
        rel
    };
    let name = rel.rsplit('/').next().unwrap_or(rel);
    CONTENT_RESOLVER.resolve_spec_where(rel, |row| row.condition.matches(name, facts))
}

/// Render an indexed non-Markdown content family from already-parsed records.
///
/// `Family::Markdown` has no record-shaped representation. Use
/// [`produce_chunks`] for Markdown text instead.
pub fn produce_chunks_by_shape(
    family: Family,
    rel: Option<&str>,
    records: &[JsonObject],
) -> ProducedChunks {
    produce_chunks_by_shape_for_locale(family, rel, records, solstone_core_window_catalog::ENGLISH)
}

/// Render an indexed non-Markdown content family from already-parsed records in `locale`.
pub fn produce_chunks_by_shape_for_locale(
    family: Family,
    rel: Option<&str>,
    records: &[JsonObject],
    locale: &str,
) -> ProducedChunks {
    let rel_text = rel.unwrap_or("");
    let mut produced = match family {
        // Markdown renders from text, not records, so there is nothing to
        // produce here. `produce_chunks` guards it before delegating — but this
        // is a public entry point, and an outside caller that resolved a
        // markdown shape reaches it directly. That caller is precisely the one
        // this function exists to serve, so report the misuse rather than
        // aborting their process: an empty render carrying an error is
        // recoverable, a panic in a library is not.
        Family::Markdown => ProducedChunks {
            chunks: Vec::new(),
            agent_override: None,
            header: None,
            error: Some(
                "markdown renders from text, not records; call produce_chunks with the file text"
                    .to_string(),
            ),
            warnings: Vec::new(),
        },
        Family::Event => events::render_for_locale(rel_text, records, locale),
        Family::Activity => activities::render_for_locale(rel, records, locale),
        Family::Experience => experiences::render_for_locale(rel, records, locale),
        Family::ActionLog => action_logs::render_for_locale(rel_text, records, locale),
        Family::StructuredImport => imports::render_for_locale(records, locale),
        Family::AiChat => ai_chat::render_for_locale(rel_text, records, locale),
        Family::Browser => browser::render_for_locale(records, locale),
        Family::ProtectedVideo => protected_video::render_for_locale(records, locale),
        Family::DesktopActivity => desktop_activity::render_for_locale(rel_text, records, locale),
        Family::DayAccumulator => day_accumulator::render(rel_text, records),
        Family::FacetEntity => facet_entities::render_for_locale(rel_text, records, locale),
        Family::Observation => observations::render_for_locale(rel_text, records, locale),
        Family::Documents => documents::render_for_locale(records, locale),
        Family::Screen => screen::render_for_locale(records, locale),
        Family::Sense => sense::render(records),
        Family::MorningBriefing => morning_briefing::render_for_locale(records, locale),
        Family::Workout => workout::render_for_locale(records, locale),
        Family::BodyRecords => body_records::render_for_locale(records, locale),
        Family::Calendar => render_calendar(records),
        Family::PossibleNames => possible_names::render_for_locale(records, locale),
        Family::Audio => audio::render(rel_text, records),
        Family::AgentMemory => ProducedChunks {
            chunks: Vec::new(),
            agent_override: Some("memory".to_string()),
            header: None,
            error: Some("agent-memory notes render from literal text".to_string()),
            warnings: Vec::new(),
        },
        Family::ScreenText => ProducedChunks {
            chunks: Vec::new(),
            agent_override: None,
            header: None,
            error: Some("screen-text renders from text, not records; call produce_chunks with the file text".to_string()),
            warnings: Vec::new(),
        },
    };
    if produced.error.is_none() && produced.chunks.is_empty() {
        for record in records.iter().filter(|record| {
            has_partial_input(record)
                && !is_retired_plan(record)
                && !is_body_record(record)
                && !is_retired_event_plan(record)
        }) {
            produced
                .chunks
                .push(recorded_chunk_for_locale(String::new(), 0, record, locale));
        }
    }
    produced
}

/// One chunk per note that meets the contract, in file order, timed at when
/// the change was recorded.
fn render_calendar(records: &[JsonObject]) -> ProducedChunks {
    let mut warnings = Vec::new();
    let chunks = records
        .iter()
        .filter_map(|record| {
            match crate::calendar::calendar_note_from_record_detail(record.clone()) {
                Ok(note) => Some(IndexChunk {
                    content: crate::calendar::render_calendar_note(
                        &note,
                        &crate::calendar::OwnerAddresses::default(),
                    ),
                    occurrence_time_ms: Some(OccurrenceTimeMs(note.changed_at_ms())),
                    source: Some(record.clone()),
                    group_offset_ms: None,
                }),
                Err(reason) => {
                    warnings.push(reason.context_slot(
                        "journal.cli.journal.indexer.chunker.calendar",
                        "reason",
                        [],
                    ));
                    None
                }
            }
        })
        .collect();
    ProducedChunks {
        chunks,
        agent_override: Some("calendar".to_string()),
        header: None,
        error: None,
        warnings,
    }
}

/// Render browser JSONL the way a talent reads it to learn what the owner did: pages that were not in front of
/// a focused window are listed with their state, without their text (`browser::render_attended`).
pub fn produce_browser_attended_chunks(text: &str) -> ProducedChunks {
    browser::render_attended(&parse_records_for_family(Family::Browser, text))
}

/// The attended browser view fitted to `budget` tokens for a window too small for it.
pub fn browser_attended_reading(
    text: &str,
    budget: u32,
    count: &dyn Fn(&str) -> u32,
) -> Option<browser::BrowserReading> {
    browser::attended_reading(
        &parse_records_for_family(Family::Browser, text),
        budget,
        count,
    )
}

pub fn produce_chunks(family: Family, rel: &str, text: &str) -> ProducedChunks {
    format_file(
        FileFormat::Content(family),
        rel,
        text,
        FormatterView::Stored,
    )
}

/// Render an indexed content family for owner display in `locale`.
pub fn produce_chunks_for_locale(
    family: Family,
    rel: &str,
    text: &str,
    locale: &str,
) -> ProducedChunks {
    format_file(
        FileFormat::Content(family),
        rel,
        text,
        FormatterView::Localized(locale),
    )
}

fn produce_content_chunks(family: Family, rel: &str, text: &str, locale: &str) -> ProducedChunks {
    if family == Family::ScreenText {
        return screen_text::produce_screen_text_chunks(text);
    }
    if family == Family::AgentMemory {
        return ProducedChunks {
            chunks: if text.trim().is_empty() {
                Vec::new()
            } else {
                vec![IndexChunk {
                    content: text.to_string(),
                    occurrence_time_ms: None,
                    source: None,
                    group_offset_ms: None,
                }]
            },
            agent_override: Some("memory".to_string()),
            header: None,
            error: None,
            warnings: Vec::new(),
        };
    }
    if family == Family::Markdown {
        let formatted = format_markdown(text);
        return ProducedChunks {
            chunks: formatted
                .chunks
                .into_iter()
                .map(|chunk| IndexChunk {
                    content: chunk.markdown,
                    occurrence_time_ms: None,
                    source: None,
                    group_offset_ms: None,
                })
                .collect(),
            agent_override: None,
            header: None,
            error: None,
            warnings: formatted.warnings,
        };
    }

    let records = parse_records_for_family(family, text);
    produce_chunks_by_shape_for_locale(family, Some(rel), &records, locale)
}

/// Render a raw percept from already-parsed records outside the indexed
/// content-family pipeline.
pub fn produce_raw_percept_chunks_by_shape(
    family: RawPerceptFamily,
    rel: Option<&str>,
    records: &[JsonObject],
) -> ProducedChunks {
    produce_raw_percept_chunks_by_shape_for_locale(
        family,
        rel,
        records,
        solstone_core_window_catalog::ENGLISH,
    )
}

/// Render a raw percept from already-parsed records in `locale`.
pub fn produce_raw_percept_chunks_by_shape_for_locale(
    family: RawPerceptFamily,
    rel: Option<&str>,
    records: &[JsonObject],
    locale: &str,
) -> ProducedChunks {
    let rel = rel.unwrap_or("");
    match family {
        RawPerceptFamily::Audio => audio::render(rel, records),
        RawPerceptFamily::RawScreen => raw_screen::render_for_locale(rel, records, locale),
    }
}

/// Render a raw percept outside the indexed content-family pipeline.
pub fn produce_raw_percept_chunks(
    family: RawPerceptFamily,
    rel: &str,
    text: &str,
) -> ProducedChunks {
    format_file(FileFormat::Raw(family), rel, text, FormatterView::Stored)
}

/// Render a raw percept for owner display in `locale`.
pub fn produce_raw_percept_chunks_for_locale(
    family: RawPerceptFamily,
    rel: &str,
    text: &str,
    locale: &str,
) -> ProducedChunks {
    format_file(
        FileFormat::Raw(family),
        rel,
        text,
        FormatterView::Localized(locale),
    )
}

/// The records a raw percept renders from, parsed exactly as
/// [`produce_raw_percept_chunks`] parses them, and how many non-empty lines
/// were not JSON objects and so render nothing.
pub fn parse_raw_percept_records(text: &str) -> (Vec<JsonObject>, usize) {
    let mut unparsed = 0;
    let records = text
        .lines()
        .filter_map(|line| {
            let record = parse_jsonl_line(line)?;
            if record.is_none() {
                unparsed += 1;
            }
            record
        })
        .collect();
    (records, unparsed)
}

/// Render raw screen records for the Screen talent's private input projection.
///
/// The ordinary raw-screen renderer remains the owner/index/display contract.
/// This separate entry point may compact a producer-typed tmux envelope because
/// its output is used only as model input for the Screen talent.
pub fn produce_screen_talent_raw_screen_chunks(rel: &str, text: &str) -> ScreenTalentRawScreen {
    let records = parse_jsonl_objects(text);
    raw_screen::render_for_screen_talent(rel, &records)
}

pub type JsonObject = Map<String, Value>;

pub const PARTIAL_INPUT_NOTICE: &str =
    "Left out: Some input did not fit. This result was written from part of the input.";

fn partial_markers(record: &JsonObject) -> impl Iterator<Item = &Value> {
    [
        record.get("partial_input"),
        record.get("participation_partial_input"),
        record.get("story").and_then(|s| s.get("partial_input")),
    ]
    .into_iter()
    .flatten()
    .filter(|v| v.is_object())
}

pub fn has_partial_input(record: &JsonObject) -> bool {
    partial_markers(record).next().is_some()
}

/// Keep a limitation visible under the same read limit as its saved body.
/// Only partial results reserve space by shortening the rendered projection;
/// their full saved source remains available.
pub fn with_partial_input_notice(text: String, record: &JsonObject) -> String {
    with_partial_input_notice_for_locale(text, record, solstone_core_cli_copy::ENGLISH)
}

/// Render any partial-input notice appended to `text` in `locale`.
pub fn with_partial_input_notice_for_locale(
    text: String,
    record: &JsonObject,
    locale: &str,
) -> String {
    let mut doc = crate::stored_markdown::Document::plain(text);
    let retained = append_partial_input_notice(&mut doc, record);
    if locale == solstone_core_cli_copy::ENGLISH || retained == doc.len() {
        doc.into_text()
    } else {
        let tail = doc.render_from(retained, locale);
        format!("{}{tail}", &doc[..retained])
    }
}

pub fn partial_input_document(
    mut text: crate::stored_markdown::Document,
    record: &JsonObject,
) -> crate::stored_markdown::Document {
    append_partial_input_notice(&mut text, record);
    text
}

/// Return the exact number of original bytes retained before the notice.
fn append_partial_input_notice(
    text: &mut crate::stored_markdown::Document,
    record: &JsonObject,
) -> usize {
    use crate::stored_markdown::Message;
    if !has_partial_input(record) {
        return text.len();
    }
    let limit = [16 * 1024, 64 * 1024]
        .into_iter()
        .find(|limit| text.len() <= *limit)
        .unwrap_or(64 * 1024);
    let note = if record
        .get("story")
        .and_then(|s| s.get("partial_input"))
        .is_some_and(Value::is_object)
    {
        Message::PartialStory
    } else {
        Message::PartialInput
    };
    let body_limit = limit - note.render(solstone_core_cli_copy::ENGLISH).len() - 2;
    if text.len() > body_limit {
        let mut boundary = body_limit;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
    }
    let retained = text.len();
    text.push_str("\n\n");
    text.push_message(note);
    let mut counts = Vec::new();
    for marker in partial_markers(record) {
        for key in ["dropped_entries", "dropped_chars"] {
            if let Some(count) = marker.get(key).and_then(Value::as_u64) {
                counts.push(format!("{key}: {count}"));
            }
        }
    }
    let detail = Message::PartialCounts {
        details: counts.join(", "),
    };
    if !counts.is_empty()
        && text.len() + 1 + detail.render(solstone_core_cli_copy::ENGLISH).len() <= limit
    {
        text.push_str("\n");
        text.push_message(detail);
    }
    retained
}

pub(super) fn recorded_chunk(
    content: String,
    occurrence_time_ms: i64,
    source: &JsonObject,
) -> IndexChunk {
    recorded_chunk_for_locale(
        content,
        occurrence_time_ms,
        source,
        solstone_core_cli_copy::ENGLISH,
    )
}

pub(super) fn recorded_chunk_for_locale(
    content: String,
    occurrence_time_ms: i64,
    source: &JsonObject,
    locale: &str,
) -> IndexChunk {
    IndexChunk {
        content: with_partial_input_notice_for_locale(content, source, locale),
        occurrence_time_ms: Some(OccurrenceTimeMs(occurrence_time_ms)),
        source: Some(source.clone()),
        group_offset_ms: None,
    }
}

/// A JSONL file's objects, as the indexer reads them.
pub fn parse_jsonl_objects(text: &str) -> Vec<JsonObject> {
    text.lines()
        .filter_map(parse_jsonl_line)
        .flatten()
        .collect()
}

/// `None` for a blank line, `Some(None)` for a line that is not a JSON object.
fn parse_jsonl_line(line: &str) -> Option<Option<JsonObject>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Object(record)) => Some(record),
        Ok(_) | Err(_) => None,
    })
}

// Whole-file JSON talent outputs are intentionally infallible. Python writes
// files(path, mtime) after _index_file returns, even on JSONDecodeError; native
// scan.rs writes that row only from index_file's Ok arm. Malformed, empty, or
// non-object content must therefore render zero chunks instead of becoming an
// indexing error, or the files table diverges.
fn parse_json_object(text: &str) -> Vec<JsonObject> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(record)) => vec![record],
        Ok(_) | Err(_) => Vec::new(),
    }
}

fn parse_records_for_family(family: Family, text: &str) -> Vec<JsonObject> {
    match family {
        Family::Documents
        | Family::Screen
        | Family::Sense
        | Family::MorningBriefing
        | Family::Workout
        | Family::PossibleNames => parse_json_object(text),
        Family::AgentMemory => Vec::new(),
        _ => parse_jsonl_objects(text),
    }
}

fn json_falsy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Bool(value)) => !value,
        Some(Value::Number(value)) => value.as_f64() == Some(0.0),
        Some(Value::String(value)) => value.is_empty(),
        Some(Value::Array(value)) => value.is_empty(),
        Some(Value::Object(value)) => value.is_empty(),
    }
}

fn clean_value(value: Option<&Value>) -> String {
    if json_falsy(value) {
        String::new()
    } else {
        value
            .map(display_value)
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

fn json_truthy(value: Option<&Value>) -> bool {
    !json_falsy(value)
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn truthy_display(record: &JsonObject, key: &str) -> Option<String> {
    let value = record.get(key)?;
    if json_falsy(Some(value)) {
        None
    } else {
        Some(display_value(value))
    }
}

fn stripped_truthy_display(record: &JsonObject, key: &str) -> Option<String> {
    let value = record.get(key)?;
    if json_falsy(Some(value)) {
        return None;
    }
    let stripped = display_value(value).trim().to_string();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped)
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut result = String::new();
    result.extend(first.to_uppercase());
    result.push_str(&chars.as_str().to_lowercase());
    result
}

fn titleize(value: &str) -> String {
    value
        .replace('_', " ")
        .split_whitespace()
        .map(capitalize)
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_string(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max_chars).collect();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use glob::{MatchOptions, Pattern};

    use crate::chunker::test_support::{
        markdown_fixture, normalize_tokens, rust_tokenize, strings, token_comparison_enabled,
    };

    use super::*;

    fn produce_chunks(family: Family, rel: &str, text: &str) -> ProducedChunks {
        super::produce_chunks(family, rel, text)
    }

    /// Why a case is allowed to differ from the reference corpus.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Divergence {
        /// This crate is right and the reference was not. The recorded native
        /// value is the contract from here on.
        Accepted,
        /// This crate is wrong. The entry pins today's behaviour so it cannot
        /// drift further while the fix is outstanding, and keeps the difference
        /// visible instead of letting a green gate imply agreement.
        Defect,
    }

    #[derive(Debug)]
    struct DivergenceEntry {
        case: &'static str,
        kind: Divergence,
        native_chunks: &'static [&'static str],
        reason: &'static str,
    }

    /// Differences between this crate and the reference corpus, each one a
    /// decision rather than an accident. A mismatch that is not listed here
    /// fails the conformance test.
    const DIVERGENCES: &[DivergenceEntry] = &[
        DivergenceEntry {
            case: "event_nominal",
            kind: Divergence::Accepted,
            native_chunks: &[
                "### Meeting: Standup\n\n**Participants:** Alice, Bob\n\nDaily sync\n",
            ],
            reason: "reference emits a blank line for an absent header slot; whitespace only, \
                     tokenizes identically, and the native form is the cleaner record",
        },
        DivergenceEntry {
            case: "event_skip_is_title_only",
            kind: Divergence::Accepted,
            native_chunks: &["### Task: Kept\n\n"],
            reason: "same absent-header blank line as event_nominal",
        },
        DivergenceEntry {
            case: "facet_entity_nominal",
            kind: Divergence::Accepted,
            native_chunks: &[
                "### Person: Alice\n\nFriend from work\n\n**Tags:** tech, mentor\n**Also known as:** A, Al\n**Contact:** alice@example.com\n**Roles:** lead, reviewer\n**Empty Note:** \n",
            ],
            reason: "same absent-header blank line as event_nominal",
        },
        DivergenceEntry {
            case: "facet_entity_missing_fields",
            kind: Divergence::Accepted,
            native_chunks: &[
                "### Project: No Description\n\n*(No description available)*\n\n",
                "### Unknown: Unnamed\n\nOnly description\n\n",
            ],
            reason: "same absent-header blank line as event_nominal",
        },
        DivergenceEntry {
            case: "facet_entity_agent_from_slug_stem",
            kind: Divergence::Accepted,
            native_chunks: &["### Person: Slugged\n\n*(No description available)*\n\n"],
            reason: "same absent-header blank line as event_nominal",
        },
        DivergenceEntry {
            case: "activity_degenerate_rows_still_emit",
            kind: Divergence::Accepted,
            native_chunks: &["### Untitled activity", "### X\n- Activity: x"],
            reason: "the placeholder title for an activity with no title is sentence-cased here \
                     and lowercase in the reference; a heading an owner can see, so cased",
        },
        DivergenceEntry {
            case: "activity_nominal",
            kind: Divergence::Accepted,
            native_chunks: &[
                "### Launch sync\n- Activity: meeting\n- Facet: work\n- Day: 20260418\n- Time: 09:00-09:05\n- Last engaged segment level: 0.5\n- Description: Team sync\n- Details: Assigned owners\n- Participation: Mina\n\nAligned on launch.\nTopics: launch, owners\n- Hidden: yes",
            ],
            reason: "the stored level_avg key contains the last engaged segment's level, \
                     so the native label names that value instead of the reference's generic Level",
        },
        DivergenceEntry {
            case: "day_accumulator_nominal",
            kind: Divergence::Accepted,
            native_chunks: &["{\"ts\":1772000000000,\"summary\":\"steady morning\"}"],
            reason: "the accumulator chunk is the record re-serialized; the reference used \
                     json.dumps default separators and this crate emits compact JSON. \
                     Tokenizes identically",
        },
    ];

    fn divergence_for(case: &str) -> Option<&'static DivergenceEntry> {
        DIVERGENCES.iter().find(|entry| entry.case == case)
    }

    const RAW_PERCEPT_NON_RAISE_CASE_COUNT: usize = 15;
    const RAW_PERCEPT_RAISE_AS_ABSENT_CASE_COUNT: usize = 1;
    const RAW_PERCEPT_ERROR_CASES: &[&str] = &[
        "audio_row_missing_start_is_reported",
        "screen_frame_missing_timestamp_is_reported",
        "screen_first_row_without_timestamp_or_raw_is_skipped_not_metadata",
    ];

    fn rewrite_sol_urls(text: &str) -> String {
        const PREFIX: &str = "/app/sol/";
        let mut rewritten = String::new();
        let mut cursor = 0;
        while let Some(offset) = text[cursor..].find(PREFIX) {
            let start = cursor + offset;
            rewritten.push_str(&text[cursor..start]);
            let path_start = start + PREFIX.len();
            let path_end = text[path_start..]
                .find(is_url_delimiter)
                .map_or(text.len(), |offset| path_start + offset);
            let path = &text[path_start..path_end];
            rewritten
                .push_str(&rewrite_sol_path(path).unwrap_or_else(|| format!("{PREFIX}{path}")));
            cursor = path_end;
        }
        rewritten.push_str(&text[cursor..]);
        rewritten
    }

    fn is_url_delimiter(ch: char) -> bool {
        ch.is_whitespace() || matches!(ch, '"' | '\'' | ')' | ']' | '<' | '>')
    }

    fn rewrite_sol_path(path: &str) -> Option<String> {
        if let Some((day, fragment)) = path.split_once('#')
            && day_key(day)
        {
            let parts = fragment.split('/').collect::<Vec<_>>();
            return match parts.as_slice() {
                [talent] if !talent.is_empty() => {
                    Some(format!("/app/thinking/#runs/{day}/{talent}"))
                }
                [talent, use_id] if !talent.is_empty() && !use_id.is_empty() => {
                    Some(format!("/app/thinking/#runs/{day}/{talent}/{use_id}"))
                }
                _ => None,
            };
        }
        if day_key(path) {
            return Some(format!("/app/thinking/#runs/{path}"));
        }
        (!path.is_empty() && !path.contains('/')).then(|| format!("/app/thinking/#runs/run/{path}"))
    }

    fn day_key(value: &str) -> bool {
        value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
    }

    fn compare_corpus_output(
        case: &serde_json::Value,
        id: &str,
        produced: ProducedChunks,
        use_divergences: bool,
        compare_error: bool,
        mismatches: &mut Vec<String>,
    ) {
        let expected = case["chunks"]
            .as_array()
            .expect("case chunks")
            .iter()
            .map(|chunk| rewrite_sol_urls(chunk["markdown"].as_str().expect("chunk markdown")))
            .collect::<Vec<_>>();
        let expected_refs = expected.iter().map(String::as_str).collect::<Vec<_>>();
        let actual: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();
        if actual != expected_refs {
            if use_divergences {
                match divergence_for(id) {
                    Some(entry) if actual == entry.native_chunks => {}
                    Some(entry) => mismatches.push(format!(
                        "{id}: recorded as a {:?} divergence, but the native output has since \
                         moved\n     recorded {:?}\n     actual   {actual:?}",
                        entry.kind, entry.native_chunks
                    )),
                    None => mismatches.push(format!(
                        "{id}: chunks differ and the difference is not recorded in DIVERGENCES\n\
                         \x20    reference {expected:?}\n     actual    {actual:?}"
                    )),
                }
            } else {
                mismatches.push(format!(
                    "{id}: chunks differ\n     reference {expected:?}\n     actual    {actual:?}"
                ));
            }
        }

        let expected_agent = case["agent"].as_str();
        if produced.agent_override.as_deref() != expected_agent {
            mismatches.push(format!(
                "{id}: agent differs — expected {:?}, actual {:?}",
                expected_agent, produced.agent_override
            ));
        }

        let expected_header = case.get("header").and_then(serde_json::Value::as_str);
        if produced.header.as_deref() != expected_header {
            mismatches.push(format!(
                "{id}: header differs — expected {:?}, actual {:?}",
                expected_header, produced.header
            ));
        }
        if compare_error {
            let expected_error = case.get("error").and_then(serde_json::Value::as_str);
            if produced.error.as_deref() != expected_error {
                mismatches.push(format!(
                    "{id}: error differs — expected {:?}, actual {:?}",
                    expected_error, produced.error
                ));
            }
        }

        for (index, (expected, actual)) in case["chunks"]
            .as_array()
            .expect("case chunks")
            .iter()
            .zip(&produced.chunks)
            .enumerate()
        {
            let expected_time = expected
                .get("occurrence_time_ms")
                .and_then(serde_json::Value::as_i64)
                .map(OccurrenceTimeMs);
            if actual.occurrence_time_ms != expected_time {
                mismatches.push(format!(
                    "{id}:{index}: occurrence time differs — expected {expected_time:?}, actual {:?}",
                    actual.occurrence_time_ms
                ));
            }
            let expected_source = expected
                .get("source")
                .and_then(serde_json::Value::as_object);
            if actual.source.as_ref() != expected_source {
                mismatches.push(format!(
                    "{id}:{index}: source differs — expected {expected_source:?}, actual {:?}",
                    actual.source
                ));
            }
        }
    }

    #[test]
    fn a_hidden_activity_keeps_its_position_and_leaves_the_index() {
        let rel = "facets/work/activities/20260301.jsonl";
        let text = concat!(
            r#"{"id":"a","title":"Alpha","created_at":1}"#,
            "\n",
            r#"{"id":"b","title":"Bravo","created_at":2,"hidden":true}"#,
            "\n",
            r#"{"id":"c","title":"Charlie","created_at":3}"#,
            "\n",
        );
        let produced = produce_chunks(Family::Activity, rel, text);
        let kept: Vec<bool> = produced
            .chunks
            .iter()
            .map(|chunk| indexed(Family::Activity, chunk))
            .collect();
        assert_eq!(kept, [true, false, true]);
        assert!(produced.chunks[2].content.contains("Charlie"));
        // The renderer still marks the hidden entry for its other readers.
        assert!(produced.chunks[1].content.contains("Bravo"));
    }

    #[test]
    fn only_an_activity_chunk_is_judged_by_its_hidden_source() {
        let source = serde_json::json!({"name": "Bob", "hidden": true})
            .as_object()
            .unwrap()
            .clone();
        let chunk = IndexChunk {
            content: "Bob".to_owned(),
            occurrence_time_ms: None,
            source: Some(source),
            group_offset_ms: None,
        };
        assert!(!indexed(Family::Activity, &chunk));
        for family in [Family::FacetEntity, Family::Observation, Family::Event] {
            assert!(indexed(family, &chunk), "{family:?}");
        }
        let empty = IndexChunk {
            content: " \n".to_owned(),
            source: None,
            ..chunk
        };
        assert!(!indexed(Family::FacetEntity, &empty));
    }

    #[test]
    fn partial_inputs_stay_visible_at_read_limits_and_in_empty_projections() {
        for size in [
            16 * 1024 - 1,
            16 * 1024,
            64 * 1024 - 1,
            64 * 1024,
            70 * 1024,
        ] {
            let record =
                serde_json::json!({"partial_input":{"dropped_entries":2,"dropped_chars":500}})
                    .as_object()
                    .unwrap()
                    .clone();
            let body = "界".repeat(size / 3);
            let text = super::with_partial_input_notice(body, &record);
            assert!(text.contains(super::PARTIAL_INPUT_NOTICE));
            assert!(
                text.len()
                    <= if size <= 16 * 1024 {
                        16 * 1024
                    } else {
                        64 * 1024
                    }
            );
        }
        for family in [Family::Sense, Family::Screen, Family::MorningBriefing] {
            let record = serde_json::json!({"partial_input":{}})
                .as_object()
                .unwrap()
                .clone();
            let produced = super::produce_chunks_by_shape(family, None, &[record]);
            assert_eq!(produced.chunks.len(), 1);
            assert!(produced.chunks[0].content.contains("Left out:"));
        }
    }

    /// Every family renders what the reference implementation recorded.
    ///
    /// The corpus is generated by `scripts/content_family_corpus.py` and is a
    /// frozen record: it cannot be regenerated once the reference tree stops
    /// being runnable, so a failure here is a question about this crate, never a
    /// prompt to re-derive the expectation.
    ///
    /// Cases with `raises` or a null `family` that do not carry `pending_family`
    /// are skipped by this assertion and covered by
    /// `content_family_corpus_records_known_divergences`. Cases carrying
    /// `pending_family` are rendered through `produce_raw_percept_chunks`: fifteen
    /// are compared in full, while the one reference-raising case is asserted
    /// against its documented as-if-content-absent behaviour.
    #[test]
    fn every_family_matches_the_reference_corpus() {
        let fixture_source = include_str!("../../../../fixtures/content_families.json");
        let legacy_url_count = fixture_source.matches("/app/sol/").count();
        assert_eq!(
            legacy_url_count, 1,
            "content families legacy Sol URL count: expected 1, actual {legacy_url_count}"
        );
        let fixture: serde_json::Value =
            serde_json::from_str(fixture_source).expect("content family fixture parses");

        let cases = fixture["cases"].as_array().expect("fixture cases");
        assert!(!cases.is_empty(), "corpus is empty — nothing was compared");

        let mut compared = 0usize;
        let mut raw_percept_compared = 0usize;
        let mut raw_percept_raised = 0usize;
        let mut mismatches: Vec<String> = Vec::new();

        for case in cases {
            let id = case["id"].as_str().expect("case id");
            if let Some(name) = case
                .get("pending_family")
                .and_then(serde_json::Value::as_str)
            {
                let family = match parse_shape_name(name) {
                    Some(ContentResolution::Unindexed(family)) => family,
                    other => panic!("{id}: unknown raw percept family {name} ({other:?})"),
                };
                let rel = case["rel"].as_str().expect("case rel");
                let text = case["input_text"].as_str().expect("case input_text");
                let produced = produce_raw_percept_chunks(family, rel, text);
                if case.get("raises").is_some() {
                    assert_eq!(id, "screen_null_content_raises_in_the_reference");
                    assert_eq!(produced.agent_override.as_deref(), Some("screen"));
                    assert_eq!(produced.header.as_deref(), Some("# Frame Analyses"));
                    assert_eq!(produced.error, None);
                    assert_eq!(produced.chunks.len(), 1);
                    assert_eq!(produced.chunks[0].content, "### 09:00:03\n");
                    assert_eq!(
                        produced.chunks[0].occurrence_time_ms,
                        Some(OccurrenceTimeMs(1_772_614_803_000))
                    );
                    assert_eq!(
                        produced.chunks[0].source.as_ref(),
                        case["input"][1].as_object()
                    );
                    raw_percept_raised += 1;
                    continue;
                }
                let expects_error = RAW_PERCEPT_ERROR_CASES.contains(&id);
                assert_eq!(
                    case.get("error")
                        .and_then(serde_json::Value::as_str)
                        .is_some(),
                    expects_error,
                    "{id}: raw-percept corpus error classification"
                );
                assert_eq!(
                    produced.error.is_some(),
                    expects_error,
                    "{id}: raw-percept produced error classification"
                );
                compare_corpus_output(case, id, produced, false, true, &mut mismatches);
                raw_percept_compared += 1;
                continue;
            }
            if case.get("raises").is_some() {
                continue;
            }
            let Some(name) = case["family"].as_str() else {
                continue;
            };
            let family = match parse_shape_name(name) {
                Some(ContentResolution::Indexed(family)) => family,
                other => panic!("{id}: unknown family {name} ({other:?})"),
            };
            let rel = case["rel"].as_str().expect("case rel");
            let text = case["input_text"].as_str().expect("case input_text");
            let produced = super::produce_chunks(family, rel, text);
            compared += 1;
            compare_corpus_output(case, id, produced, true, false, &mut mismatches);
        }

        assert!(compared > 0, "no cases were compared");
        assert_eq!(
            raw_percept_compared, RAW_PERCEPT_NON_RAISE_CASE_COUNT,
            "raw-percept non-raise cases compared"
        );
        assert_eq!(
            raw_percept_raised, RAW_PERCEPT_RAISE_AS_ABSENT_CASE_COUNT,
            "raw-percept raises cases safely handled"
        );
        assert!(
            mismatches.is_empty(),
            "{} of {compared} compared cases diverge from the reference:\n  - {}",
            mismatches.len(),
            mismatches.join("\n  - ")
        );
    }

    /// Outstanding defects, stated rather than implied.
    ///
    /// The conformance test is green while a `Defect` divergence stands, because
    /// its job is to catch *drift*. This one exists so the green does not read as
    /// "this crate matches the reference" when part of it knowingly does not.
    /// Closing a defect means deleting its entry, not editing this count.
    #[test]
    fn outstanding_content_divergences_are_declared() {
        let defects: Vec<&DivergenceEntry> = DIVERGENCES
            .iter()
            .filter(|entry| entry.kind == Divergence::Defect)
            .collect();

        for entry in &defects {
            assert!(
                entry.reason.len() > 40,
                "{}: a defect divergence needs a reason that explains the owner-visible \
                 consequence, not a label",
                entry.case
            );
        }

        assert!(defects.is_empty(), "outstanding defects: {defects:?}");
    }

    /// The corpus carries exceptional cases whose notes are load-bearing, whether
    /// they remain skipped or are now rendered through `pending_family`. This
    /// asserts they stay present and stay described, so removing one is a decision
    /// rather than an accident.
    #[test]
    fn content_family_corpus_records_known_divergences() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/content_families.json"))
                .expect("content family fixture parses");
        let cases = fixture["cases"].as_array().expect("fixture cases");

        let divergences: Vec<&str> = cases
            .iter()
            .filter(|case| case.get("raises").is_some() || case["family"].is_null())
            .map(|case| case["id"].as_str().expect("case id"))
            .collect();

        assert!(
            !divergences.is_empty(),
            "the corpus records no divergences — either the generator regressed or \
             every difference was closed, and the second one needs saying out loud"
        );
        for case in cases
            .iter()
            .filter(|case| case.get("raises").is_some() || case["family"].is_null())
        {
            let id = case["id"].as_str().expect("case id");
            assert!(
                case.get("note")
                    .and_then(serde_json::Value::as_str)
                    .is_some(),
                "{id}: a recorded divergence must carry a note saying why"
            );
        }
    }

    #[test]
    fn classifies_indexable_families() {
        for (path, family) in [
            ("20240101/talents/flow.md", Family::Markdown),
            ("20260304/talents/pulse.jsonl", Family::DayAccumulator),
            (
                "20260304/default/090000_300/talents/sense.json",
                Family::Sense,
            ),
            (
                "20260304/default/090000_300/talents/documents.json",
                Family::Documents,
            ),
            (
                "20260304/default/090000_300/talents/screen.json",
                Family::Screen,
            ),
            (
                "20260304/talents/morning_briefing.json",
                Family::MorningBriefing,
            ),
            (
                "20260101/import.ics/imported.jsonl",
                Family::StructuredImport,
            ),
            (
                "20260101/import.claude/thread_a/conversation_transcript.jsonl",
                Family::AiChat,
            ),
            (
                "20260818/import.text/065323_5/conversation_transcript.jsonl",
                Family::AiChat,
            ),
            (
                "20260703/suze.browser/000141_317/browser_mail-google-com.jsonl",
                Family::Browser,
            ),
            ("facets/work/events/20240101.jsonl", Family::Event),
            ("facets/work/activities/20240101.jsonl", Family::Activity),
            ("facets/work/logs/20240101.jsonl", Family::ActionLog),
            ("facets/work/entities/foo.jsonl", Family::FacetEntity),
            (
                "facets/work/entities/alice/observations.jsonl",
                Family::Observation,
            ),
        ] {
            assert_eq!(classify(path), ContentResolution::Indexed(family), "{path}");
        }
    }

    #[test]
    fn agent_memory_pattern_admits_only_the_note_path_and_emits_literal_text() {
        let rel = format!(
            "20240101/agent-memory-{}/120000_60/note.txt",
            "a".repeat(64)
        );
        assert_eq!(
            classify(&rel),
            ContentResolution::Indexed(Family::AgentMemory)
        );
        assert_eq!(
            classify("20240101/talents/memory.md"),
            ContentResolution::Indexed(Family::Markdown)
        );
        assert_eq!(
            classify(&rel.replace("note.txt", "origin.json")),
            ContentResolution::Unrecognized
        );
        let uppercase = format!(
            "20240101/agent-memory-{}/120000_60/note.txt",
            "A".repeat(64)
        );
        assert_eq!(classify(&uppercase), ContentResolution::Unrecognized);

        let produced = produce_chunks(
            Family::AgentMemory,
            &rel,
            "# authored heading\n\nplain note text",
        );
        assert_eq!(produced.agent_override.as_deref(), Some("memory"));
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(
            produced.chunks[0].content,
            "# authored heading\n\nplain note text"
        );
        assert!(
            produce_chunks(Family::AgentMemory, &rel, "  \n ")
                .chunks
                .is_empty()
        );
    }

    #[test]
    fn classifies_non_indexed_and_unrecognized_paths() {
        assert_eq!(
            classify("20240101/default/123456_300/audio.jsonl"),
            ContentResolution::Indexed(Family::Audio)
        );
        assert_eq!(
            classify("20240101/default/123456_300/screen.jsonl"),
            ContentResolution::Unindexed(RawPerceptFamily::RawScreen)
        );
        assert_eq!(
            classify("entities/alice/entity.json"),
            ContentResolution::IndexedElsewhere
        );
        assert_eq!(
            classify("reflections/weekly/20260308.json"),
            ContentResolution::IndexedElsewhere
        );
        assert_eq!(classify("notes/foo.txt"), ContentResolution::Unrecognized);
    }

    #[test]
    fn structural_content_patterns_take_priority_over_day_rooted_patterns() {
        for (path, family) in [
            ("facets/work/logs/browser_mail.jsonl", Family::ActionLog),
            ("facets/work/events/browser_x.jsonl", Family::Event),
            ("facets/work/activities/browser_a.jsonl", Family::Activity),
            ("facets/chat/logs/chat.jsonl", Family::ActionLog),
            (
                "facets/import.claude/logs/conversation_transcript.jsonl",
                Family::ActionLog,
            ),
        ] {
            assert_eq!(classify(path), ContentResolution::Indexed(family), "{path}");
        }
    }

    /// An indexed family wins over a known-unindexed pattern that also matches.
    ///
    /// The three AI-chat legacy filenames match their own family pattern *and*
    /// `*/*/*/*_audio.jsonl`, and both are day-rooted, so namespace precedence
    /// cannot separate them. The first assertion in each iteration proves the
    /// collision is real rather than assumed — without it this test could pass
    /// against patterns that never overlap and prove nothing.
    ///
    /// Why it is worth pinning: an unindexed resolution is `continue`d by the
    /// scan with no warning and no skipped count, so if these three ever
    /// resolved unindexed, every imported ChatGPT, Claude and Gemini transcript
    /// would stop being searchable while the scan still reported success.
    ///
    /// ⚠ This pins the outcome, not a mechanism. Swapping the order in which
    /// `classify` consults the two tables does **not** break it — that was
    /// tried, and the full suite stayed green — so whatever decides this case
    /// is not table order alone. Do not read the test as guarding an ordering.
    ///
    /// The final assertion is the inverted twin: a path matching *only* the
    /// unindexed pattern must still resolve unindexed, so this cannot be
    /// satisfied by a resolver that never consults the unindexed table at all.
    #[test]
    fn an_indexed_family_wins_over_a_matching_unindexed_pattern() {
        let options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        for path in [
            "20260101/import.chatgpt/conv_b/imported_audio.jsonl",
            "20260101/import.claude/conv_b/imported_audio.jsonl",
            "20260101/import.gemini/conv_b/imported_audio.jsonl",
        ] {
            assert!(
                Pattern::new("*/*/*/*_audio.jsonl")
                    .expect("valid unindexed pattern")
                    .matches_path_with(Path::new(path), options),
                "{path} must genuinely collide, or this test proves nothing"
            );
            assert_eq!(
                classify(path),
                ContentResolution::Indexed(Family::AiChat),
                "{path}"
            );
        }

        let imported_text = "20260818/import.text/065323_5/conversation_transcript.jsonl";
        assert!(
            Pattern::new("*/*/*/*_transcript.jsonl")
                .expect("valid unindexed pattern")
                .matches_path_with(Path::new(imported_text), options),
            "{imported_text} must genuinely collide, or this test proves nothing"
        );
        assert_eq!(
            classify(imported_text),
            ContentResolution::Indexed(Family::AiChat),
            "{imported_text}"
        );

        assert_eq!(
            classify("20260101/default/123456_300/left_audio.jsonl"),
            ContentResolution::Indexed(Family::Audio),
        );
        assert_eq!(
            classify("20260101/default/123456_300/left_screen.jsonl"),
            ContentResolution::Unindexed(RawPerceptFamily::RawScreen),
        );
    }

    #[test]
    fn every_cross_namespace_intersection_has_the_expected_resolution() {
        let structural_json = [
            ("events", Family::Event),
            ("entities", Family::FacetEntity),
            ("activities", Family::Activity),
            ("logs", Family::ActionLog),
        ];
        let day_json = [
            "facets/chat/{structural}/chat.jsonl",
            "facets/x/{structural}/browser_x.jsonl",
            "facets/import.chatgpt/{structural}/conversation_transcript.jsonl",
            "facets/import.claude/{structural}/conversation_transcript.jsonl",
            "facets/import.gemini/{structural}/conversation_transcript.jsonl",
            "facets/import.chatgpt/{structural}/imported_audio.jsonl",
            "facets/import.claude/{structural}/imported_audio.jsonl",
            "facets/import.gemini/{structural}/imported_audio.jsonl",
        ];

        let mut json_intersections = 0;
        for (structural, family) in structural_json {
            for day_pattern in day_json {
                let path = day_pattern.replace("{structural}", structural);
                assert_eq!(
                    classify(&path),
                    ContentResolution::Indexed(family),
                    "structural JSON pattern must win for {path}"
                );
                json_intersections += 1;
            }
        }
        assert_eq!(json_intersections, 32);

        // Pre-existing overlaps. Both roots already classify these as Markdown,
        // so root-first matching must preserve their shared outcome.
        let markdown_intersections = [
            "facets/x/activities/talents/x/x.md",
            "imports/talents/summary.md",
            "20260101/import.appleXhealth/090000_300/imported.md",
            "20260101/import.chatgpt/conv_b/x_transcript.md",
        ];
        for path in markdown_intersections {
            assert_eq!(
                classify(path),
                ContentResolution::Indexed(Family::Markdown),
                "pre-existing Markdown overlap changed for {path}"
            );
        }
        assert_eq!(markdown_intersections.len(), 4);

        for path in [
            "facets/import.x/news/x_transcript.md",
            "facets/import.x/news/imported.md",
            "facets/work/news/20260101.md",
        ] {
            assert_eq!(
                classify(path),
                ContentResolution::Unrecognized,
                "{path} must be Unrecognized"
            );
        }
    }

    #[test]
    fn registered_patterns_compile_and_cover_every_family() {
        for family in ALL_FAMILIES {
            assert!(
                INDEX_FAMILY_PATTERNS
                    .iter()
                    .any(|pattern| pattern.classification == ContentResolution::Indexed(family)),
                "{family:?} has no registered content pattern"
            );
        }
        for pattern in INDEX_FAMILY_PATTERNS {
            Pattern::new(pattern.pattern).expect("index family pattern compiles");
        }
    }

    #[test]
    fn markdown_producer_matches_python_oracle_tokens() {
        let fixture = markdown_fixture();
        for case in fixture["cases"].as_array().expect("fixture cases") {
            if !token_comparison_enabled(case) {
                continue;
            }
            let id = case["id"].as_str().expect("case id");
            let input = case["input"].as_str().expect("case input");
            let produced = produce_chunks(Family::Markdown, "20240101/talents/oracle.md", input);
            assert_eq!(
                produced
                    .warnings
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                strings(&case["warnings"]),
                "{id} warnings"
            );
            assert_eq!(
                produced.chunks.len(),
                case["chunk_count"].as_u64().expect("chunk count") as usize,
                "{id} chunk count"
            );
            for (idx, expected_chunk) in case["chunks"]
                .as_array()
                .expect("case chunks")
                .iter()
                .enumerate()
            {
                let normalizations = strings(&expected_chunk["normalizations"]);
                let recorded_tokens = strings(&expected_chunk["tokens"]);
                assert_eq!(
                    normalize_tokens(
                        rust_tokenize(&produced.chunks[idx].content),
                        &normalizations
                    ),
                    recorded_tokens,
                    "{id}:{idx} producer tokens"
                );
            }
            assert_eq!(produced.agent_override, None, "{id} agent override");
        }
    }

    #[test]
    fn markdown_producer_ports_grouping_rules() {
        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Tasks\n\nintro alpha\n\n- item one\n- item two\n",
        );
        assert_eq!(produced.chunks.len(), 2);
        assert_eq!(
            rust_tokenize(&produced.chunks[0].content),
            ["tasks", "intro", "alpha", "item", "one"]
        );
        assert_eq!(
            rust_tokenize(&produced.chunks[1].content),
            ["tasks", "intro", "alpha", "item", "two"]
        );
        assert!(
            !produced
                .chunks
                .iter()
                .any(|chunk| chunk.content.trim() == "# Tasks\n\nintro alpha")
        );

        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Tasks\n\n- item one\n- item two\n",
        );
        assert_eq!(produced.chunks.len(), 2);
        assert_eq!(
            rust_tokenize(&produced.chunks[0].content),
            ["tasks", "item", "one"]
        );
        assert_eq!(
            rust_tokenize(&produced.chunks[1].content),
            ["tasks", "item", "two"]
        );

        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Definitions\n\n- **alpha:** value one\n- ordinary note.\n- **beta:** value two\n- ordinary other.\n",
        );
        assert_eq!(produced.chunks.len(), 1);
        assert_eq!(
            rust_tokenize(&produced.chunks[0].content),
            [
                "definitions",
                "alpha",
                "value",
                "one",
                "ordinary",
                "note",
                "beta",
                "value",
                "two",
                "ordinary",
                "other"
            ]
        );
        assert_eq!(produced.agent_override, None);
        assert!(produced.warnings.is_empty());
    }

    #[test]
    fn markdown_producer_ports_table_and_heading_rules() {
        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Root\n\n## Matrix\n\nintro alpha\n\n| Name | Value |\n| --- | --- |\n| beta | one |\n| gamma | two |\n",
        );
        assert_eq!(produced.chunks.len(), 2);
        assert_eq!(
            rust_tokenize(&produced.chunks[0].content),
            [
                "root", "matrix", "intro", "alpha", "name", "value", "beta", "one"
            ]
        );
        assert_eq!(
            rust_tokenize(&produced.chunks[1].content),
            [
                "root", "matrix", "intro", "alpha", "name", "value", "gamma", "two"
            ]
        );

        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            "# Root\n\n## Empty\n\n| Name | Value |\n| --- | --- |\n",
        );
        assert!(produced.chunks.is_empty());
    }

    #[test]
    fn markdown_producer_keeps_words_beyond_the_old_line_and_chunk_limits() {
        let paragraph = format!("{}lineend", "ordinary ".repeat(300));
        let line = "alpha ".repeat(300);
        let produced = produce_chunks(
            Family::Markdown,
            "20240101/talents/flow.md",
            &format!("# Long\n\n{paragraph}\n\n{line}\n{line}\n{line}blockend"),
        );
        assert!(produced.warnings.is_empty());
        assert_eq!(produced.chunks.len(), 3);
        assert!(produced.chunks[0].content.ends_with("lineend"));
        assert!(produced.chunks[2].content.ends_with("blockend"));
    }

    #[test]
    fn retired_schedule_plans_among_events_are_not_indexed() {
        let text = concat!(
            "{\"title\":\"Q-Day\",\"type\":\"deadline\",\"agent\":\"schedule\",\"occurred\":false}\n",
            "{\"title\":\"Launch\",\"type\":\"milestone\",\"agent\":\"timeline\",\"occurred\":false}\n",
            "{\"title\":\"Standup\",\"type\":\"meeting\",\"agent\":\"schedule\",\"occurred\":true}\n",
        );
        let produced = produce_chunks(Family::Event, "facets/work/events/20290101.jsonl", text);
        let contents = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(contents.len(), 2, "{contents:?}");
        assert!(contents[0].contains("### Planned Milestone: Launch"));
        assert!(contents[1].contains("### Meeting: Standup"));
        assert!(!contents.iter().any(|content| content.contains("Q-Day")));

        // A file holding only plans renders nothing, partial input included.
        let only_plan = "{\"title\":\"Q-Day\",\"agent\":\"schedule\",\"occurred\":false,\"partial_input\":{\"reason\":\"clipped\"}}\n";
        let produced = produce_chunks(
            Family::Event,
            "facets/work/events/20290101.jsonl",
            only_plan,
        );
        assert!(produced.chunks.is_empty(), "{:?}", produced.chunks);
    }

    #[test]
    fn jsonl_parser_skips_malformed_and_non_object_lines_for_all_jsonl_families() {
        let text = r#"
{"title":"Planning","type":"meeting"}
42
["not", "object"]
not json
{"title":"Review","type":"task"}
"#;
        let produced = produce_chunks(Family::Event, "facets/work/events/20240101.jsonl", text);
        assert_eq!(produced.chunks.len(), 2);
        assert!(produced.chunks[0].content.contains("Meeting: Planning"));
        assert!(produced.chunks[1].content.contains("Task: Review"));

        let produced = produce_chunks(
            Family::ActionLog,
            "config/actions/20240101.jsonl",
            r#"
42
not json
{"action":"identity_update","actor":"settings"}
"#,
        );
        assert_eq!(produced.chunks.len(), 1);
        assert!(
            produced.chunks[0]
                .content
                .contains("Identity Update by settings")
        );

        let produced = produce_chunks(
            Family::Activity,
            "facets/work/activities/20240101.jsonl",
            r#"
42
not json
{"id":"coding_090000_300"}
"#,
        );
        assert_eq!(produced.chunks.len(), 1);
        assert!(produced.chunks[0].content.contains("### Coding 090000 300"));
    }

    #[test]
    fn json_object_parser_is_infallible_for_talent_json_inputs() {
        assert_eq!(parse_json_object(r#"{"title":"Planning"}"#).len(), 1);
        for text in ["", "   ", "not json", "null", "42", r#""string""#, "[]"] {
            assert!(parse_json_object(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn text_wrappers_delegate_to_by_shape_renderers() {
        let event_text = r#"{"title":"Planning","type":"meeting"}"#;
        let event_records = parse_jsonl_objects(event_text);
        assert_eq!(
            super::produce_chunks(
                Family::Event,
                "facets/work/events/20260101.jsonl",
                event_text,
            ),
            produce_chunks_by_shape(
                Family::Event,
                Some("facets/work/events/20260101.jsonl"),
                &event_records,
            )
        );

        let screen_text = r#"{"timestamp":3,"content":{}}"#;
        let screen_records = parse_jsonl_objects(screen_text);
        assert_eq!(
            super::produce_raw_percept_chunks(
                RawPerceptFamily::RawScreen,
                "20260304/workstation/090000_300/screen.jsonl",
                screen_text,
            ),
            produce_raw_percept_chunks_by_shape(
                RawPerceptFamily::RawScreen,
                Some("20260304/workstation/090000_300/screen.jsonl"),
                &screen_records,
            )
        );
    }

    #[test]
    fn by_shape_activity_without_path_uses_the_pathless_header() {
        let records = parse_jsonl_objects(r#"{"title":"Planning"}"#);
        let produced = produce_chunks_by_shape(Family::Activity, None, &records);
        assert_eq!(produced.header.as_deref(), Some("# Activities"));
    }

    #[test]
    fn by_shape_raw_screen_uses_synthetic_records_with_a_real_path() {
        let records = parse_jsonl_objects(r#"{"timestamp":3,"content":{}}"#);
        let produced = produce_raw_percept_chunks_by_shape(
            RawPerceptFamily::RawScreen,
            Some("20260304/workstation/090000_300/screen.jsonl"),
            &records,
        );
        assert_eq!(produced.header.as_deref(), Some("# Frame Analyses"));
        assert_eq!(produced.chunks.len(), 1);
    }

    #[test]
    fn structured_import_agent_preserves_source_case_until_merge() {
        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            "",
        );
        assert_eq!(produced.agent_override, None);
        assert_eq!(produced.chunks.len(), 0);

        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"import":{"source":"ics"}}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.ics"));

        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"import":{"source":"ICS"}}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.ICS"));

        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"entry_count":1}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.unknown"));
    }

    #[test]
    fn structured_import_skips_header_and_empty_generic_entries() {
        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"import":{"source":"ics"},"title":"Header"}
{"type":"generic"}"#,
        );
        assert_eq!(produced.chunks.len(), 0);

        let produced = produce_chunks(
            Family::StructuredImport,
            "20260101/import.ics/imported.jsonl",
            r#"{"import":{"source":"ics"}}
{"type":"calendar_event","title":"Quarterly Planning","ts":"2026-01-01T09:30:00-07:00"}"#,
        );
        assert_eq!(produced.chunks.len(), 1);
        assert!(produced.chunks[0].content.contains("Quarterly Planning"));
    }

    #[test]
    fn ai_chat_agent_comes_from_path_or_fallback() {
        let produced = produce_chunks(
            Family::AiChat,
            "20260101/import.claude/thread_a/conversation_transcript.jsonl",
            r#"{"model":"claude-3"}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.claude"));

        let produced = produce_chunks(
            Family::AiChat,
            "20260101/misc/thread_a/conversation_transcript.jsonl",
            r#"{"model":"claude-3"}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("import.ai_chat"));
    }

    #[test]
    fn ai_chat_indexes_only_non_empty_start_bearing_turns() {
        let produced = produce_chunks(
            Family::AiChat,
            "20260101/import.claude/thread_a/conversation_transcript.jsonl",
            r#"{"model":"claude-3","imported":{"facet":"work"}}
{"start":"00:00:01","speaker":"User","text":"Hello"}
{"start":"00:00:02","speaker":"Assistant","text":""}
{"start":"00:00:03","speaker":"Assistant","text":"Hi there"}
{"speaker":"System","text":"metadata-like"}"#,
        );
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();
        assert_eq!(contents, vec!["**User:** Hello", "**Assistant:** Hi there"]);

        let produced = produce_chunks(
            Family::AiChat,
            "20260101/import.claude/thread_a/conversation_transcript.jsonl",
            r#"{"model":"claude-3"}
{"start":"00:00:01","speaker":"User","text":""}"#,
        );
        assert_eq!(produced.chunks.len(), 0);
    }

    #[test]
    fn event_skip_predicate_is_title_only() {
        let produced = produce_chunks(
            Family::Event,
            "facets/work/events/20240101.jsonl",
            r#"{"type":"meeting"}
{"title":"","type":"meeting"}
{"title":"Standup","type":"meeting","participants":["Alice","Bob"],"summary":"Daily sync"}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("event"));
        assert_eq!(produced.chunks.len(), 1);
        assert!(produced.chunks[0].content.contains("### Meeting: Standup"));
        assert!(
            produced.chunks[0]
                .content
                .contains("**Participants:** Alice, Bob")
        );
        assert!(produced.chunks[0].content.contains("Daily sync"));
    }

    #[test]
    fn action_log_skip_predicate_is_action_only() {
        let produced = produce_chunks(
            Family::ActionLog,
            "config/actions/20240101.jsonl",
            r#"{"actor":"settings"}
{"action":"","actor":"settings"}
{"action":"identity_update","actor":"settings","source":"app","timestamp":"2025-12-16T07:33:05.135587+00:00","use_id":"123","params":{"name":"Alice"}}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("action"));
        assert_eq!(produced.chunks.len(), 1);
        assert!(
            produced.chunks[0]
                .content
                .contains("### Identity Update by settings")
        );
        assert!(
            produced.chunks[0]
                .content
                .contains("**Source:** app | **Time:** 07:33:05")
        );
        assert!(
            produced.chunks[0]
                .content
                .contains("**Talent:** [123](/app/thinking/#runs/run/123)")
        );
        assert!(produced.chunks[0].content.contains("- name: Alice"));
    }

    #[test]
    fn activity_objects_always_produce_chunks() {
        let produced = produce_chunks(
            Family::Activity,
            "facets/work/activities/20240101.jsonl",
            r#"{}
{"id":"x"}
{"title":"Launch sync","activity":"meeting","facet":"work","day":"20260418","segments":["090000_300"],"level_avg":0.5,"description":"Team sync","details":"Assigned owners","participation":[{"name":"Mina"}],"story":{"body":"Aligned on launch.","topics":["launch","owners"]},"hidden":true}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("activity"));
        assert_eq!(produced.chunks.len(), 3);
        assert!(produced.chunks[0].content.contains("### Untitled activity"));
        assert!(produced.chunks[1].content.contains("### X"));
        assert!(produced.chunks[2].content.contains("### Launch sync"));
        assert!(produced.chunks[2].content.contains("- Time: 09:00-09:05"));
        assert!(produced.chunks[2].content.contains("- Participation: Mina"));
        assert!(produced.chunks[2].content.contains("Aligned on launch."));
        assert!(
            produced.chunks[2]
                .content
                .contains("Topics: launch, owners")
        );
        assert!(produced.chunks[2].content.contains("- Hidden: yes"));
    }

    #[test]
    fn facet_entity_agent_comes_from_ascii_digit_file_stem() {
        for (rel, agent) in [
            ("facets/work/entities/20260304.jsonl", "entity:detected"),
            ("facets/work/entities/123.jsonl", "entity:detected"),
            ("facets/work/entities/99999999.jsonl", "entity:detected"),
            ("facets/work/entities/some-slug.jsonl", "entity:attached"),
        ] {
            let produced = produce_chunks(Family::FacetEntity, rel, "");
            assert_eq!(produced.agent_override.as_deref(), Some(agent), "{rel}");
            assert_eq!(produced.chunks.len(), 0, "{rel}");
        }
    }

    #[test]
    fn facet_entity_renderer_formats_entity_fields() {
        let produced = produce_chunks(
            Family::FacetEntity,
            "facets/work/entities/20260304.jsonl",
            r#"{"id":"alice","type":"Person","name":"Alice","description":"Friend from work","tags":["tech","mentor"],"aka":["A","Al"],"contact":"alice@example.com","roles":["lead","reviewer"],"empty_note":"","last_seen":"20260304","detached":true}
{"type":"Project","name":"No Description","description":""}
{"description":"Only description"}"#,
        );

        assert_eq!(produced.agent_override.as_deref(), Some("entity:detected"));
        assert_eq!(produced.chunks.len(), 3);
        let first = &produced.chunks[0].content;
        assert!(first.contains("### Person: Alice"));
        assert!(first.contains("Friend from work"));
        assert!(first.contains("**Tags:** tech, mentor"));
        assert!(first.contains("**Also known as:** A, Al"));
        assert!(first.contains("**Contact:** alice@example.com"));
        assert!(first.contains("**Roles:** lead, reviewer"));
        assert!(first.contains("**Empty Note:** "));
        assert!(!first.contains("Last Seen"));
        assert!(!first.contains("Detached"));
        assert!(
            produced.chunks[1]
                .content
                .contains("*(No description available)*")
        );
        assert!(produced.chunks[2].content.contains("### Unknown: Unnamed"));
    }

    #[test]
    fn observation_renderer_formats_source_day_when_truthy() {
        let produced = produce_chunks(
            Family::Observation,
            "facets/work/entities/alice/observations.jsonl",
            r#"{"content":"Prefers morning meetings","source_day":"20250113"}
{"content":"Expert in distributed systems"}
{"source_day":"20250114"}
{"content":"","source_day":""}"#,
        );

        assert_eq!(produced.agent_override.as_deref(), Some("observation"));
        let contents: Vec<&str> = produced
            .chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect();
        assert_eq!(
            contents,
            vec![
                "- Prefers morning meetings (observed: 20250113)",
                "- Expert in distributed systems",
                "-  (observed: 20250114)",
                "- ",
            ]
        );
    }

    #[test]
    fn markdown_disposition_is_keyed_by_matched_pattern() {
        assert_eq!(
            resolve_spec("20260107/talents/day.md").unwrap().disposition,
            IndexDisposition::Admitted {
                category: AdmittedCategory::Facets,
                basis: ScopeBasis::JournalWide,
            }
        );
        assert_eq!(
            resolve_spec("20260107/default/123456_300/talents/day.md")
                .unwrap()
                .disposition,
            IndexDisposition::Admitted {
                category: AdmittedCategory::Transcripts,
                basis: ScopeBasis::SegmentAssigned,
            }
        );
    }
}
