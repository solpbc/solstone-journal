// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::kinds::Kind;
use super::{
    ContentResolution, Family, FamilyPattern, IndexDisposition, PatternRoot, RawPerceptFamily,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindAssignment {
    Settled(Kind),
    Legacy(Kind),
    /// The kind of the material named in the import manifest.
    ImportSummary,
    NotContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentUnit {
    Segment,
    Event,
    Window,
    Stay,
    Day,
    Record,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Content(Family),
    Raw(RawPerceptFamily),
    IndexedElsewhere,
    Place,
    PhotoDescription,
    Original,
    NotContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCondition {
    Path,
    ImageHeader,
    TranscribeHeader,
    ArchiveName,
}

impl FileCondition {
    pub fn matches(self, name: &str, facts: FileFacts) -> bool {
        match self {
            Self::Path => true,
            Self::ImageHeader => facts.image_header,
            Self::TranscribeHeader => facts.transcribe_header,
            Self::ArchiveName => crate::audio_archive::is_audio_archive_transcript_name(name),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FileFacts {
    pub transcribe_header: bool,
    pub image_header: bool,
    pub archive_part: bool,
    pub calendar_stream: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    Search,
    Dashboard,
    Agent,
    Talents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfferedTo(u8);

impl OfferedTo {
    pub const NONE: Self = Self(0);
    pub const DASHBOARD: Self = Self(2);
    pub const DASHBOARD_AGENT: Self = Self(6);
    pub const AGENT: Self = Self(4);
    pub const AGENT_TALENTS: Self = Self(12);
    pub const SEARCH_AGENT_TALENTS: Self = Self(13);
    pub const SEARCH_DASHBOARD: Self = Self(3);
    pub const SEARCH_DASHBOARD_TALENTS: Self = Self(11);
    pub const SEARCH_DASHBOARD_AGENT: Self = Self(7);
    pub const SEARCH_DASHBOARD_AGENT_TALENTS: Self = Self(15);
    pub const DASHBOARD_AGENT_TALENTS: Self = Self(14);
    pub const DASHBOARD_TALENTS: Self = Self(10);
    pub const fn contains(self, reader: Reader) -> bool {
        let bit = match reader {
            Reader::Search => 1,
            Reader::Dashboard => 2,
            Reader::Agent => 4,
            Reader::Talents => 8,
        };
        self.0 & bit != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePattern {
    pub pattern: &'static str,
    pub root: PatternRoot,
    pub format: FileFormat,
    /// Kept separate from format: recognizing a file never grants index admission.
    pub classification: ContentResolution,
    pub disposition: IndexDisposition,
    pub kind: KindAssignment,
    pub unit: ContentUnit,
    /// Static reader routes, subject to each reader's existing content and
    /// visibility predicates. Search means a text-index route, independently
    /// of connection admission (`disposition`).
    pub offered_to: OfferedTo,
    pub condition: FileCondition,
    /// Segment projection is narrower than search/fetch admission. A calendar
    /// file, for example, is fetchable but not part of an agent's segment page.
    pub segment: Option<SegmentRule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentRule {
    pub readers: OfferedTo,
    pub calendar_stream: bool,
}

impl SegmentRule {
    pub const fn new(readers: OfferedTo) -> Self {
        Self {
            readers,
            calendar_stream: false,
        }
    }
}

impl FilePattern {
    pub(super) fn index_spec(&self) -> Option<FamilyPattern> {
        let ContentResolution::Indexed(family) = self.classification else {
            return None;
        };
        Some(FamilyPattern {
            pattern: self.pattern,
            family,
            root: self.root,
            disposition: self.disposition,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentReader {
    Agent,
    Talents,
    Dashboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderException {
    PhotoAsSpeechForTalents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentOffer {
    pub format: FileFormat,
    pub exception: Option<ReaderException>,
}

pub fn markdown_transcript_name(name: &str) -> bool {
    segment_file_offers(name, SegmentReader::Agent, FileFacts::default())
        .iter()
        .any(|offer| offer.format == FileFormat::Content(Family::Markdown))
}

pub fn speech_transcript_name(name: &str) -> bool {
    segment_file_offers(name, SegmentReader::Agent, FileFacts::default())
        .iter()
        .any(|offer| offer.format == FileFormat::Raw(RawPerceptFamily::Audio))
}

/// Select one segment file for a reader. Discovery is deliberately independent
/// of search classification: custom streams and producer-selected names have
/// always been readable. The photo talent projection is explicit.
pub fn segment_file_offers(
    rel: &str,
    reader: SegmentReader,
    facts: FileFacts,
) -> Vec<SegmentOffer> {
    use std::sync::OnceLock;
    static NAMES: OnceLock<Vec<(&'static FilePattern, glob::Pattern)>> = OnceLock::new();
    let names = NAMES.get_or_init(|| {
        super::INDEX_FAMILY_PATTERNS
            .iter()
            .filter(|row| row.segment.is_some())
            .map(|row| {
                (
                    row,
                    glob::Pattern::new(row.pattern.rsplit('/').next().unwrap())
                        .expect("registered segment pattern"),
                )
            })
            .collect()
    });
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let actual = rel
        .contains('/')
        .then(|| super::file_spec_with_facts(rel, facts))
        .flatten();
    let audience = match reader {
        SegmentReader::Agent => Reader::Agent,
        SegmentReader::Talents => Reader::Talents,
        SegmentReader::Dashboard => Reader::Dashboard,
    };
    let mut offers = Vec::new();
    for (row, pattern) in names {
        let rule = row.segment.expect("compiled segment row");
        if !rule.readers.contains(audience)
            || (rule.calendar_stream && !facts.calendar_stream)
            || !row.condition.matches(name, facts)
            || !pattern.matches_with(
                name,
                glob::MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: true,
                    require_literal_leading_dot: false,
                },
            )
        {
            continue;
        }
        let chat = actual.is_some_and(|row| row.format == FileFormat::Content(Family::AiChat));
        let offer = match (reader, row.format) {
            (
                SegmentReader::Agent | SegmentReader::Talents,
                FileFormat::Raw(RawPerceptFamily::Audio),
            ) if chat => SegmentOffer {
                format: FileFormat::Content(Family::AiChat),
                exception: None,
            },
            (SegmentReader::Talents, FileFormat::PhotoDescription) => SegmentOffer {
                format: FileFormat::Raw(RawPerceptFamily::Audio),
                exception: Some(ReaderException::PhotoAsSpeechForTalents),
            },
            _ => SegmentOffer {
                format: row.format,
                exception: None,
            },
        };
        if !offers.contains(&offer) {
            offers.push(offer);
        }
    }
    // Only registered conversation imports have a dashboard transcript view.
    // The basename rules above still supply the legacy audio and talent views.
    if reader == SegmentReader::Dashboard
        && name == "conversation_transcript.jsonl"
        && let Some(row) = actual
        && row.format == FileFormat::Content(Family::AiChat)
        && row.offered_to.contains(Reader::Dashboard)
    {
        offers.push(SegmentOffer {
            format: row.format,
            exception: None,
        });
    }
    // Placement, rather than a filename convention, identifies archive parts
    // for this reader. The caller obtained this fact from placement.json.
    if reader == SegmentReader::Dashboard && facts.archive_part {
        let audio = SegmentOffer {
            format: FileFormat::Raw(RawPerceptFamily::Audio),
            exception: None,
        };
        if !offers.contains(&audio) {
            offers.push(audio);
        }
    }
    // Dashboard's existing if/else precedence is part of its projection.
    // Talent readers retain every independently matching projection.
    offers.sort_by_key(|offer| match offer.format {
        FileFormat::Raw(RawPerceptFamily::Audio) => 0,
        FileFormat::Raw(RawPerceptFamily::RawScreen) => 1,
        FileFormat::Content(Family::Browser) => 2,
        FileFormat::Content(Family::Calendar) => 3,
        FileFormat::Content(Family::ProtectedVideo) => 4,
        FileFormat::Content(Family::DesktopActivity) => 5,
        _ => 6,
    });
    offers
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::content::file_spec;

    #[test]
    fn offered_audiences_match_each_segment_rule() {
        for row in super::super::INDEX_FAMILY_PATTERNS {
            let Some(rule) = row.segment else { continue };
            for reader in [Reader::Dashboard, Reader::Agent, Reader::Talents] {
                assert_eq!(
                    row.offered_to.contains(reader),
                    rule.readers.contains(reader),
                    "{} ({:?}, {reader:?})",
                    row.pattern,
                    row.condition,
                );
            }
        }
    }

    #[test]
    fn offered_search_matches_text_index_routes() {
        for row in super::super::INDEX_FAMILY_PATTERNS {
            let searchable = match row.classification {
                // Disposition controls connection admission, not indexing.
                ContentResolution::Indexed(_) => true,
                ContentResolution::IndexedElsewhere => match row.pattern {
                    // The entity-search builder projects identity text into
                    // synthetic rows. Weekly JSON accompanies indexed Markdown.
                    "entities/*/entity.json" => true,
                    "reflections/weekly/*.json" => false,
                    _ => panic!("unaccounted separate index route: {}", row.pattern),
                },
                ContentResolution::Unindexed(_) | ContentResolution::Unrecognized => false,
            };
            assert_eq!(
                row.offered_to.contains(Reader::Search),
                searchable,
                "{} ({:?}, {:?})",
                row.pattern,
                row.classification,
                row.disposition,
            );
        }
    }

    fn missing_files(root: &Path, at: &Path, missing: &mut Vec<String>) {
        for entry in std::fs::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                missing_files(root, &path, missing);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|part| part.as_os_str().to_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("/");
                let image_header = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| text.lines().next().map(str::to_owned))
                    .and_then(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
                    .is_some_and(|header| header["kind"] == "image");
                let facts = FileFacts {
                    image_header,
                    transcribe_header: solstone_core_processing_record::is_transcribe_output(&path),
                    ..Default::default()
                };
                if super::super::file_spec_with_facts(&rel, facts).is_none() {
                    missing.push(rel);
                }
            }
        }
    }

    #[test]
    fn fixture_journal_files_have_explicit_registry_rows() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/journal");
        let mut missing = Vec::new();
        missing_files(&root, &root, &mut missing);
        let extra = root.with_file_name("registry-journal");
        missing_files(&extra, &extra, &mut missing);
        missing.sort();
        assert!(
            missing.is_empty(),
            "files without registry rows: {missing:#?}"
        );
        let control = tempfile::tempdir().unwrap();
        let unknown = "chronicle/20260107/custom/120000_60/mystery-content.jsonl";
        let path = control.path().join(unknown);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "{\"text\":\"unknown content\"}\n").unwrap();
        let mut negative = Vec::new();
        missing_files(control.path(), control.path(), &mut negative);
        assert_eq!(negative, [unknown]);
        assert_eq!(
            file_spec("config/journal.json").unwrap().kind,
            KindAssignment::NotContent
        );
    }

    #[test]
    fn registry_rows_have_fixture_witnesses() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/registry-journal");
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../tests/fixtures/registry-cases.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let rel = case["path"].as_str().unwrap();
            assert!(root.join(rel).is_file());
            let facts = FileFacts {
                image_header: case["condition"] == "ImageHeader",
                transcribe_header: case["condition"] == "TranscribeHeader",
                ..Default::default()
            };
            let spec = super::super::file_spec_with_facts(rel, facts).unwrap();
            assert_eq!(
                format!("{:?}", spec.condition),
                case["condition"].as_str().unwrap()
            );
            assert_eq!(spec.pattern, case["pattern"].as_str().unwrap(), "{rel}");
        }
        for row in super::super::INDEX_FAMILY_PATTERNS {
            assert!(
                cases
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|case| case["pattern"] == row.pattern
                        && case["condition"] == format!("{:?}", row.condition)),
                "{} has no witness",
                row.pattern
            );
        }
    }

    #[test]
    fn imported_conversation_adds_only_a_dashboard_segment_offer() {
        let facts = FileFacts::default();
        let chat = FileFormat::Content(Family::AiChat);
        let audio = FileFormat::Raw(RawPerceptFamily::Audio);
        for source in ["text", "chatgpt", "claude", "gemini"] {
            let rel = format!("20260107/import.{source}/120000_60/conversation_transcript.jsonl");
            assert_eq!(
                super::super::classify(&rel),
                ContentResolution::Indexed(Family::AiChat)
            );
            assert!(file_spec(&rel).unwrap().offered_to.contains(Reader::Search));
            assert_eq!(
                segment_file_offers(&rel, SegmentReader::Agent, facts),
                [SegmentOffer {
                    format: chat,
                    exception: None
                }]
            );
            assert_eq!(
                segment_file_offers(&rel, SegmentReader::Talents, facts),
                [SegmentOffer {
                    format: chat,
                    exception: None
                }]
            );
            assert_eq!(
                segment_file_offers(&rel, SegmentReader::Dashboard, facts),
                [SegmentOffer {
                    format: chat,
                    exception: None
                }]
            );
        }
        let unregistered = "20260107/custom/120000_60/conversation_transcript.jsonl";
        assert!(segment_file_offers(unregistered, SegmentReader::Dashboard, facts).is_empty());
        let legacy = "20260107/import.claude/120000_60/imported_audio.jsonl";
        assert_eq!(
            segment_file_offers(legacy, SegmentReader::Dashboard, facts),
            [SegmentOffer {
                format: audio,
                exception: None
            }]
        );
    }

    #[test]
    fn reader_selection_preserves_custom_streams_headers_and_named_exceptions() {
        use SegmentReader as R;
        for name in [
            "audio.jsonl",
            "imported_audio.jsonl",
            "conversation_transcript.jsonl",
            "p0001_0123456789abcdef.jsonl",
            "note_transcript.md",
            "imported.md",
            "body.jsonl",
            "workout.json",
        ] {
            let rel = format!("20260107/custom/120000_60/{name}");
            assert!(
                !segment_file_offers(&rel, R::Agent, FileFacts::default()).is_empty(),
                "{name}"
            );
        }
        let arbitrary = "20260107/custom/120000_60/recording.jsonl";
        assert!(segment_file_offers(arbitrary, R::Agent, FileFacts::default()).is_empty());
        // The producer-owned header can recognize a filename whose basename
        // could not be represented as UTF-8 by a filesystem reader.
        assert!(
            !segment_file_offers(
                "",
                R::Agent,
                FileFacts {
                    transcribe_header: true,
                    ..Default::default()
                }
            )
            .is_empty()
        );
        assert!(
            !segment_file_offers(
                arbitrary,
                R::Agent,
                FileFacts {
                    transcribe_header: true,
                    ..Default::default()
                }
            )
            .is_empty()
        );
        assert_eq!(
            super::super::classify(arbitrary),
            ContentResolution::Unrecognized
        );
        let chat = "20260107/import.claude/120000_60/conversation_transcript.jsonl";
        assert_eq!(
            segment_file_offers(chat, R::Agent, FileFacts::default())[0].format,
            FileFormat::Content(Family::AiChat)
        );
        assert_eq!(
            segment_file_offers(chat, R::Talents, FileFacts::default())[0].format,
            FileFormat::Content(Family::AiChat)
        );
        let photo = "20260107/camera/120000_60/photo.jsonl";
        let image = FileFacts {
            image_header: true,
            ..Default::default()
        };
        assert!(segment_file_offers(photo, R::Agent, image).is_empty());
        assert_eq!(
            segment_file_offers(photo, R::Talents, image)[0].exception,
            Some(ReaderException::PhotoAsSpeechForTalents)
        );
        assert_eq!(
            super::super::classify(photo),
            ContentResolution::Unrecognized
        );
        assert!(segment_file_offers("radioaudio.jsonl", R::Agent, FileFacts::default()).is_empty());
        assert!(
            !segment_file_offers("radioaudio.jsonl", R::Talents, FileFacts::default()).is_empty()
        );
        assert!(
            segment_file_offers(
                "20260107/custom/120000_60/location.jsonl",
                R::Agent,
                FileFacts::default()
            )
            .is_empty()
        );
        assert!(
            segment_file_offers(
                "20260107/custom/120000_60/location.jsonl",
                R::Talents,
                FileFacts::default()
            )
            .is_empty()
        );
        assert_eq!(
            segment_file_offers(
                "calendar_note.txt",
                R::Dashboard,
                FileFacts {
                    calendar_stream: true,
                    ..Default::default()
                }
            )[0]
            .format,
            FileFormat::Content(Family::Calendar)
        );
        assert!(
            segment_file_offers("calendar_note.txt", R::Dashboard, FileFacts::default()).is_empty()
        );
        for (name, first) in [
            (
                "browser_audio.jsonl",
                FileFormat::Raw(RawPerceptFamily::Audio),
            ),
            (
                "browser_screen.jsonl",
                FileFormat::Raw(RawPerceptFamily::RawScreen),
            ),
        ] {
            let offers = segment_file_offers(name, R::Talents, FileFacts::default());
            assert_eq!(
                offers.iter().map(|offer| offer.format).collect::<Vec<_>>(),
                [first, FileFormat::Content(Family::Browser)]
            );
        }
    }

    #[test]
    fn effective_admission_is_path_specific_even_when_kinds_match() {
        use super::super::kinds::Kind as K;
        use super::super::{AdmittedCategory as P, classify, resolve_spec};
        use KindAssignment::{Legacy, Settled};
        let cases = [
            (
                "20260107/default/120000_60/audio.jsonl",
                Some(K::Transcripts),
                Some(P::Transcripts),
            ),
            (
                "20260107/import.document/120000_60/document_transcript.md",
                Some(K::Documents),
                Some(P::Documents),
            ),
            (
                "20260107/default/120000_60/talents/screen_text.md",
                Some(K::Screen),
                Some(P::ScreenText),
            ),
            (
                "20260107/default/120000_60/talents/screen.md",
                Some(K::Screen),
                Some(P::ScreenText),
            ),
            (
                "20260107/import.apple_health/120000_60/body.jsonl",
                Some(K::Body),
                Some(P::Body),
            ),
            (
                "20260107/import.ics/120000_60/event_transcript.md",
                Some(K::Calendar),
                Some(P::Calendar),
            ),
            ("20260107/location.md", Some(K::Places), Some(P::Places)),
            (
                "20260107/default/120000_60/location.jsonl",
                Some(K::Places),
                None,
            ),
            (
                "20260107/import.image/120000_60/image_transcript.md",
                Some(K::Photos),
                Some(P::Photos),
            ),
            (
                "20260107/import.image/120000_60/original.jpg",
                Some(K::Photos),
                None,
            ),
            (
                "facets/work/activities/20260107.jsonl",
                Some(K::Facets),
                Some(P::Facets),
            ),
            (
                "facets/work/entities/20260107.jsonl",
                Some(K::Entities),
                Some(P::Entities),
            ),
            (
                "20260107/agent-memory-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/120000_60/note.txt",
                Some(K::Notes),
                Some(P::Notes),
            ),
        ];
        for (path, kind, permission) in cases {
            let spec = file_spec(path).unwrap();
            match (kind, spec.kind) {
                (Some(expected), Settled(actual) | Legacy(actual)) => assert_eq!(expected, actual),
                _ => panic!("unexpected kind for {path}"),
            }
            let current = if super::super::body_material::is_body_path(path) {
                Some(P::Body)
            } else if let Some(spec) = resolve_spec(path) {
                match spec.disposition {
                    IndexDisposition::Admitted { category, .. } => Some(
                        super::super::kinds::indexed_permission(None, path, category),
                    ),
                    IndexDisposition::Excluded => None,
                }
            } else if matches!(
                classify(path),
                ContentResolution::Unindexed(RawPerceptFamily::Audio)
            ) {
                Some(P::Transcripts)
            } else {
                None
            };
            assert_eq!(current, permission, "{path}");
        }
    }

    #[test]
    fn metadata_rows_do_not_shadow_old_classification_domains() {
        use super::super::{classify, resolve_spec};
        assert_eq!(
            classify("config/talents/example.md"),
            ContentResolution::Indexed(Family::Markdown)
        );
        assert!(resolve_spec("config/talents/example.md").is_some());
        assert_eq!(
            classify("config/a/b/audio.jsonl"),
            ContentResolution::Indexed(Family::Audio)
        );
        assert_eq!(
            classify("chronicle/20260107/default/120000_60/audio.jsonl"),
            ContentResolution::Unrecognized
        );
        assert_eq!(
            file_spec("chronicle/20260107/120000_60/audio.jsonl")
                .unwrap()
                .format,
            FileFormat::Raw(RawPerceptFamily::Audio)
        );
        assert_eq!(
            classify("20260107/120000_60/audio.jsonl"),
            ContentResolution::Unrecognized
        );
    }
}
